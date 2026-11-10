// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Proposals of another time (iTIP, RFC 5546 §3.2.7 and §3.2.8) that the
//! bridge sends for the web, without I/O.
//! CPU work: call it off the async threads.
//!
//! - [`counter`]: an attendee proposes another time to the organizer
//!   (`COUNTER`). Nothing is written to the calendar: the organizer
//!   decides. The event comes from the calendar (a stored event, maybe one
//!   of its occurrences) or from the invitation email itself (when it is
//!   not in the calendar).
//! - [`decline_counter`]: the organizer keeps the time (`DECLINECOUNTER`)
//!   to the attendee whose `COUNTER` email they got.
//!
//! Everything in the messages comes from the stored event or the email,
//! never from the client but the proposed times and the note: `UID`,
//! `SEQUENCE`, `RECURRENCE-ID`, organizer and attendee as the calendar
//! has them. Times go in UTC (no `VTIMEZONE` needed; one is added for a
//! `RECURRENCE-ID` with `TZID`). The web builds the same messages for
//! Stalwart (`apps/web/src/calendar/itip.ts`).

use calcard::common::PartialDateTime;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarMethod,
    ICalendarParameterName, ICalendarParameterValue, ICalendarProperty, ICalendarValue,
};

use calcard::common::timezone::Tz;

use super::calendar::{ParsedCalendar, RecurrenceKey, Zone, component_zone, rid_naive};
use super::itip::{
    ItipMessage, Method, Party, Role, Subject, component, events, is_own, message, organizer_of,
    parse, recurrence_id, role, sequence, text_of, when, without_schedule_params,
};
use crate::ConvertError;

/// Longest note, in characters (the web's `MAX_COMMENT`).
pub const MAX_COMMENT: usize = 2000;

/// Why a proposal can't be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CounterError {
    /// A text does not parse.
    #[error(transparent)]
    Convert(#[from] ConvertError),
    /// The user is not invited to the event (or organizes it).
    #[error("the user is not invited to this event")]
    NotInvited,
    /// The event has no such occurrence.
    #[error("no such occurrence")]
    NoOccurrence,
    /// The user does not organize the stored event.
    #[error("the user does not organize this event")]
    NotOrganizer,
    /// The email is not a `COUNTER` for the stored event.
    #[error("not a proposal for this event")]
    NotACounter,
    /// Who proposed is not invited to the stored event.
    #[error("the proposal is not from one of the event's attendees")]
    NotAGuest,
}

/// A proposed time, seconds since the epoch (UTC), `start <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proposal {
    start: i64,
    end: i64,
}

impl Proposal {
    /// `None` if it ends before it starts.
    #[must_use]
    pub const fn new(start: i64, end: i64) -> Option<Self> {
        if end < start {
            None
        } else {
            Some(Self { start, end })
        }
    }
}

/// Which event a `COUNTER` is about.
#[derive(Debug, Clone, Copy)]
pub enum CounterSource<'a> {
    /// A stored event; with `Some`, one occurrence of its series.
    Stored {
        /// Its iCalendar text.
        text: &'a [u8],
        /// The occurrence (`None`: the whole event).
        occurrence: Option<RecurrenceKey>,
    },
    /// The iCalendar part of an invitation email (not in the calendar).
    Invitation(&'a [u8]),
}

/// The note as it goes out: no control characters but line breaks,
/// trimmed, at most [`MAX_COMMENT`] characters; `None` if empty.
#[must_use]
pub fn clean_comment(comment: Option<&str>) -> Option<String> {
    let text: String = comment?
        .replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    let text: String = text.trim().chars().take(MAX_COMMENT).collect();
    let text = text.trim_end().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The `RECURRENCE-ID` of the occurrence `key` of a series without an
/// override for it: its `DTSTART`'s form (`TZID`, `VALUE=DATE`, UTC).
fn recurrence_from(main: &ICalendarComponent, key: RecurrenceKey) -> Option<ICalendarEntry> {
    let start = main.property(&ICalendarProperty::Dtstart)?;
    let dt = start.values.first()?.as_partial_date_time()?;
    let mut value = PartialDateTime::from_naive_timestamp(key.naive());
    if !dt.has_time() {
        value.hour = None;
        value.minute = None;
        value.second = None;
    }
    value.tz_hour = dt.tz_hour;
    value.tz_minute = dt.tz_minute;
    value.tz_minus = dt.tz_minus;
    Some(ICalendarEntry {
        name: ICalendarProperty::RecurrenceId,
        params: start.params.clone(),
        values: std::iter::once(ICalendarValue::PartialDateTime(value)).collect(),
    })
}

/// The first `property` of `comp`, else of `main`.
fn either<'a>(
    comp: &'a ICalendarComponent,
    main: &'a ICalendarComponent,
    property: &ICalendarProperty,
) -> Option<&'a ICalendarEntry> {
    comp.property(property).or_else(|| main.property(property))
}

