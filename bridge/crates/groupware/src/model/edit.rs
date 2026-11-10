// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Editing a stored iCalendar object through JSCalendar
//! without I/O. CPU work (calcard): call it off the async
//! threads.
//!
//! - [`EditableEvent::open`]: the stored text as calcard's JSCalendar
//!   `Group` (with what it does not convert under `iCalendar`, and the
//!   `VTIMEZONE`s), and its first entry (the event, with its overrides in
//!   `recurrenceOverrides`) apart, for the JMAP layer to patch.
//! - [`EditableEvent::to_icalendar`] / [`new_icalendar`]: back to
//!   iCalendar: `DTSTAMP` and `LAST-MODIFIED` now on what changed, a
//!   `PRODID`, a `VTIMEZONE` for every `TZID` used, no `METHOD`.
//! - [`occurrence_target`]: where one occurrence goes in
//!   `recurrenceOverrides` (its key, start and duration), as Stalwart's
//!   `CalendarEvent/set` by synthetic id.

use calcard::common::PartialDateTime;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarProperty,
    ICalendarValue,
};
use serde_json::{Map, Value};

use super::calendar::{EventOptions, ParsedCalendar, RecurrenceKey, Zone};
use super::convert::{Format, MAX_ICALENDAR_BYTES, checked_text, guarded, write_text};
use crate::ConvertError;

type CalendarObject<'x> = calcard::jscalendar::JSCalendar<'x, String, String>;

/// The `PRODID` of objects the bridge writes from scratch.
pub const PRODID: &str = "-//Nebula Mail//Bridge//EN";

/// A stored object opened for a change.
#[derive(Clone)]
pub struct EditableEvent {
    group: Map<String, Value>,
    /// The event or task (first entry of the group), to patch.
    pub event: Map<String, Value>,
    /// The event as opened, to tell which components a patch changes
    /// (their `LAST-MODIFIED` and `DTSTAMP`, [`stamp`]).
    opened: Map<String, Value>,
    /// The stored object, to see what the write changes (a task completed
    /// or reopened, [`super::task`]).
    before: ICalendar,
}

impl std::fmt::Debug for EditableEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditableEvent").finish_non_exhaustive()
    }
}

impl EditableEvent {
    /// Opens an iCalendar object whose first entry is an event or a task.
    ///
    /// # Errors
    /// [`ConvertError`] for anything that is not a `VCALENDAR` with an
    /// event or a task first.
    pub fn open(text: &[u8]) -> Result<Self, ConvertError> {
        let format = Format::ICalendar;
        let text = checked_text(text, format)?;
        guarded(format, || {
            let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
            let before = ical.clone();
            let js: CalendarObject<'static> = ical.into_jscalendar();
            let Ok(Value::Object(mut group)) = serde_json::to_value(&js.0) else {
                return Err(ConvertError::Invalid { format });
            };
            let Some(Value::Array(entries)) = group.get_mut("entries") else {
                return Err(ConvertError::Invalid { format });
            };
            if entries.is_empty() {
                return Err(ConvertError::Invalid { format });
            }
            let Value::Object(event) = entries.remove(0) else {
                return Err(ConvertError::Invalid { format });
            };
            if !matches!(
                event.get("@type").and_then(Value::as_str),
                Some("Event" | "Task")
            ) {
                return Err(ConvertError::Invalid { format });
            }
            Ok(Self {
                group,
                opened: event.clone(),
                event,
                before,
            })
        })
    }

    /// Whether it is a task (JSCalendar `Task`, a `VTODO`).
    #[must_use]
    pub fn is_task(&self) -> bool {
        self.event.get("@type").and_then(Value::as_str) == Some("Task")
    }

    /// The iCalendar text with the (patched) event back in its place.
    ///
    /// # Errors
    /// [`ConvertError::Export`] if calcard can't write it;
    /// [`ConvertError::TooLarge`] past the iCalendar limit.
    pub fn to_icalendar(&self, now: i64) -> Result<String, ConvertError> {
        let with = |event: &Map<String, Value>| {
            let mut group = self.group.clone();
            let entries = group
                .entry("entries")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(list) = entries {
                list.insert(0, Value::Object(event.clone()));
            }
            Value::Object(group)
        };
        let baseline = with(&self.opened);
        export(&with(&self.event), now, Some((&self.before, &baseline)))
    }
}

