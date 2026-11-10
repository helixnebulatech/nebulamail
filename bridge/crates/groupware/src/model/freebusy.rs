// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The `VFREEBUSY` reply of a scheduling outbox (RFC 6638 §5, RFC 5545
//! §3.8.2.6) as busy periods, the way `Principal/getAvailability`
//! returns them (draft-ietf-jmap-calendars, `BusyPeriod`).
//!
//! The text comes from the DAV server: it is parsed with calcard inside
//! the panic guard and the size limit of [`convert`](super::convert).
//! `FBTYPE=BUSY` (the default) is `confirmed`, `BUSY-TENTATIVE`
//! `tentative`, `BUSY-UNAVAILABLE` `unavailable`, `FREE` is left out and
//! an unknown type counts as busy (RFC 5545 §3.2.9). Periods must be in
//! UTC (§3.8.2.6); a floating one is left out. Each period is cut to the
//! range asked for; they come sorted, at most [`MAX_BUSY_PERIODS`].
//!
//! The user's own busy times come from the local copy instead
//! ([`own_busy_periods`], one stored object at a time), with the rules of
//! CalDAV's `free-busy-query` (RFC 4791 §7.10) on each occurrence:
//!
//! - only events (`VEVENT`): tasks, journals and `VFREEBUSY` don't count;
//! - `STATUS:CANCELLED` and `TRANSP:TRANSPARENT` (JSCalendar
//!   `freeBusyStatus: "free"`) are free; `STATUS:TENTATIVE` is
//!   `tentative`, anything else `confirmed`;
//! - Outlook's `X-MICROSOFT-CDO-BUSYSTATUS`: `FREE` free, `TENTATIVE`
//!   tentative, `OOF` unavailable;
//! - as an attendee (one of `own`, the user's addresses), a `DECLINED`
//!   invitation is free and one not answered yet (`NEEDS-ACTION`) or
//!   `TENTATIVE` is `tentative`;
//! - an event without length (`DTSTART` only) takes no time.
//!
//! Occurrences come from [`ParsedCalendar::instances`] (recurrences,
//! overrides, time zones; floating and all-day times in the calendar's
//! zone), with its limits.

use calcard::common::PartialDateTime;
use calcard::icalendar::{
    ICalendar, ICalendarClassification, ICalendarComponent, ICalendarComponentType,
    ICalendarFreeBusyType, ICalendarParameterName, ICalendarParameterValue,
    ICalendarParticipationStatus, ICalendarPeriod, ICalendarProperty, ICalendarStatus,
    ICalendarTransparency, ICalendarValue,
};

use super::calendar::{ParsedCalendar, RecurrenceKey, Zone};
use super::convert::{Format, checked_text, guarded};
use crate::ConvertError;

/// Most busy periods returned for one person and range.
pub const MAX_BUSY_PERIODS: usize = 10_000;

/// How busy a period is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum BusyStatus {
    /// `FBTYPE=BUSY` (JMAP `confirmed`).
    Confirmed,
    /// `FBTYPE=BUSY-TENTATIVE` (JMAP `tentative`).
    Tentative,
    /// `FBTYPE=BUSY-UNAVAILABLE` (JMAP `unavailable`).
    Unavailable,
}

/// A busy period, in seconds since the epoch (UTC), `start < end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BusyPeriod {
    pub start: i64,
    pub end: i64,
    pub status: BusyStatus,
}

