// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A calendar's defaults (4.5): its time zone and its
//! default alerts, as CalDAV keeps them.
//!
//! - **Time zone:** `calendar-timezone-id` (RFC 7809) first, else the
//!   `TZID` of `calendar-timezone` (RFC 4791 §5.2.2); only a zone calcard
//!   knows, by its IANA name ([`calendar_zone`]). Written as both
//!   ([`vtimezone_calendar`] makes the `VCALENDAR`).
//! - **Default alerts:** `default-alarm-vevent-datetime` and `-date`
//!   (draft-daboo-valarm-extensions §9: "one or more `VALARM` components
//!   or the empty string"). Like Stalwart's JMAP, only alerts relative to
//!   the start or the end, shown or mailed ([`DefaultAlert`]); anything
//!   else in the server's value is skipped. The server's text is parsed
//!   strictly ([`DefaultAlerts::from_valarms`]): size and count limits,
//!   only `VALARM` components (any other `BEGIN`/`END` and the value is
//!   ignored), through calcard inside `catch_unwind`. What the bridge
//!   writes and stores is always its own canonical form
//!   ([`DefaultAlerts::to_valarms`]).

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use calcard::icalendar::{
    ICalendar, ICalendarAction, ICalendarComponentType, ICalendarDuration, ICalendarParameterName,
    ICalendarParameterValue, ICalendarProperty, ICalendarRelated, ICalendarValue,
};
use serde_json::{Map, Value};

use super::calendar::Zone;
use super::convert::{Format, checked_text, guarded, timezone_id, write_text};

/// Most default alerts kept per kind (with time, without time).
pub const MAX_DEFAULT_ALERTS: usize = 32;
/// Longest `default-alarm-vevent-*` text read (as `dav-proto` keeps).
pub const MAX_VALARMS_BYTES: usize = 64 * 1024;
/// Longest alert id (a JMAP `Id`).
const MAX_ID: usize = 255;
/// Largest number in a duration we accept (100 years of seconds).
const MAX_DURATION_PART: u32 = 3_155_760_000;

/// What a default alert does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AlertAction {
    /// Shown (`display`; `AUDIO` alarms read as this, as calcard does).
    Display,
    /// Mailed (`email`).
    Email,
}

/// Where the offset of a default alert counts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelativeTo {
    /// The event's start.
    Start,
    /// The event's end.
    End,
}

/// A signed RFC 8984 / RFC 5545 duration with whole seconds: weeks, or
/// days and a time. Invariant: `weeks > 0` only with `days == 0` and
/// `seconds == 0` (what [`Self::parse`] reads and `Display` writes; a
/// server's `P1W2D` is kept as `P9D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SignedDuration {
    negative: bool,
    weeks: u32,
    days: u32,
    seconds: u32,
}