/// The `ATTENDEE` of `comp` (else of `main`) with an address `matches`.
fn attendee<'a>(
    comp: &'a ICalendarComponent,
    main: &'a ICalendarComponent,
    matches: impl Fn(&str) -> bool,
) -> Option<&'a ICalendarEntry> {
    let find = |c: &'a ICalendarComponent| {
        c.properties(&ICalendarProperty::Attendee)
            .find(|e| e.calendar_address().is_some_and(&matches))
    };
    find(comp).or_else(|| find(main))
}

fn party(entry: &ICalendarEntry) -> Option<Party> {
    Some(Party {
        address: entry.calendar_address()?.to_owned(),
        name: entry
            .parameter(&ICalendarParameterName::Cn)
            .and_then(ICalendarParameterValue::as_text)
            .map(str::to_owned),
    })
}

fn utc(seconds: i64) -> ICalendarValue {
    ICalendarValue::PartialDateTime(PartialDateTime::from_utc_timestamp(seconds))
}

/// The start of an iTIP component: `UID`, `DTSTAMP`, `SEQUENCE` and the
/// `RECURRENCE-ID`, if any.
fn head(
    uid: &ICalendarEntry,
    seq: i64,
    rid: Option<ICalendarEntry>,
    now: i64,
) -> ICalendarComponent {
    let mut out = ICalendarComponent::new(ICalendarComponentType::VEvent);
    out.entries.push(uid.clone());
    out.add_dtstamp(PartialDateTime::from_utc_timestamp(now));
    out.add_sequence(seq);
    if let Some(rid) = rid {
        out.entries.push(rid);
    }
    out
}

fn add_comment(out: &mut ICalendarComponent, comment: Option<&String>) {
    if let Some(comment) = comment {
        out.add_property(
            ICalendarProperty::Comment,
            ICalendarValue::Text(comment.clone()),
        );
    }
}

/// The component a `COUNTER` is about, the main one and its
/// `RECURRENCE-ID`.
type Target<'a> = (
    &'a ICalendarComponent,
    &'a ICalendarComponent,
    Option<ICalendarEntry>,
);

/// What a `COUNTER` is about: the whole event, or one occurrence that
/// exists (not excluded, within the series): its override if it has one,
/// found by the occurrence key (the wall clock in the event's zone, so a
/// `RECURRENCE-ID` in UTC or in another zone matches too), else the series
/// with a `RECURRENCE-ID` in its `DTSTART`'s form.
fn counter_target(
    parsed: &ParsedCalendar,
    occurrence: Option<RecurrenceKey>,
) -> Result<Target<'_>, CounterError> {
    let ical = parsed.icalendar();
    let ids = events(ical);
    let main = ids
        .first()
        .and_then(|i| component(ical, *i))
        .ok_or(CounterError::NotInvited)?;
    let Some(key) = occurrence else {
        return Ok((main, main, recurrence_id(main).cloned()));
    };
    if !parsed.is_series() {
        return Err(CounterError::NoOccurrence);
    }
    let instances = parsed.instances(Zone::UTC)?;
    let instance = instances
        .list
        .iter()
        .find(|instance| instance.key == key)
        .ok_or(CounterError::NoOccurrence)?;
    let found = usize::try_from(instance.component())
        .ok()
        .and_then(|i| component(ical, i))
        .ok_or(CounterError::NoOccurrence)?;
    if let Some(rid) = recurrence_id(found) {
        return Ok((found, main, Some(rid.clone())));
    }
    let rid = recurrence_from(main, key).ok_or(CounterError::NoOccurrence)?;
    Ok((main, main, Some(rid)))
}