/// The busy periods of a `VFREEBUSY` reply between `from` and `to`.
///
/// # Errors
///
/// [`ConvertError::TooLarge`], [`ConvertError::NotUtf8`],
/// [`ConvertError::Invalid`] (not iCalendar, or no `VFREEBUSY`), or
/// [`ConvertError::Panicked`].
pub fn busy_periods(input: &[u8], from: i64, to: i64) -> Result<Vec<BusyPeriod>, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(input, format)?;
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let replies: Vec<_> = ical
            .components
            .iter()
            .filter(|c| c.component_type == ICalendarComponentType::VFreebusy)
            .collect();
        if replies.is_empty() {
            return Err(ConvertError::Invalid { format });
        }
        let mut periods = Vec::new();
        let entries = replies
            .iter()
            .flat_map(|reply| reply.entries.iter())
            .filter(|entry| entry.name == ICalendarProperty::Freebusy);
        for entry in entries {
            let status = match entry.parameter(&ICalendarParameterName::Fbtype) {
                Some(ICalendarParameterValue::Fbtype(ICalendarFreeBusyType::Free)) => continue,
                Some(ICalendarParameterValue::Fbtype(ICalendarFreeBusyType::BusyTentative)) => {
                    BusyStatus::Tentative
                }
                Some(ICalendarParameterValue::Fbtype(ICalendarFreeBusyType::BusyUnavailable)) => {
                    BusyStatus::Unavailable
                }
                _ => BusyStatus::Confirmed,
            };
            for value in &entry.values {
                let Some((start, end)) = period(value) else {
                    continue;
                };
                let (start, end) = (start.max(from), end.min(to));
                if start < end {
                    periods.push(BusyPeriod { start, end, status });
                    if periods.len() >= MAX_BUSY_PERIODS {
                        break;
                    }
                }
            }
            if periods.len() >= MAX_BUSY_PERIODS {
                break;
            }
        }
        periods.sort_unstable();
        periods.dedup();
        Ok(periods)
    })
}

/// A UTC period as start and end, if it is one.
fn period(value: &ICalendarValue) -> Option<(i64, i64)> {
    let ICalendarValue::Period(period) = value else {
        return None;
    };
    match period.as_ref() {
        ICalendarPeriod::Range { start, end } => {
            let (start, end) = (utc(start)?, utc(end)?);
            (start < end).then_some((start, end))
        }
        ICalendarPeriod::Duration { start, duration } => {
            let start = utc(start)?;
            let length = duration.as_seconds();
            (length > 0).then_some(())?;
            Some((start, start.checked_add(length)?))
        }
    }
}

fn utc(time: &PartialDateTime) -> Option<i64> {
    (time.has_date_and_time() && time.has_zone())
        .then(|| time.to_timestamp())
        .flatten()
}

/// How an event component counts for the user's availability: `None` is
/// free.
fn busy_status(component: &ICalendarComponent, own: &[String]) -> Option<BusyStatus> {
    if component.component_type != ICalendarComponentType::VEvent {
        return None;
    }
    let first = |name: &ICalendarProperty| {
        component
            .property(name)
            .and_then(|entry| entry.values.first())
    };
    let mut status = match first(&ICalendarProperty::Status) {
        Some(ICalendarValue::Status(ICalendarStatus::Cancelled)) => return None,
        Some(ICalendarValue::Status(ICalendarStatus::Tentative)) => BusyStatus::Tentative,
        _ => BusyStatus::Confirmed,
    };
    if matches!(
        first(&ICalendarProperty::Transp),
        Some(ICalendarValue::Transparency(
            ICalendarTransparency::Transparent
        ))
    ) {
        return None;
    }
    let outlook = component
        .entries
        .iter()
        .find(|entry| {
            matches!(&entry.name, ICalendarProperty::Other(name)
                if name.eq_ignore_ascii_case("X-MICROSOFT-CDO-BUSYSTATUS"))
        })
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_text);
    match outlook.map(str::to_ascii_uppercase).as_deref() {
        Some("FREE") => return None,
        Some("TENTATIVE") => status = status.max(BusyStatus::Tentative),
        Some("OOF") => status = BusyStatus::Unavailable,
        _ => {}
    }
    let mine = component
        .properties(&ICalendarProperty::Attendee)
        .find(|entry| {
            entry
                .calendar_address()
                .is_some_and(|address| own.iter().any(|o| o.eq_ignore_ascii_case(address)))
        });
    if let Some(entry) = mine {
        match entry.parameter(&ICalendarParameterName::Partstat) {
            Some(ICalendarParameterValue::Partstat(ICalendarParticipationStatus::Declined)) => {
                return None;
            }
            Some(ICalendarParameterValue::Partstat(
                ICalendarParticipationStatus::NeedsAction | ICalendarParticipationStatus::Tentative,
            ))
            | None
                if status == BusyStatus::Confirmed =>
            {
                status = BusyStatus::Tentative;
            }
            _ => {}
        }
    }
    Some(status)
}