impl SignedDuration {
    /// Parses `[+-]P…` (`-PT15M`, `P1D`, `-P1DT2H`, `P2W`). Fractions,
    /// empty durations (`P`, `PT`) and parts too big are refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = match text.as_bytes().first()? {
            b'-' => (true, text.get(1..)?),
            b'+' => (false, text.get(1..)?),
            _ => (false, text),
        };
        let rest = rest.strip_prefix('P')?;
        let (date, time) = match rest.split_once('T') {
            Some((date, time)) => (date, Some(time)),
            None => (rest, None),
        };
        let mut out = Self {
            negative,
            ..Self::default()
        };
        let mut any = false;
        if let Some(weeks) = date.strip_suffix('W') {
            if time.is_some() {
                return None;
            }
            out.weeks = number(weeks)?;
            return Some(out);
        }
        if !date.is_empty() {
            out.days = number(date.strip_suffix('D')?)?;
            any = true;
        }
        if let Some(mut time) = time {
            if time.is_empty() {
                return None;
            }
            let mut seconds: u32 = 0;
            for (unit, factor) in [('H', 3600_u32), ('M', 60), ('S', 1)] {
                if let Some((value, after)) = time.split_once(unit)
                    && !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit())
                {
                    let part = number(value)?.checked_mul(factor)?;
                    seconds = seconds.checked_add(part)?;
                    time = after;
                    any = true;
                }
            }
            if !time.is_empty() || seconds > MAX_DURATION_PART {
                return None;
            }
            out.seconds = seconds;
        }
        any.then_some(out)
    }

    fn from_ical(duration: &ICalendarDuration) -> Option<Self> {
        let seconds = duration
            .hours
            .checked_mul(3600)?
            .checked_add(duration.minutes.checked_mul(60)?)?
            .checked_add(duration.seconds)?;
        // Weeks with days or a time (`P1W2D`, which RFC 5545 does not
        // allow but servers write): all of it as days, so nothing is lost
        // when it is written back.
        let (weeks, days) = if duration.weeks > 0 && (duration.days > 0 || seconds > 0) {
            (
                0,
                duration.weeks.checked_mul(7)?.checked_add(duration.days)?,
            )
        } else {
            (duration.weeks, duration.days)
        };
        (weeks <= MAX_DURATION_PART && days <= MAX_DURATION_PART && seconds <= MAX_DURATION_PART)
            .then_some(Self {
                negative: duration.neg,
                weeks,
                days,
                seconds,
            })
    }
}

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|n| *n <= MAX_DURATION_PART)
}

/// The canonical form: `-P1DT2H30M`, `P2W`, `PT0S`.
impl fmt::Display for SignedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_char('-')?;
        }
        f.write_char('P')?;
        if self.weeks > 0 {
            return write!(f, "{}W", self.weeks);
        }
        if self.days > 0 {
            write!(f, "{}D", self.days)?;
        }
        if self.seconds > 0 || self.days == 0 {
            f.write_char('T')?;
            let (hours, rest) = (self.seconds / 3600, self.seconds % 3600);
            let (minutes, seconds) = (rest / 60, rest % 60);
            if hours > 0 {
                write!(f, "{hours}H")?;
            }
            if minutes > 0 {
                write!(f, "{minutes}M")?;
            }
            if seconds > 0 || self.seconds == 0 {
                write!(f, "{seconds}S")?;
            }
        }
        Ok(())
    }
}

/// One default alert: an `OffsetTrigger` and its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultAlert {
    /// How long before (negative) or after the start or end.
    pub offset: SignedDuration,
    /// Start or end.
    pub relative_to: RelativeTo,
    /// Shown or mailed.
    pub action: AlertAction,
}