/// The iCalendar text of a new event or task (a JSCalendar `Event`, or a
/// `Task` when `@type` says so; `uid` set).
///
/// # Errors
/// As [`EditableEvent::to_icalendar`].
pub fn new_icalendar(event: &Map<String, Value>, now: i64) -> Result<String, ConvertError> {
    let mut event = event.clone();
    event.entry("@type").or_insert_with(|| Value::from("Event"));
    let mut group = Map::new();
    group.insert("@type".to_owned(), Value::from("Group"));
    group.insert(
        "entries".to_owned(),
        Value::Array(vec![Value::Object(event)]),
    );
    export(&Value::Object(group), now, None)
}

/// Whether `entry` says when the component was written, not what it is.
fn is_stamp(entry: &ICalendarEntry) -> bool {
    matches!(
        entry.name,
        ICalendarProperty::Dtstamp | ICalendarProperty::LastModified
    )
}

/// What a component says, without its stamps, sub-components included
/// (a `VALARM` is part of its event).
fn content<'a>(ical: &'a ICalendar, component: &'a ICalendarComponent) -> Vec<&'a ICalendarEntry> {
    let mut out: Vec<&ICalendarEntry> = component.entries.iter().filter(|e| !is_stamp(e)).collect();
    let mut stack: Vec<u32> = component.component_ids.clone();
    while let Some(id) = stack.pop() {
        if let Some(child) = usize::try_from(id)
            .ok()
            .and_then(|i| ical.components.get(i))
        {
            out.extend(child.entries.iter());
            stack.extend(child.component_ids.iter().copied());
        }
    }
    out
}

/// The stamps of every event or task the write changes (or adds):
///
/// - `LAST-MODIFIED` now (RFC 5545 §3.8.7.3: when the information of the
///   component was last revised in the calendar store); unchanged
///   components keep theirs. A new object gets none (that would be
///   `CREATED`, §3.8.7.1, which calcard writes from JSCalendar `created`).
/// - `DTSTAMP` now (RFC 5545 §3.8.7.2: REQUIRED; in an object without
///   `METHOD`, as a stored one is, it is equivalent to `LAST-MODIFIED`).
///   An unchanged component keeps its own, unless it has none.
///
/// `baseline`: the stored object converted as `ical` was (`None`: a new
/// object, or the stored one does not convert: everything is new).
fn stamp(ical: &mut ICalendar, baseline: Option<&ICalendar>, now: i64) {
    let stamp = PartialDateTime::from_utc_timestamp(now);
    let identity = |ical: &ICalendar, c: &ICalendarComponent| {
        (
            c.component_type.clone(),
            c.property(&ICalendarProperty::Uid).cloned(),
            c.property(&ICalendarProperty::RecurrenceId).cloned(),
            content(ical, c)
                .into_iter()
                .cloned()
                .collect::<Vec<ICalendarEntry>>(),
        )
    };
    let old: Vec<_> = baseline
        .map(|b| {
            b.components
                .iter()
                .filter(|c| is_item(c))
                .map(|c| identity(b, c))
                .collect()
        })
        .unwrap_or_default();
    let mut changed = Vec::new();
    for (index, component) in ical.components.iter().enumerate() {
        if !is_item(component) {
            continue;
        }
        let mine = identity(ical, component);
        changed.push((index, !old.contains(&mine)));
    }
    for (index, is_changed) in changed {
        let Some(component) = ical.components.get_mut(index) else {
            continue;
        };
        let has_stamp = component.property(&ICalendarProperty::Dtstamp).is_some();
        if is_changed || !has_stamp {
            component
                .entries
                .retain(|entry| entry.name != ICalendarProperty::Dtstamp);
            component.add_dtstamp(stamp.clone());
        }
        if is_changed && baseline.is_some() {
            component
                .entries
                .retain(|entry| entry.name != ICalendarProperty::LastModified);
            component.entries.push(
                ICalendarEntry::new(ICalendarProperty::LastModified)
                    .with_value(ICalendarValue::PartialDateTime(stamp.clone())),
            );
        }
    }
}