/// The `COUNTER` of the user (`own`) proposing `proposal` for `source`,
/// with an optional note. Sent from the user's address to the organizer.
///
/// # Errors
/// [`CounterError`]: unreadable text, the user is not an attendee, no
/// such occurrence.
pub fn counter(
    source: CounterSource<'_>,
    own: &[String],
    proposal: Proposal,
    comment: Option<&str>,
    now: i64,
) -> Result<ItipMessage, CounterError> {
    let (text, occurrence) = match source {
        CounterSource::Stored { text, occurrence } => (text, occurrence),
        CounterSource::Invitation(text) => (text, None),
    };
    let parsed = ParsedCalendar::parse(text)?;
    let ical = parsed.icalendar();
    let (comp, main, rid) = counter_target(&parsed, occurrence)?;
    let organizer = either(comp, main, &ICalendarProperty::Organizer)
        .filter(|e| e.calendar_address().is_some_and(|a| !is_own(own, a)))
        .ok_or(CounterError::NotInvited)?;
    let me = attendee(comp, main, |a| is_own(own, a)).ok_or(CounterError::NotInvited)?;
    let (Some(to), Some(from)) = (party(organizer), party(me)) else {
        return Err(CounterError::NotInvited);
    };
    let uid = either(comp, main, &ICalendarProperty::Uid).ok_or(CounterError::NotInvited)?;
    let comment = clean_comment(comment);
    let mut out = head(uid, sequence(comp), rid, now);
    out.entries.push(without_schedule_params(organizer));
    out.entries.push(without_schedule_params(me));
    if let Some(summary) = either(comp, main, &ICalendarProperty::Summary) {
        out.entries.push(summary.clone());
    }
    out.add_property(ICalendarProperty::Dtstart, utc(proposal.start));
    out.add_property(ICalendarProperty::Dtend, utc(proposal.end));
    add_comment(&mut out, comment.as_ref());
    let title = text_of(comp, &ICalendarProperty::Summary)
        .or_else(|| text_of(main, &ICalendarProperty::Summary))
        .unwrap_or_default();
    let when = when(&out);
    let ics = message(ical, Method::Counter, vec![out])?;
    Ok(ItipMessage {
        subject: Subject::Event,
        method: Method::Counter,
        organizer: from.address,
        to: vec![to],
        ics,
        title,
        when,
        comment,
    })
}

/// Whether `ical` says `METHOD:COUNTER`.
fn is_counter(ical: &ICalendar) -> bool {
    ical.components
        .iter()
        .filter(|c| c.component_type == ICalendarComponentType::VCalendar)
        .filter_map(|c| c.property(&ICalendarProperty::Method))
        .flat_map(|e| e.values.iter())
        .any(|v| matches!(v, ICalendarValue::Method(ICalendarMethod::Counter)))
}

/// Whether two `RECURRENCE-ID`s name the same occurrence: the same wall
/// clock in `zone` (the event's), each read in its own `TZID` (a UTC one
/// and a `TZID` one of the same instant match).
fn same_occurrence(a: &ICalendarEntry, b: &ICalendarEntry, zone: Tz) -> bool {
    let a = rid_naive(a, zone);
    a.is_some() && a == rid_naive(b, zone)
}