/// Why a JMAP default alert was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DefaultAlertError {
    /// The id is not a JMAP id.
    #[error("an alert id must be 1 to 255 letters, digits, '-' or '_'")]
    Id,
    /// Not an `Alert` with an `OffsetTrigger` (as Stalwart, the only kind
    /// a default alert can be).
    #[error("a default alert needs an OffsetTrigger with an offset")]
    Trigger,
    /// An action other than `display` or `email`.
    #[error("a default alert's action must be display or email")]
    Action,
    /// A member that is not `@type`, `trigger`, `action`.
    #[error("a default alert has only @type, trigger and action")]
    Member,
    /// More than [`MAX_DEFAULT_ALERTS`].
    #[error("too many default alerts")]
    TooMany,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl DefaultAlert {
    /// Reads a JMAP `Alert` (draft-ietf-jmap-calendars §4.1.1,
    /// `defaultAlertsWithTime` values).
    ///
    /// # Errors
    /// [`DefaultAlertError`] for anything but the shape Stalwart accepts.
    pub fn from_jmap(alert: &Map<String, Value>) -> Result<Self, DefaultAlertError> {
        let mut action = AlertAction::Display;
        let mut trigger = None;
        for (key, value) in alert {
            match key.as_str() {
                "@type" if value == "Alert" => {}
                "action" => {
                    action = match value.as_str() {
                        Some("display") => AlertAction::Display,
                        Some("email") => AlertAction::Email,
                        _ => return Err(DefaultAlertError::Action),
                    };
                }
                "trigger" => trigger = Some(value.as_object().ok_or(DefaultAlertError::Trigger)?),
                _ => return Err(DefaultAlertError::Member),
            }
        }
        let trigger = trigger.ok_or(DefaultAlertError::Trigger)?;
        let mut offset = None;
        let mut relative_to = RelativeTo::Start;
        for (key, value) in trigger {
            match key.as_str() {
                "@type" if value == "OffsetTrigger" => {}
                "offset" => offset = value.as_str().and_then(SignedDuration::parse),
                "relativeTo" => {
                    relative_to = match value.as_str() {
                        Some("start") => RelativeTo::Start,
                        Some("end") => RelativeTo::End,
                        _ => return Err(DefaultAlertError::Trigger),
                    };
                }
                _ => return Err(DefaultAlertError::Trigger),
            }
        }
        Ok(Self {
            offset: offset.ok_or(DefaultAlertError::Trigger)?,
            relative_to,
            action,
        })
    }

    /// The JMAP `Alert` (every member, as Stalwart writes it).
    #[must_use]
    pub fn to_jmap(&self) -> Map<String, Value> {
        let mut trigger = Map::new();
        trigger.insert("@type".into(), "OffsetTrigger".into());
        trigger.insert("offset".into(), self.offset.to_string().into());
        trigger.insert(
            "relativeTo".into(),
            match self.relative_to {
                RelativeTo::Start => "start",
                RelativeTo::End => "end",
            }
            .into(),
        );
        let mut alert = Map::new();
        alert.insert("@type".into(), "Alert".into());
        alert.insert(
            "action".into(),
            match self.action {
                AlertAction::Display => "display",
                AlertAction::Email => "email",
            }
            .into(),
        );
        alert.insert("trigger".into(), Value::Object(trigger));
        alert
    }

    fn write_valarm(&self, id: &str, out: &mut String) {
        let _ = write!(out, "BEGIN:VALARM\r\nUID:{id}\r\n");
        out.push_str(match self.action {
            AlertAction::Display => "ACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\n",
            AlertAction::Email => "ACTION:EMAIL\r\nSUMMARY:Reminder\r\nDESCRIPTION:Reminder\r\n",
        });
        let related = match self.relative_to {
            RelativeTo::Start => "",
            RelativeTo::End => ";RELATED=END",
        };
        let _ = write!(out, "TRIGGER{related}:{}\r\nEND:VALARM\r\n", self.offset);
    }
}

/// The default alerts of one kind (with time or without), by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultAlerts(BTreeMap<String, DefaultAlert>);