fn is_item(component: &ICalendarComponent) -> bool {
    matches!(
        component.component_type,
        ICalendarComponentType::VEvent | ICalendarComponentType::VTodo
    )
}

/// `before`: the stored object and its group as opened (unpatched).
fn export(
    group: &Value,
    now: i64,
    before: Option<(&ICalendar, &Value)>,
) -> Result<String, ConvertError> {
    let format = Format::JsCalendar;
    let json = group.to_string();
    if json.len() > format.limit() {
        return Err(ConvertError::TooLarge {
            format,
            size: json.len(),
            limit: format.limit(),
        });
    }
    let baseline = before.map(|(_, opened)| opened.to_string());
    let text = guarded(format, || {
        let to_ical = |json: &str| {
            CalendarObject::parse(json)
                .map_err(|_| ConvertError::Invalid { format })?
                .into_icalendar()
                .map_err(|_| ConvertError::Export { format })
        };
        let mut ical = to_ical(&json)?;
        super::convert::check_ical_names(&ical)?;
        // The stored object through the same conversion: a component
        // that comes out the same was not changed (calcard reorders
        // properties, so the stored text itself can't be compared).
        let baseline = baseline.as_deref().map(to_ical).transpose().ok().flatten();
        stamp(&mut ical, baseline.as_ref(), now);
        finish(&mut ical);
        super::task::mark_completion(&mut ical, before.map(|(stored, _)| stored), now);
        write_text(&ical, format)
    })?;
    if text.len() > MAX_ICALENDAR_BYTES {
        return Err(ConvertError::TooLarge {
            format: Format::ICalendar,
            size: text.len(),
            limit: MAX_ICALENDAR_BYTES,
        });
    }
    Ok(text)
}

/// What every write gets (RFC 5545's required properties): a `PRODID`,
/// the `VTIMEZONE`s (§3.2.19), and never a `METHOD` (RFC 4791 §4.1: a
/// calendar object resource MUST NOT have one; JSCalendar's `method`
/// would add it, from the client or from a stored object another client
/// wrote with it). The stamps: [`stamp`].
fn finish(ical: &mut ICalendar) {
    super::convert::strip_method(ical);
    if let Some(root) = ical.components.first_mut()
        && root.component_type == ICalendarComponentType::VCalendar
        && root.property(&ICalendarProperty::Prodid).is_none()
    {
        root.entries.insert(
            0,
            ICalendarEntry::new(ICalendarProperty::Prodid)
                .with_value(ICalendarValue::Text(PRODID.to_owned())),
        );
    }
    ical.add_missing_timezones();
}

/// Where one occurrence of a stored event is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceTarget {
    /// Whether the event repeats; if not, the "occurrence" is the event
    /// itself (Stalwart: a synthetic id of an event without repetition
    /// names the event).
    pub recurring: bool,
    /// Its key in `recurrenceOverrides` (`LocalDateTime`).
    pub key: String,
    /// Its start now (`LocalDateTime`, in the event's zone).
    pub start: Option<String>,
    /// Its duration now.
    pub duration: Option<String>,
    /// A task's `due` now (`LocalDateTime`, in the task's zone).
    pub due: Option<String>,
}

/// `YYYY-MM-DDTHH:MM:SS` of a wall-clock reading.
fn local_text(naive: i64) -> Option<String> {
    let dt = PartialDateTime::from_naive_timestamp(naive);
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year?,
        dt.month?,
        dt.day?,
        dt.hour.unwrap_or(0),
        dt.minute.unwrap_or(0),
        dt.second.unwrap_or(0)
    ))
}