/// One busy occurrence of the user's: the period and which occurrence
/// of the object it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnBusy {
    /// The period (cut to the range).
    pub period: BusyPeriod,
    /// The occurrence (for `ParsedCalendar::occurrence`).
    pub key: RecurrenceKey,
    /// Its details may be shown: the component of the occurrence has no
    /// `CLASS` or `CLASS:PUBLIC` (`PRIVATE`, `CONFIDENTIAL` or another
    /// one: not; JSCalendar `privacy` other than `public`).
    pub shareable: bool,
}

/// Whether a component's details may be shown ([`OwnBusy::shareable`]).
fn is_shareable(component: &ICalendarComponent) -> bool {
    match component
        .property(&ICalendarProperty::Class)
        .and_then(|entry| entry.values.first())
    {
        None => true,
        Some(value) => matches!(
            value,
            ICalendarValue::Classification(ICalendarClassification::Public)
        ),
    }
}

/// The user's busy periods in one stored object between `from` and `to`
/// (UTC seconds), floating and all-day times read in `zone` (the
/// calendar's). `own`: the user's addresses, without `mailto:`. Cut to
/// the range, sorted, at most [`MAX_BUSY_PERIODS`].
///
/// # Errors
/// [`ConvertError::Panicked`] if calcard fails.
pub fn own_busy_periods(
    parsed: &ParsedCalendar,
    zone: Zone,
    range: (i64, i64),
    own: &[String],
) -> Result<Vec<BusyPeriod>, ConvertError> {
    let mut periods: Vec<BusyPeriod> = own_busy_occurrences(parsed, zone, range, own)?
        .into_iter()
        .map(|busy| busy.period)
        .collect();
    periods.dedup();
    Ok(periods)
}