impl DefaultAlerts {
    /// None.
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each alert by id.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &DefaultAlert)> {
        self.0.iter().map(|(id, alert)| (id.as_str(), alert))
    }

    /// Reads a JMAP `Id[Alert]` map.
    ///
    /// # Errors
    /// [`DefaultAlertError`] for a bad id, an alert of another shape, or
    /// too many.
    pub fn from_jmap(map: &Map<String, Value>) -> Result<Self, DefaultAlertError> {
        if map.len() > MAX_DEFAULT_ALERTS {
            return Err(DefaultAlertError::TooMany);
        }
        let mut out = BTreeMap::new();
        for (id, alert) in map {
            if !valid_id(id) {
                return Err(DefaultAlertError::Id);
            }
            let alert = alert.as_object().ok_or(DefaultAlertError::Trigger)?;
            out.insert(id.clone(), DefaultAlert::from_jmap(alert)?);
        }
        Ok(Self(out))
    }

    /// Sets or removes (`None`) the alert `id` (a `defaultAlerts*/<id>`
    /// patch).
    ///
    /// # Errors
    /// As [`Self::from_jmap`].
    pub fn patch(&mut self, id: &str, alert: Option<&Value>) -> Result<(), DefaultAlertError> {
        if !valid_id(id) {
            return Err(DefaultAlertError::Id);
        }
        match alert {
            Some(value) => {
                let alert = value.as_object().ok_or(DefaultAlertError::Trigger)?;
                self.0
                    .insert(id.to_owned(), DefaultAlert::from_jmap(alert)?);
                if self.0.len() > MAX_DEFAULT_ALERTS {
                    return Err(DefaultAlertError::TooMany);
                }
            }
            None => {
                self.0.remove(id);
            }
        }
        Ok(())
    }

    /// The JMAP `Id[Alert]` map.
    #[must_use]
    pub fn to_jmap(&self) -> Map<String, Value> {
        self.0
            .iter()
            .map(|(id, alert)| (id.clone(), Value::Object(alert.to_jmap())))
            .collect()
    }

    /// The value of `default-alarm-vevent-*`: one `VALARM` per alert
    /// (with `UID` = its id, RFC 9074), or `""` for none.
    #[must_use]
    pub fn to_valarms(&self) -> String {
        let mut out = String::new();
        for (id, alert) in &self.0 {
            alert.write_valarm(id, &mut out);
        }
        out
    }

    /// Reads a server's `default-alarm-vevent-*`. Anything that is not
    /// plain `VALARM`s gives no alerts; alarms the bridge can't represent
    /// (absolute triggers, `PROCEDURE`…) are skipped. Ids: the alarm's
    /// `UID`, `X-WR-ALARMUID` (Apple) or `JSID`, else `k1`, `k2`… by
    /// position (as calcard names them).
    #[must_use]
    pub fn from_valarms(text: &str) -> Self {
        Self::parse_valarms(text).unwrap_or_default()
    }

    fn parse_valarms(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_VALARMS_BYTES || !only_valarms(text) {
            return None;
        }
        let wrapped = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//nebulamail//defaults//EN\r\n\
             BEGIN:VEVENT\r\nUID:defaults\r\nDTSTART:20000101T000000Z\r\n{text}\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let format = Format::ICalendar;
        let checked = checked_text(wrapped.as_bytes(), format).ok()?;
        guarded(format, || {
            let ical =
                ICalendar::parse(checked).map_err(|_| crate::ConvertError::Invalid { format })?;
            let mut out = BTreeMap::new();
            let alarms = ical
                .components
                .iter()
                .filter(|c| c.component_type == ICalendarComponentType::VAlarm)
                .take(MAX_DEFAULT_ALERTS);
            for (position, alarm) in alarms.enumerate() {
                let Some(read) = read_valarm(alarm) else {
                    continue;
                };
                let named = [
                    ICalendarProperty::Uid,
                    ICalendarProperty::Other("X-WR-ALARMUID".into()),
                    ICalendarProperty::Jsid,
                ]
                .iter()
                .find_map(|name| {
                    alarm
                        .property(name)
                        .and_then(|entry| entry.values.first())
                        .and_then(ICalendarValue::as_text)
                        .map(str::trim)
                        .filter(|id| valid_id(id) && !out.contains_key(*id))
                        .map(str::to_owned)
                });
                let id = named.unwrap_or_else(|| format!("k{}", position.saturating_add(1)));
                out.entry(id).or_insert(read);
            }
            Ok(Self(out))
        })
        .ok()
    }
}

/// Every `BEGIN:`/`END:` line (after unfolding) names `VALARM`: the value
/// can't close the event it is wrapped in, nor bring other components.
fn only_valarms(text: &str) -> bool {
    let unfolded = text
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    let mut depth = 0_u8;
    let mut count = 0_usize;
    for line in unfolded.lines() {
        let line = line.trim_end_matches('\r');
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_uppercase();
        let is_alarm = value.trim().eq_ignore_ascii_case("VALARM");
        match name.as_str() {
            "BEGIN" if is_alarm && depth == 0 => {
                depth = 1;
                count = count.saturating_add(1);
            }
            "END" if is_alarm && depth == 1 => depth = 0,
            "BEGIN" | "END" => return false,
            _ if depth == 0 => return false,
            _ => {}
        }
    }
    depth == 0 && count > 0
}