/// Where the occurrence `key` of the object goes (`None`: it has no such
/// occurrence). CPU (it expands the recurrence): call it off the async
/// threads (`spawn_blocking`).
///
/// # Errors
/// [`ConvertError`] if the object does not parse.
pub fn occurrence_target(
    text: &[u8],
    key: RecurrenceKey,
) -> Result<Option<OccurrenceTarget>, ConvertError> {
    let parsed = ParsedCalendar::parse(text)?;
    let Some(event) = parsed.occurrence(key, Zone::UTC, &EventOptions::default())? else {
        return Ok(None);
    };
    let editable = EditableEvent::open(text)?;
    let base = &editable.event;
    // Also the iCalendar's own rules: a task with a DUE and no DTSTART
    // keeps its RRULE under `iCalendar.properties` (RFC 8984 repeats from
    // `start`), yet its occurrences are real.
    let recurring = parsed.is_recurring()
        || ["recurrenceRule", "recurrenceRules", "recurrenceOverrides"]
            .iter()
            .any(|name| base.get(*name).is_some_and(|v| !v.is_null()));
    let key = local_text(key.naive()).ok_or(ConvertError::Invalid {
        format: Format::ICalendar,
    })?;
    let text_of = |name: &str| {
        event
            .members
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(Some(OccurrenceTarget {
        recurring,
        key,
        start: text_of("start"),
        duration: text_of("duration"),
        due: text_of("due"),
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    pub(crate) const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n\
        X-WR-CALNAME:keep\r\nBEGIN:VEVENT\r\nUID:w@example.com\r\nDTSTAMP:20261001T000000Z\r\n\
        SEQUENCE:2\r\nX-FOO;BAR=1:keep me\r\n\
        DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDTEND;TZID=Europe/Madrid:20261019T103000\r\n\
        RRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const NOW: i64 = 1_792_000_000;

    #[test]
    fn a_round_trip_keeps_what_jmap_does_not_know() {
        let mut edit = EditableEvent::open(WEEKLY.as_bytes()).unwrap();
        edit.event
            .insert("title".to_owned(), Value::from("Renamed"));
        let text = edit.to_icalendar(NOW).unwrap();
        for kept in [
            "X-FOO;BAR=1:keep me",
            "X-WR-CALNAME:keep",
            "PRODID:-//t//EN",
            "RRULE:FREQ=WEEKLY;COUNT=4",
            "SUMMARY:Renamed",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Madrid",
        ] {
            assert!(text.contains(kept), "{kept} missing:\n{text}");
        }
        assert!(!text.contains("DTSTAMP:20261001T000000Z"), "{text}");
        assert_eq!(text.matches("DTSTAMP").count(), 1, "{text}");
    }

    #[test]
    fn new_events_get_what_ical_requires() {
        let event = json!({"title": "New", "start": "2026-11-05T10:00:00", "duration": "PT1H",
                           "timeZone": "Europe/Madrid", "uid": "u1"});
        let text = new_icalendar(event.as_object().unwrap(), NOW).unwrap();
        for wanted in [
            "PRODID:-//Nebula Mail//Bridge//EN",
            "DTSTAMP:",
            "UID:u1",
            "DTSTART;TZID=Europe/Madrid:20261105T100000",
            "BEGIN:VTIMEZONE",
        ] {
            assert!(text.contains(wanted), "{wanted} missing:\n{text}");
        }
    }

    /// RFC 4791 §4.1: what goes into a calendar has no `METHOD`, whether
    /// the client sends JSCalendar `method` or the stored object had one.
    #[test]
    fn calendar_objects_never_carry_method() {
        let event = json!({"title": "New", "start": "2026-11-05T10:00:00", "duration": "PT1H",
                           "timeZone": "Europe/Madrid", "uid": "u1", "method": "request"});
        let text = new_icalendar(event.as_object().unwrap(), NOW).unwrap();
        assert!(!text.contains("METHOD"), "{text}");
        let raw = with_ical(json!({"properties": [["method", {}, "text", "PUBLISH"]]}));
        let text = new_icalendar(&raw, NOW).unwrap();
        assert!(!text.contains("METHOD"), "{text}");

        let stored = WEEKLY.replace("X-WR-CALNAME:keep", "METHOD:REQUEST\r\nX-WR-CALNAME:keep");
        let mut edit = EditableEvent::open(stored.as_bytes()).unwrap();
        edit.event
            .insert("method".to_owned(), Value::from("publish"));
        let text = edit.to_icalendar(NOW).unwrap();
        assert!(!text.contains("METHOD"), "{text}");
        assert!(text.contains("X-WR-CALNAME:keep"), "{text}");
    }

    /// The `VEVENT` lines of `text`, one list per component.
    fn components(text: &str) -> Vec<String> {
        text.split("BEGIN:VEVENT\r\n")
            .skip(1)
            .map(|c| c.split("END:VEVENT").next().unwrap_or_default().to_owned())
            .collect()
    }

    /// RFC 5545 §3.8.7.3 / §3.8.7.2: `LAST-MODIFIED` and `DTSTAMP` move on
    /// the components the edit changes, never on the others.
    #[test]
    fn stamps_follow_what_changed() {
        let uid = WEEKLY.lines().find(|l| l.starts_with("UID:")).unwrap();
        let stored = WEEKLY
            .replace(
                "SUMMARY:Weekly\r\n",
                "SUMMARY:Weekly\r\nLAST-MODIFIED:20261001T000000Z\r\n",
            )
            .replace(
                "END:VEVENT\r\nEND:VCALENDAR",
                &format!(
                    "END:VEVENT\r\nBEGIN:VEVENT\r\n{uid}\r\nDTSTAMP:20261002T000000Z\r\n\
                 LAST-MODIFIED:20261002T000000Z\r\n\
                 RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
                 DTSTART;TZID=Europe/Madrid:20261026T120000\r\n\
                 DTEND;TZID=Europe/Madrid:20261026T123000\r\nSUMMARY:Moved\r\n\
                 END:VEVENT\r\nEND:VCALENDAR"
                ),
            );
        let now_stamp = "20261014T174640Z"; // NOW
        // Only the override changes.
        let mut edit = EditableEvent::open(stored.as_bytes()).unwrap();
        let overrides = edit
            .event
            .get_mut("recurrenceOverrides")
            .and_then(Value::as_object_mut)
            .unwrap();
        let (_, patch) = overrides.iter_mut().next().unwrap();
        patch["title"] = Value::from("Moved again");
        let text = edit.to_icalendar(NOW).unwrap();
        let parts = components(&text);
        assert_eq!(parts.len(), 2, "{text}");
        let (master, moved) = if parts[0].contains("RECURRENCE-ID") {
            (&parts[1], &parts[0])
        } else {
            (&parts[0], &parts[1])
        };
        assert!(master.contains("DTSTAMP:20261001T000000Z"), "{text}");
        assert!(master.contains("LAST-MODIFIED:20261001T000000Z"), "{text}");
        assert!(moved.contains("SUMMARY:Moved again"), "{text}");
        assert!(moved.contains(&format!("DTSTAMP:{now_stamp}")), "{text}");
        assert!(
            moved.contains(&format!("LAST-MODIFIED:{now_stamp}")),
            "{text}"
        );
        assert_eq!(moved.matches("LAST-MODIFIED").count(), 1, "{text}");

        // The master changes: so does the override, which inherits it.
        let mut edit = EditableEvent::open(stored.as_bytes()).unwrap();
        edit.event
            .insert("description".to_owned(), Value::from("Agenda"));
        let text = edit.to_icalendar(NOW).unwrap();
        assert_eq!(
            text.matches(&format!("LAST-MODIFIED:{now_stamp}")).count(),
            2,
            "{text}"
        );
        assert!(!text.contains("20261001T000000Z"), "{text}");

        // A new event: DTSTAMP, no LAST-MODIFIED.
        let event = json!({"title": "New", "start": "2026-11-05T10:00:00", "uid": "u1"});
        let text = new_icalendar(event.as_object().unwrap(), NOW).unwrap();
        assert!(text.contains(&format!("DTSTAMP:{now_stamp}")), "{text}");
        assert!(!text.contains("LAST-MODIFIED"), "{text}");
    }

    #[test]
    fn occurrences_are_found_by_key() {
        // 2026-10-26T10:00 as a wall-clock reading.
        let key = RecurrenceKey::from_naive(1_792_404_000 + 7 * 86_400).unwrap();
        let target = occurrence_target(WEEKLY.as_bytes(), key).unwrap().unwrap();
        assert!(target.recurring);
        assert_eq!(target.key, "2026-10-26T10:00:00");
        assert_eq!(target.start.as_deref(), Some("2026-10-26T10:00:00"));
        assert_eq!(target.duration.as_deref(), Some("PT30M"));
        let missing = RecurrenceKey::from_naive(1_792_404_000 + 3600).unwrap();
        assert!(
            occurrence_target(WEEKLY.as_bytes(), missing)
                .unwrap()
                .is_none()
        );
        assert!(EditableEvent::open(b"BEGIN:VCARD\r\nEND:VCARD\r\n").is_err());
    }

    fn with_ical(extra: Value) -> Map<String, Value> {
        let mut event = json!({"title": "New", "start": "2026-11-05T10:00:00", "duration": "PT1H",
                               "timeZone": "Europe/Madrid", "uid": "u1"});
        event["iCalendar"] = extra;
        event.as_object().unwrap().clone()
    }

    /// A name from the JSON is written as it is: one that breaks the line
    /// (CR/LF, `:`, `;`…) would add properties or events of its own.
    #[test]
    fn names_that_break_the_format_are_refused() {
        let injected = with_ical(json!({"properties": [[
            "x-a:b\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:y\r\nX-C", {}, "text", "z"
        ]]}));
        assert!(matches!(
            new_icalendar(&injected, NOW),
            Err(ConvertError::Export { .. })
        ));
        for name in ["x-a;b", "x a", "", "x-\u{7f}"] {
            let bad = with_ical(json!({"properties": [[name, {}, "text", "z"]]}));
            assert!(new_icalendar(&bad, NOW).is_err(), "{name:?}");
        }
        let param = with_ical(json!({"properties": [[
            "x-ok", {"x-p\r\nX-EVIL": "1"}, "text", "z"
        ]]}));
        assert!(new_icalendar(&param, NOW).is_err());
        let param = with_ical(json!({"properties": [["x-ok", {"x-p;q": "1"}, "text", "z"]]}));
        assert!(new_icalendar(&param, NOW).is_err());
        let fine = with_ical(json!({"properties": [["x-ok", {"x-p": "a;b"}, "text", "z"]]}));
        let text = new_icalendar(&fine, NOW).unwrap();
        assert!(text.contains("X-OK;X-P=\"a;b\":z"), "{text}");
    }

    /// A component calcard does not know, named with a line break.
    #[test]
    fn component_names_that_break_the_format_are_refused() {
        let mut ical = ICalendar::parse(WEEKLY).unwrap();
        ical.components[1].component_type =
            ICalendarComponentType::Other("X-A\r\nBEGIN:VEVENT".to_owned());
        assert!(super::super::convert::check_ical_names(&ical).is_err());
        ical.components[1].component_type = ICalendarComponentType::Other("X-FINE_1".to_owned());
        assert!(super::super::convert::check_ical_names(&ical).is_ok());
    }

    /// Names outside RFC 5545's token that other clients write (`_`, not
    /// ASCII) are kept: such an event stays editable.
    #[test]
    fn odd_but_harmless_names_stay_editable() {
        let stored = WEEKLY.replace("X-FOO;BAR=1:keep me", "X-FOO_BAR:1\r\nX-ÑAME;X-P_Q=2:yes");
        let mut edit = EditableEvent::open(stored.as_bytes()).unwrap();
        edit.event
            .insert("title".to_owned(), Value::from("Renamed"));
        let text = edit.to_icalendar(NOW).unwrap();
        assert!(text.contains("X-FOO_BAR:1"), "{text}");
        assert!(text.contains("SUMMARY:Renamed"), "{text}");
        assert!(text.contains("X-P_Q=2"), "{text}");
    }
}