/// [`own_busy_periods`] with the occurrence of each period, sorted by
/// period.
///
/// # Errors
/// [`ConvertError::Panicked`] if calcard fails.
pub fn own_busy_occurrences(
    parsed: &ParsedCalendar,
    zone: Zone,
    (from, to): (i64, i64),
    own: &[String],
) -> Result<Vec<OwnBusy>, ConvertError> {
    let instances = parsed.instances(zone)?;
    guarded(Format::ICalendar, || {
        let components = &parsed.icalendar().components;
        let mut found = Vec::new();
        for instance in &instances.list {
            if instance.is_task() || instance.utc_end <= from || instance.utc_start >= to {
                continue;
            }
            let Some(component) = usize::try_from(instance.component())
                .ok()
                .and_then(|index| components.get(index))
            else {
                continue;
            };
            let Some(status) = busy_status(component, own) else {
                continue;
            };
            let (start, end) = (instance.utc_start.max(from), instance.utc_end.min(to));
            if start < end {
                found.push(OwnBusy {
                    period: BusyPeriod { start, end, status },
                    key: instance.key,
                    shareable: is_shareable(component),
                });
                if found.len() >= MAX_BUSY_PERIODS {
                    break;
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        Ok(found)
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const FROM: i64 = 1_792_454_400; // 2026-10-20T00:00:00Z
    const TO: i64 = 1_793_664_000; // 2026-11-03T00:00:00Z

    /// Stalwart 0.16.27's reply for alice (LF line ends as in its CDATA,
    /// made CRLF by the DAV client; a folded line).
    const STALWART: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Stalwart Labs LLC//Stalwart Server//EN\r\n\
        METHOD:REPLY\r\nBEGIN:VFREEBUSY\r\nDTSTART:20261020T000000Z\r\nDTEND:20261103T000000Z\r\n\
        DTSTAMP:20261027T192349Z\r\n\
        FREEBUSY;FBTYPE=BUSY:20261020T133000Z/20261020T143000Z,20261021T150000Z/202\r\n \
        61021T160000Z,20261029T000000Z/20261031T000000Z\r\n\
        ORGANIZER:mailto:alice@nebulamail.test\r\nATTENDEE:mailto:alice@nebulamail.test\r\n\
        UID:fb-test-1\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";

    /// `YYYY-MM-DDTHH:MM:SSZ` as seconds since the epoch.
    fn at(text: &str) -> i64 {
        let n: Vec<i64> = text
            .trim_end_matches('Z')
            .split(['-', 'T', ':'])
            .map(|part| part.parse().unwrap())
            .collect();
        let (y, m, d) = (n[0], n[1], n[2]);
        // Howard Hinnant's days_from_civil.
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + n[3] * 3600 + n[4] * 60 + n[5]
    }

    #[test]
    fn stalwart_reply() {
        let periods = busy_periods(STALWART.as_bytes(), FROM, TO).unwrap();
        assert_eq!(
            periods,
            [
                BusyPeriod {
                    start: at("2026-10-20T13:30:00Z"),
                    end: at("2026-10-20T14:30:00Z"),
                    status: BusyStatus::Confirmed
                },
                BusyPeriod {
                    start: at("2026-10-21T15:00:00Z"),
                    end: at("2026-10-21T16:00:00Z"),
                    status: BusyStatus::Confirmed
                },
                BusyPeriod {
                    start: at("2026-10-29T00:00:00Z"),
                    end: at("2026-10-31T00:00:00Z"),
                    status: BusyStatus::Confirmed
                },
            ]
        );
        // An empty reply (nothing shared, or free): no periods.
        let empty = "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nBEGIN:VFREEBUSY\r\n\
                     UID:x\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";
        assert_eq!(busy_periods(empty.as_bytes(), FROM, TO).unwrap(), []);
    }

    #[test]
    fn types_durations_and_clipping() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:x\r\n\
            FREEBUSY;FBTYPE=BUSY-TENTATIVE:20261022T090000Z/PT1H\r\n\
            FREEBUSY;FBTYPE=BUSY-UNAVAILABLE:20261023T090000Z/20261023T100000Z\r\n\
            FREEBUSY;FBTYPE=FREE:20261024T090000Z/20261024T100000Z\r\n\
            FREEBUSY;FBTYPE=X-OUT-OF-OFFICE:20261025T090000Z/20261025T100000Z\r\n\
            FREEBUSY:20261019T230000Z/20261020T010000Z\r\n\
            FREEBUSY:20261102T230000Z/20261103T010000Z\r\n\
            FREEBUSY:20261201T090000Z/20261201T100000Z\r\n\
            FREEBUSY:20261026T090000/20261026T100000\r\n\
            FREEBUSY:20261027T100000Z/20261027T090000Z\r\n\
            END:VFREEBUSY\r\nEND:VCALENDAR\r\n";
        let periods = busy_periods(text.as_bytes(), FROM, TO).unwrap();
        let got: Vec<(i64, i64, BusyStatus)> =
            periods.iter().map(|p| (p.start, p.end, p.status)).collect();
        assert_eq!(
            got,
            [
                // Cut to the start of the range.
                (FROM, at("2026-10-20T01:00:00Z"), BusyStatus::Confirmed),
                (
                    at("2026-10-22T09:00:00Z"),
                    at("2026-10-22T10:00:00Z"),
                    BusyStatus::Tentative
                ),
                (
                    at("2026-10-23T09:00:00Z"),
                    at("2026-10-23T10:00:00Z"),
                    BusyStatus::Unavailable
                ),
                // An unknown type counts as busy.
                (
                    at("2026-10-25T09:00:00Z"),
                    at("2026-10-25T10:00:00Z"),
                    BusyStatus::Confirmed
                ),
                // Cut to the end of the range. FREE, floating, reversed and
                // outside periods are gone.
                (at("2026-11-02T23:00:00Z"), TO, BusyStatus::Confirmed),
            ]
        );
    }

    #[test]
    fn hostile_replies() {
        assert!(matches!(
            busy_periods(b"not a calendar", FROM, TO),
            Err(ConvertError::Invalid { .. })
        ));
        let event_only =
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert!(busy_periods(event_only.as_bytes(), FROM, TO).is_err());
        assert!(matches!(
            busy_periods(&[0xff, 0xfe], FROM, TO),
            Err(ConvertError::NotUtf8 { .. })
        ));
        let huge = vec![b'a'; super::super::convert::MAX_ICALENDAR_BYTES + 1];
        assert!(matches!(
            busy_periods(&huge, FROM, TO),
            Err(ConvertError::TooLarge { .. })
        ));
        // Many periods: capped.
        let mut many = String::from("BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nUID:x\r\n");
        for minute in 0..(MAX_BUSY_PERIODS + 50) {
            let start = FROM + i64::try_from(minute).unwrap() * 60;
            write!(many, "FREEBUSY:{}/PT30S\r\n", stamp(start)).unwrap();
        }
        many.push_str("END:VFREEBUSY\r\nEND:VCALENDAR\r\n");
        let periods = busy_periods(many.as_bytes(), FROM, TO).unwrap();
        assert_eq!(periods.len(), MAX_BUSY_PERIODS);
    }

    fn stamp(seconds: i64) -> String {
        let dt = PartialDateTime::from_utc_timestamp(seconds);
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            dt.year.unwrap(),
            dt.month.unwrap(),
            dt.day.unwrap(),
            dt.hour.unwrap(),
            dt.minute.unwrap(),
            dt.second.unwrap()
        )
    }

    fn own(events: &str, zone: Zone) -> Vec<(i64, i64, BusyStatus)> {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n{}END:VCALENDAR\r\n",
            events.replace('\n', "\r\n")
        );
        let parsed = ParsedCalendar::parse(text.as_bytes()).unwrap();
        own_busy_periods(&parsed, zone, (FROM, TO), &["alice@example.com".to_owned()])
            .unwrap()
            .into_iter()
            .map(|p| (p.start, p.end, p.status))
            .collect()
    }

    fn event(uid: &str, lines: &str) -> String {
        format!("BEGIN:VEVENT\nUID:{uid}\nDTSTAMP:20261001T000000Z\n{lines}END:VEVENT\n")
    }

    #[test]
    fn own_statuses_and_transparency() {
        let utc = Zone::UTC;
        let one = |lines: &str| {
            own(
                &event(
                    "x",
                    &format!("DTSTART:20261022T090000Z\nDTEND:20261022T100000Z\n{lines}"),
                ),
                utc,
            )
        };
        let busy = |status| {
            vec![(
                at("2026-10-22T09:00:00Z"),
                at("2026-10-22T10:00:00Z"),
                status,
            )]
        };
        assert_eq!(one(""), busy(BusyStatus::Confirmed));
        assert_eq!(one("STATUS:CONFIRMED\n"), busy(BusyStatus::Confirmed));
        assert_eq!(one("STATUS:TENTATIVE\n"), busy(BusyStatus::Tentative));
        assert_eq!(one("STATUS:CANCELLED\n"), []);
        assert_eq!(one("TRANSP:TRANSPARENT\n"), []);
        assert_eq!(one("TRANSP:OPAQUE\n"), busy(BusyStatus::Confirmed));
        assert_eq!(one("X-MICROSOFT-CDO-BUSYSTATUS:FREE\n"), []);
        assert_eq!(
            one("X-MICROSOFT-CDO-BUSYSTATUS:OOF\n"),
            busy(BusyStatus::Unavailable)
        );
        assert_eq!(
            one("X-MICROSOFT-CDO-BUSYSTATUS:TENTATIVE\n"),
            busy(BusyStatus::Tentative)
        );
        // As an attendee: declined is free, not answered or tentative is
        // tentative, accepted is busy. Others' answers don't matter.
        let invited = |partstat: &str| {
            one(&format!(
                "ORGANIZER:mailto:bob@example.com\n\
                 ATTENDEE;PARTSTAT=DECLINED:mailto:carol@example.com\n\
                 ATTENDEE{partstat}:mailto:Alice@Example.com\n"
            ))
        };
        assert_eq!(invited(";PARTSTAT=DECLINED"), []);
        assert_eq!(
            invited(";PARTSTAT=NEEDS-ACTION"),
            busy(BusyStatus::Tentative)
        );
        assert_eq!(invited(""), busy(BusyStatus::Tentative));
        assert_eq!(invited(";PARTSTAT=TENTATIVE"), busy(BusyStatus::Tentative));
        assert_eq!(invited(";PARTSTAT=ACCEPTED"), busy(BusyStatus::Confirmed));
        // Without length: no time taken.
        assert_eq!(own(&event("p", "DTSTART:20261022T090000Z\n"), utc), []);
        // A task is not busy time.
        let task = "BEGIN:VTODO\nUID:t\nDTSTAMP:20261001T000000Z\nDTSTART:20261022T090000Z\n\
                    DUE:20261022T100000Z\nEND:VTODO\n";
        assert_eq!(own(task, utc), []);
    }

    #[test]
    fn own_recurrences_zones_and_clipping() {
        let madrid = Zone::named("Europe/Madrid").unwrap();
        // Daily at 10:00 Madrid from 18 Oct: 08:00 UTC until the change on
        // 25 Oct, 09:00 UTC after; one occurrence cancelled, one moved and
        // one deleted. The range starts on 20 Oct.
        let series = format!(
            "{}{}{}",
            event(
                "s",
                "DTSTART;TZID=Europe/Madrid:20261018T100000\nDURATION:PT1H\n\
                 RRULE:FREQ=DAILY;COUNT=10\nEXDATE;TZID=Europe/Madrid:20261022T100000\n"
            ),
            event(
                "s",
                "RECURRENCE-ID;TZID=Europe/Madrid:20261021T100000\n\
                 DTSTART;TZID=Europe/Madrid:20261021T150000\nDURATION:PT1H\n"
            ),
            event(
                "s",
                "RECURRENCE-ID;TZID=Europe/Madrid:20261023T100000\n\
                 DTSTART;TZID=Europe/Madrid:20261023T100000\nDURATION:PT1H\nSTATUS:CANCELLED\n"
            ),
        );
        let got = own(&series, madrid);
        let starts: Vec<i64> = got.iter().map(|p| p.0).collect();
        assert_eq!(
            starts,
            [
                at("2026-10-20T08:00:00Z"),
                at("2026-10-21T13:00:00Z"),
                at("2026-10-24T08:00:00Z"),
                at("2026-10-25T09:00:00Z"),
                at("2026-10-26T09:00:00Z"),
                at("2026-10-27T09:00:00Z"),
            ]
        );
        // An all-day event in the calendar's zone, and one that starts
        // before the range (cut).
        let all_day = event(
            "d",
            "DTSTART;VALUE=DATE:20261030\nDTEND;VALUE=DATE:20261031\n",
        );
        assert_eq!(
            own(&all_day, madrid),
            [(
                at("2026-10-29T23:00:00Z"),
                at("2026-10-30T23:00:00Z"),
                BusyStatus::Confirmed
            )]
        );
        let early = event("e", "DTSTART:20261019T220000Z\nDTEND:20261020T020000Z\n");
        assert_eq!(
            own(&early, Zone::UTC),
            [(FROM, at("2026-10-20T02:00:00Z"), BusyStatus::Confirmed)]
        );
    }
}