/// The organizer's `DECLINECOUNTER` to whoever sent `counter` (the
/// `COUNTER` text of their email) about the stored event `stored`.
///
/// # Errors
/// [`CounterError`]: unreadable text, the user does not organize the
/// event, the email is not a `COUNTER` for it, or its author is not one
/// of the event's attendees.
pub fn decline_counter(
    stored: &[u8],
    counter: &[u8],
    own: &[String],
    comment: Option<&str>,
    now: i64,
) -> Result<ItipMessage, CounterError> {
    if role(stored, own)? != Role::Organizer {
        return Err(CounterError::NotOrganizer);
    }
    let proposal = parse(counter)?;
    if !is_counter(&proposal) {
        return Err(CounterError::NotACounter);
    }
    let proposed = events(&proposal)
        .first()
        .and_then(|i| component(&proposal, *i))
        .cloned()
        .ok_or(CounterError::NotACounter)?;
    let ical = parse(stored)?;
    let ids = events(&ical);
    let main = ids
        .first()
        .and_then(|i| component(&ical, *i))
        .ok_or(CounterError::NotOrganizer)?;
    let uid = main
        .property(&ICalendarProperty::Uid)
        .ok_or(CounterError::NotACounter)?;
    if text_of(main, &ICalendarProperty::Uid) != text_of(&proposed, &ICalendarProperty::Uid) {
        return Err(CounterError::NotACounter);
    }
    let rid = recurrence_id(&proposed).cloned();
    let zone = component_zone(main).unwrap_or(Tz::UTC);
    let comp = rid
        .as_ref()
        .and_then(|rid| {
            ids.iter()
                .skip(1)
                .filter_map(|i| component(&ical, *i))
                .find(|c| recurrence_id(c).is_some_and(|r| same_occurrence(r, rid, zone)))
        })
        .unwrap_or(main);
    let organizer_address = organizer_of(main).unwrap_or_default();
    let organizer = either(comp, main, &ICalendarProperty::Organizer)
        .cloned()
        .ok_or(CounterError::NotOrganizer)?;
    // The proposer: an attendee of the email who is a guest of the event
    // (never the organizer or the user).
    let guest = proposed
        .properties(&ICalendarProperty::Attendee)
        .filter_map(ICalendarEntry::calendar_address)
        .filter(|a| !is_own(own, a) && !a.eq_ignore_ascii_case(&organizer_address))
        .find_map(|a| {
            ids.iter()
                .filter_map(|i| component(&ical, *i))
                .flat_map(|c| c.properties(&ICalendarProperty::Attendee))
                .find(|e| {
                    e.calendar_address()
                        .is_some_and(|b| b.eq_ignore_ascii_case(a))
                })
        })
        .ok_or(CounterError::NotAGuest)?;
    let to = party(guest).ok_or(CounterError::NotAGuest)?;
    let comment = clean_comment(comment);
    let mut out = head(uid, sequence(comp), rid, now);
    out.entries.push(without_schedule_params(&organizer));
    out.entries.push(without_schedule_params(guest));
    add_comment(&mut out, comment.as_ref());
    let title = text_of(comp, &ICalendarProperty::Summary)
        .or_else(|| text_of(main, &ICalendarProperty::Summary))
        .unwrap_or_default();
    let ics = message(&ical, Method::DeclineCounter, vec![out])?;
    Ok(ItipMessage {
        subject: Subject::Event,
        method: Method::DeclineCounter,
        organizer: organizer_address,
        to: vec![to],
        ics,
        title,
        when: None,
        comment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_792_483_200; // 2026-10-20T08:00:00Z

    fn bob() -> Vec<String> {
        vec!["bob@example.com".to_owned()]
    }

    fn alice() -> Vec<String> {
        vec!["alice@example.com".to_owned()]
    }

    fn series(extra: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
             UID:s@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:3\r\n\
             DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDURATION:PT1H\r\n\
             RRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Weekly\r\n\
             ORGANIZER;CN=Alice;SCHEDULE-AGENT=CLIENT:mailto:alice@example.com\r\n\
             ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.com\r\n\
             ATTENDEE;CN=Bob;PARTSTAT=TENTATIVE;SCHEDULE-AGENT=CLIENT:mailto:bob@example.com\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n\
             END:VEVENT\r\n{extra}END:VCALENDAR\r\n"
        )
    }

    fn unfold(text: &str) -> String {
        text.replace("\r\n ", "")
    }

    fn lines(text: &str) -> Vec<String> {
        unfold(text).split("\r\n").map(str::to_owned).collect()
    }

    fn proposal() -> Proposal {
        Proposal::new(START, START + 3600).unwrap()
    }

    #[test]
    fn a_proposal_for_the_whole_event() {
        let text = series("");
        let message = counter(
            CounterSource::Stored {
                text: text.as_bytes(),
                occurrence: None,
            },
            &bob(),
            proposal(),
            Some("  Better\r\nafter lunch\u{7}  "),
            0,
        )
        .unwrap();
        assert_eq!(message.method, Method::Counter);
        assert_eq!(message.organizer, "bob@example.com");
        assert_eq!(message.to.len(), 1);
        assert_eq!(message.to[0].address, "alice@example.com");
        assert_eq!(message.to[0].name.as_deref(), Some("Alice"));
        assert_eq!(message.title, "Weekly");
        assert_eq!(message.when.as_deref(), Some("2026-10-20 08:00 (UTC)"));
        assert_eq!(message.comment.as_deref(), Some("Better\nafter lunch"));
        let ics = lines(&message.ics);
        for line in [
            "METHOD:COUNTER",
            "UID:s@example.com",
            "SEQUENCE:3",
            "DTSTART:20261020T080000Z",
            "DTEND:20261020T090000Z",
            "SUMMARY:Weekly",
            "COMMENT:Better\\nafter lunch",
        ] {
            assert!(ics.iter().any(|l| l == line), "{line} in {ics:#?}");
        }
        let text = unfold(&message.ics);
        // Only Bob, with his answer so far; no scheduling parameters, no
        // alarms, no rule.
        assert_eq!(text.matches("ATTENDEE").count(), 1, "{text}");
        assert!(text.contains("PARTSTAT=TENTATIVE"), "{text}");
        assert!(text.contains("mailto:bob@example.com"), "{text}");
        assert!(!text.contains("SCHEDULE-"), "{text}");
        assert!(
            !text.contains("VALARM") && !text.contains("RRULE"),
            "{text}"
        );
        assert!(!text.contains("RECURRENCE-ID"), "{text}");
    }

    /// One occurrence: its RECURRENCE-ID in the series' form, from the
    /// key (no override yet) or from its override (with its SEQUENCE).
    #[test]
    fn a_proposal_for_one_occurrence() {
        let text = series("");
        // 2026-10-26 10:00 in Madrid, as a wall-clock key.
        let key = RecurrenceKey::from_naive(1_793_008_800).unwrap();
        let message = counter(
            CounterSource::Stored {
                text: text.as_bytes(),
                occurrence: Some(key),
            },
            &bob(),
            proposal(),
            None,
            0,
        )
        .unwrap();
        let ics = unfold(&message.ics);
        assert!(
            ics.contains("RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000"),
            "{ics}"
        );
        assert!(ics.contains("BEGIN:VTIMEZONE"), "{ics}");
        assert!(!ics.contains("COMMENT"), "{ics}");
        assert!(message.comment.is_none());
        let with_override = series(
            "BEGIN:VEVENT\r\nUID:s@example.com\r\nSEQUENCE:5\r\n\
             RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\nSUMMARY:Moved\r\n\
             ORGANIZER:mailto:alice@example.com\r\n\
             ATTENDEE;PARTSTAT=DECLINED:mailto:bob@example.com\r\nEND:VEVENT\r\n",
        );
        let message = counter(
            CounterSource::Stored {
                text: with_override.as_bytes(),
                occurrence: Some(key),
            },
            &bob(),
            proposal(),
            None,
            0,
        )
        .unwrap();
        let ics = unfold(&message.ics);
        assert!(
            ics.contains("SEQUENCE:5") && ics.contains("SUMMARY:Moved"),
            "{ics}"
        );
        assert!(ics.contains("PARTSTAT=DECLINED"), "{ics}");
        assert_eq!(message.title, "Moved");
    }

    /// An all-day series: RECURRENCE-ID is a DATE.
    #[test]
    fn all_day_occurrence() {
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
                    UID:d@example.com\r\nDTSTAMP:20261001T000000Z\r\n\
                    DTSTART;VALUE=DATE:20261019\r\nDTEND;VALUE=DATE:20261020\r\n\
                    RRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Trip\r\n\
                    ORGANIZER:mailto:alice@example.com\r\n\
                    ATTENDEE:mailto:bob@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let key = RecurrenceKey::from_naive(1_792_540_800).unwrap(); // 2026-10-21
        let message = counter(
            CounterSource::Stored {
                text: text.as_bytes(),
                occurrence: Some(key),
            },
            &bob(),
            proposal(),
            None,
            0,
        )
        .unwrap();
        let ics = unfold(&message.ics);
        assert!(ics.contains("RECURRENCE-ID;VALUE=DATE:20261021"), "{ics}");
        assert!(ics.contains("SEQUENCE:0"), "{ics}");
    }

    /// From an invitation email (maybe only one occurrence of a series).
    #[test]
    fn from_an_invitation() {
        let invitation = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nMETHOD:REQUEST\r\n\
             BEGIN:VEVENT\r\nUID:i@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:2\r\n\
             RECURRENCE-ID:20261026T090000Z\r\nDTSTART:20261026T090000Z\r\nDURATION:PT1H\r\n\
             SUMMARY:Review\r\nORGANIZER;CN=Carol:mailto:carol@example.net\r\n\
             ATTENDEE;CN=Bob:mailto:BOB@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let message = counter(
            CounterSource::Invitation(invitation.as_bytes()),
            &bob(),
            proposal(),
            Some("Can't make it"),
            0,
        )
        .unwrap();
        assert_eq!(message.to[0].address, "carol@example.net");
        assert_eq!(message.organizer, "BOB@example.com");
        let ics = unfold(&message.ics);
        assert!(ics.contains("RECURRENCE-ID:20261026T090000Z"), "{ics}");
        assert!(ics.contains("SEQUENCE:2") && ics.contains("UID:i@example.com"));
    }

    #[test]
    fn refused() {
        let text = series("");
        let stored = |occurrence| CounterSource::Stored {
            text: text.as_bytes(),
            occurrence,
        };
        // The organizer can't propose to herself; strangers can't either.
        assert_eq!(
            counter(stored(None), &alice(), proposal(), None, 0).unwrap_err(),
            CounterError::NotInvited
        );
        let carol = vec!["carol@example.com".to_owned()];
        assert_eq!(
            counter(stored(None), &carol, proposal(), None, 0).unwrap_err(),
            CounterError::NotInvited
        );
        assert!(matches!(
            counter(
                CounterSource::Invitation(b"nope"),
                &bob(),
                proposal(),
                None,
                0
            ),
            Err(CounterError::Convert(_))
        ));
        assert!(Proposal::new(10, 5).is_none());
        assert!(Proposal::new(5, 5).is_some());
    }

    fn line_of(text: &str, prefix: &str) -> String {
        let found = text.split("\r\n").find(|l| l.starts_with(prefix)).unwrap();
        format!("{found}\r\n")
    }

    fn propose(text: &str, key: Option<RecurrenceKey>) -> Result<ItipMessage, CounterError> {
        counter(
            CounterSource::Stored {
                text: text.as_bytes(),
                occurrence: key,
            },
            &bob(),
            proposal(),
            None,
            0,
        )
    }

    /// 2026-10-26 10:00 in Madrid, as a wall-clock key.
    const OCT_26: i64 = 1_793_008_800;

    /// A key that names no occurrence is refused, never made up.
    #[test]
    fn no_such_occurrence() {
        let key = RecurrenceKey::from_naive(OCT_26);
        // Not a series.
        let single = series("").replace("RRULE:FREQ=WEEKLY;COUNT=4\r\n", "");
        assert_eq!(
            propose(&single, key).unwrap_err(),
            CounterError::NoOccurrence
        );
        assert!(propose(&single, None).is_ok());
        // Excluded.
        let excluded = series("").replace(
            "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        assert_eq!(
            propose(&excluded, key).unwrap_err(),
            CounterError::NoOccurrence
        );
        // Not on the rule (an hour off), or past its end.
        for off in [OCT_26 + 3600, OCT_26 + 4 * 7 * 86_400] {
            assert_eq!(
                propose(&series(""), RecurrenceKey::from_naive(off)).unwrap_err(),
                CounterError::NoOccurrence
            );
        }
    }

    /// An override whose RECURRENCE-ID is in UTC: found by the instant in
    /// the event's zone, and the user invited only to it can propose.
    #[test]
    fn an_override_named_in_utc() {
        let base = series("");
        let bob_line = line_of(&base, "ATTENDEE;CN=Bob");
        let without_bob = base.replacen(&bob_line, "", 1);
        let text = format!(
            "{}BEGIN:VEVENT\r\n{}SEQUENCE:7\r\nRECURRENCE-ID:20261026T090000Z\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\nSUMMARY:Moved\r\n\
             {}{bob_line}END:VEVENT\r\nEND:VCALENDAR\r\n",
            without_bob.trim_end_matches("END:VCALENDAR\r\n"),
            line_of(&base, "UID:"),
            line_of(&base, "ORGANIZER"),
        );
        let message = propose(&text, RecurrenceKey::from_naive(OCT_26)).unwrap();
        let ics = unfold(&message.ics);
        assert!(ics.contains("RECURRENCE-ID:20261026T090000Z"), "{ics}");
        assert!(
            ics.contains("SEQUENCE:7") && ics.contains("SUMMARY:Moved"),
            "{ics}"
        );
        assert_eq!(message.title, "Moved");
        // Declining a proposal that names it with the TZID form.
        let email = proposal_email(
            &bob()[0],
            "RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        let message =
            decline_counter(text.as_bytes(), email.as_bytes(), &alice(), None, 0).unwrap();
        let ics = unfold(&message.ics);
        assert!(ics.contains("SEQUENCE:7"), "{ics}");
        assert_eq!(message.title, "Moved");
    }

    fn proposal_email(from: &str, rid: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//EN\r\nMETHOD:COUNTER\r\n\
             BEGIN:VEVENT\r\nUID:s@example.com\r\nDTSTAMP:20261002T000000Z\r\nSEQUENCE:3\r\n{rid}\
             ORGANIZER:mailto:alice@example.com\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:{from}\r\n\
             DTSTART:20261020T080000Z\r\nDTEND:20261020T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    #[test]
    fn declining_a_proposal() {
        let stored = series("");
        let email = proposal_email("bob@example.com", "");
        let message =
            decline_counter(stored.as_bytes(), email.as_bytes(), &alice(), Some("No"), 0).unwrap();
        assert_eq!(message.method, Method::DeclineCounter);
        assert_eq!(message.organizer, "alice@example.com");
        assert_eq!(message.to[0].address, "bob@example.com");
        assert_eq!(message.to[0].name.as_deref(), Some("Bob"));
        assert_eq!(message.title, "Weekly");
        let ics = unfold(&message.ics);
        assert!(ics.contains("METHOD:DECLINECOUNTER"), "{ics}");
        assert!(
            ics.contains("SEQUENCE:3") && ics.contains("COMMENT:No"),
            "{ics}"
        );
        assert_eq!(ics.matches("ATTENDEE").count(), 1, "{ics}");
        assert!(
            !ics.contains("SCHEDULE-") && !ics.contains("DTSTART:"),
            "{ics}"
        );
        // One occurrence: its RECURRENCE-ID, the override's SEQUENCE.
        let with_override = series(
            "BEGIN:VEVENT\r\nUID:s@example.com\r\nSEQUENCE:6\r\n\
             RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\n\
             ORGANIZER:mailto:alice@example.com\r\n\
             ATTENDEE:mailto:bob@example.com\r\nEND:VEVENT\r\n",
        );
        let email = proposal_email(
            "bob@example.com",
            "RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        let message = decline_counter(
            with_override.as_bytes(),
            email.as_bytes(),
            &alice(),
            None,
            0,
        )
        .unwrap();
        let ics = unfold(&message.ics);
        assert!(
            ics.contains("RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000"),
            "{ics}"
        );
        assert!(ics.contains("SEQUENCE:6"), "{ics}");
    }

    #[test]
    fn declining_refused() {
        let stored = series("");
        let email = proposal_email("bob@example.com", "");
        // Not her event.
        assert_eq!(
            decline_counter(stored.as_bytes(), email.as_bytes(), &bob(), None, 0).unwrap_err(),
            CounterError::NotOrganizer
        );
        // Not a COUNTER.
        let request = email.replace("METHOD:COUNTER", "METHOD:REQUEST");
        assert_eq!(
            decline_counter(stored.as_bytes(), request.as_bytes(), &alice(), None, 0).unwrap_err(),
            CounterError::NotACounter
        );
        // Another event.
        let other = email.replace("UID:s@example.com", "UID:other@example.com");
        assert_eq!(
            decline_counter(stored.as_bytes(), other.as_bytes(), &alice(), None, 0).unwrap_err(),
            CounterError::NotACounter
        );
        // Someone who isn't a guest.
        let stranger = proposal_email("mallory@example.org", "");
        assert_eq!(
            decline_counter(stored.as_bytes(), stranger.as_bytes(), &alice(), None, 0).unwrap_err(),
            CounterError::NotAGuest
        );
    }

    #[test]
    fn comments() {
        assert_eq!(clean_comment(None), None);
        assert_eq!(clean_comment(Some(" \r\n\t ")), None);
        assert_eq!(
            clean_comment(Some("a\rb\r\nc\u{0}")).as_deref(),
            Some("a\nb\nc")
        );
        assert_eq!(
            clean_comment(Some(&"é".repeat(3000))).map(|c| c.chars().count()),
            Some(MAX_COMMENT)
        );
    }
}