fn read_valarm(alarm: &calcard::icalendar::ICalendarComponent) -> Option<DefaultAlert> {
    let action = match alarm
        .property(&ICalendarProperty::Action)
        .and_then(|entry| entry.values.first())
    {
        Some(ICalendarValue::Action(ICalendarAction::Display | ICalendarAction::Audio)) => {
            AlertAction::Display
        }
        Some(ICalendarValue::Action(ICalendarAction::Email)) => AlertAction::Email,
        _ => return None,
    };
    let trigger = alarm.property(&ICalendarProperty::Trigger)?;
    let Some(ICalendarValue::Duration(duration)) = trigger.values.first() else {
        return None;
    };
    let relative_to = if trigger.params.iter().any(|p| {
        p.name == ICalendarParameterName::Related
            && p.value == ICalendarParameterValue::Related(ICalendarRelated::End)
    }) {
        RelativeTo::End
    } else {
        RelativeTo::Start
    };
    Some(DefaultAlert {
        offset: SignedDuration::from_ical(duration)?,
        relative_to,
        action,
    })
}

/// The IANA name of a zone calcard knows (also a Windows or other name it
/// maps to one); `None` otherwise.
#[must_use]
pub fn canonical_zone(name: &str) -> Option<String> {
    Zone::named(name.trim()).map(Zone::iana_name)
}

/// A calendar's zone: `calendar-timezone-id`, else the `TZID` of
/// `calendar-timezone`, by its IANA name.
#[must_use]
pub fn calendar_zone(id: Option<&str>, calendar_timezone: Option<&str>) -> Option<String> {
    id.and_then(canonical_zone).or_else(|| {
        calendar_timezone
            .and_then(timezone_id)
            .as_deref()
            .and_then(canonical_zone)
    })
}

/// First and last instants a written `VTIMEZONE` covers (2000-01-01 and
/// 2050-01-01 UTC): calcard folds the years into `RRULE`s.
const VTIMEZONE_FROM: i64 = 946_684_800;
const VTIMEZONE_TO: i64 = 2_524_608_000;

/// A `calendar-timezone` value (a `VCALENDAR` with the `VTIMEZONE` of the
/// zone, RFC 4791 §5.2.2) for a name calcard knows.
#[must_use]
pub fn vtimezone_calendar(name: &str) -> Option<String> {
    let zone = canonical_zone(name)?;
    let empty =
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//Bridge//EN\r\nEND:VCALENDAR\r\n";
    guarded(Format::ICalendar, || {
        let mut ical = ICalendar::parse(empty).map_err(|_| crate::ConvertError::Invalid {
            format: Format::ICalendar,
        })?;
        ical.add_timezone(&zone, VTIMEZONE_FROM, VTIMEZONE_TO)
            .map(|_| write_text(&ical, Format::ICalendar))
            .transpose()
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn alerts(value: &Value) -> DefaultAlerts {
        DefaultAlerts::from_jmap(value.as_object().unwrap()).unwrap()
    }

    #[test]
    fn durations() {
        for (text, canonical) in [
            ("-PT15M", "-PT15M"),
            ("PT0S", "PT0S"),
            ("P1D", "P1D"),
            ("-P1DT2H30M", "-P1DT2H30M"),
            ("+PT90M", "PT1H30M"),
            ("P2W", "P2W"),
            ("-PT1H0M5S", "-PT1H5S"),
        ] {
            assert_eq!(SignedDuration::parse(text).unwrap().to_string(), canonical);
        }
        for bad in [
            "",
            "P",
            "PT",
            "-",
            "15M",
            "PT1.5S",
            "P1W2D",
            "PT1H1H",
            "PT-1M",
            "P99999999999D",
            "PT1HX",
            "pt1h",
            "P1DT",
        ] {
            assert_eq!(SignedDuration::parse(bad), None, "{bad}");
        }
    }

    /// RFC 5545 has weeks or days, but servers write both: nothing is
    /// lost (`-P1W2D` is nine days), and it reads back the same.
    #[test]
    fn weeks_with_days_from_a_server() {
        let read = |text: &str| {
            let wrapped =
                format!("BEGIN:VALARM\r\nUID:a\r\nACTION:DISPLAY\r\nTRIGGER:{text}\r\nEND:VALARM");
            let alerts = DefaultAlerts::from_valarms(&wrapped);
            let again = DefaultAlerts::from_valarms(&alerts.to_valarms());
            assert_eq!(alerts, again, "{text}");
            alerts
                .iter()
                .next()
                .map(|(_, alert)| alert.offset.to_string())
        };
        assert_eq!(read("-P1W2D").as_deref(), Some("-P9D"));
        assert_eq!(read("P2W").as_deref(), Some("P2W"));
        assert_eq!(read("P1WT1H").as_deref(), Some("P7DT1H"));
        let huge = ICalendarDuration {
            weeks: MAX_DURATION_PART,
            days: 1,
            ..ICalendarDuration::default()
        };
        assert_eq!(SignedDuration::from_ical(&huge), None);
    }

    #[test]
    fn jmap_round_trip_and_shape() {
        let value = json!({
            "a": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}},
            "b": {"trigger": {"offset": "PT0S", "relativeTo": "end"}, "action": "email"}
        });
        let parsed = alerts(&value);
        assert_eq!(
            Value::Object(parsed.to_jmap()),
            json!({
                "a": {"@type": "Alert", "action": "display",
                      "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "start"}},
                "b": {"@type": "Alert", "action": "email",
                      "trigger": {"@type": "OffsetTrigger", "offset": "PT0S", "relativeTo": "end"}}
            })
        );
        for (bad, error) in [
            (
                json!({"a": {"trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-01T00:00:00Z"}}}),
                DefaultAlertError::Trigger,
            ),
            (
                json!({"a": {"trigger": {"offset": "-PT5M"}, "action": "sms"}}),
                DefaultAlertError::Action,
            ),
            (
                json!({"a": {"trigger": {"offset": "-PT5M"}, "acknowledged": "2026-01-01T00:00:00Z"}}),
                DefaultAlertError::Member,
            ),
            (
                json!({"a b": {"trigger": {"offset": "-PT5M"}}}),
                DefaultAlertError::Id,
            ),
            (
                json!({"a": {"@type": "Event", "trigger": {"offset": "-PT5M"}}}),
                DefaultAlertError::Member,
            ),
            (json!({"a": {}}), DefaultAlertError::Trigger),
        ] {
            assert_eq!(
                DefaultAlerts::from_jmap(bad.as_object().unwrap()).unwrap_err(),
                error,
                "{bad}"
            );
        }
        let many: Map<String, Value> = (0..=MAX_DEFAULT_ALERTS)
            .map(|n| (format!("a{n}"), json!({"trigger": {"offset": "-PT5M"}})))
            .collect();
        assert_eq!(
            DefaultAlerts::from_jmap(&many).unwrap_err(),
            DefaultAlertError::TooMany
        );
    }

    #[test]
    fn valarms_round_trip() {
        let parsed = alerts(&json!({
            "a": {"trigger": {"offset": "-PT15M"}},
            "b": {"trigger": {"offset": "P1D", "relativeTo": "end"}, "action": "email"}
        }));
        let text = parsed.to_valarms();
        assert!(text.starts_with("BEGIN:VALARM\r\nUID:a\r\nACTION:DISPLAY\r\n"));
        assert!(text.contains("TRIGGER;RELATED=END:P1D\r\n"));
        assert_eq!(DefaultAlerts::from_valarms(&text), parsed);
        assert_eq!(DefaultAlerts::new().to_valarms(), "");
        assert!(DefaultAlerts::from_valarms("").is_empty());
    }

    /// What Apple and other clients write: `X-WR-ALARMUID`, `AUDIO`,
    /// LF line ends, absolute triggers (skipped), no id (by position).
    #[test]
    fn valarms_from_servers() {
        let text = "BEGIN:VALARM\nX-WR-ALARMUID:F1A2-B3\nUID:F1A2-B3\nTRIGGER:-PT5M\nACTION:AUDIO\nEND:VALARM\n\
                    BEGIN:VALARM\nTRIGGER;VALUE=DATE-TIME:20260101T000000Z\nACTION:DISPLAY\nEND:VALARM\n\
                    BEGIN:VALARM\nTRIGGER:PT0S\nACTION:DISPLAY\nDESCRIPTION:x\nEND:VALARM\n\
                    BEGIN:VALARM\nTRIGGER:-PT1H\nACTION:PROCEDURE\nEND:VALARM";
        let read = DefaultAlerts::from_valarms(text);
        let ids: Vec<&str> = read.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["F1A2-B3", "k3"]);
        let (_, first) = read.iter().next().unwrap();
        assert_eq!(first.action, AlertAction::Display);
        assert_eq!(first.offset.to_string(), "-PT5M");
    }

    /// The value can't close the wrapping event or bring anything else.
    #[test]
    fn hostile_valarms() {
        for text in [
            "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:x",
            "TRIGGER:-PT5M\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM",
            "BEGIN:VALARM\r\nBEGIN:VALARM\r\nEND:VALARM\r\nEND:VALARM",
            "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY",
            "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nEN\r\n D:VEVENT\r\nEND:VALARM",
            "BEGIN:VTODO\r\nEND:VTODO",
            "garbage",
        ] {
            assert!(DefaultAlerts::from_valarms(text).is_empty(), "{text:?}");
        }
        let many = "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nEND:VALARM\r\n".repeat(100);
        assert_eq!(
            DefaultAlerts::from_valarms(&many).iter().count(),
            MAX_DEFAULT_ALERTS
        );
        let huge = format!(
            "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nDESCRIPTION:{}\r\nEND:VALARM",
            "x".repeat(MAX_VALARMS_BYTES)
        );
        assert!(DefaultAlerts::from_valarms(&huge).is_empty());
    }

    #[test]
    fn zones() {
        assert_eq!(
            canonical_zone("Europe/Madrid").as_deref(),
            Some("Europe/Madrid")
        );
        assert_eq!(canonical_zone("Nowhere/Land"), None);
        let tz = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:America/New_York\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
        assert_eq!(
            calendar_zone(Some("Europe/Madrid"), Some(tz)).as_deref(),
            Some("Europe/Madrid")
        );
        assert_eq!(
            calendar_zone(Some("bad"), Some(tz)).as_deref(),
            Some("America/New_York")
        );
        assert_eq!(calendar_zone(None, None), None);
        let made = vtimezone_calendar("Europe/Madrid").unwrap();
        assert!(made.starts_with("BEGIN:VCALENDAR\r\n"), "{made}");
        assert!(
            made.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\n"),
            "{made}"
        );
        assert!(made.contains("BEGIN:STANDARD") && made.ends_with("END:VCALENDAR\r\n"));
        assert!(
            made.contains("BEGIN:DAYLIGHT") && made.contains("RRULE:"),
            "{made}"
        );
        assert!(made.len() < 4096, "{made}");
        assert_eq!(
            calendar_zone(None, Some(&made)).as_deref(),
            Some("Europe/Madrid")
        );
        assert_eq!(vtimezone_calendar("Nowhere/Land"), None);
    }
}
