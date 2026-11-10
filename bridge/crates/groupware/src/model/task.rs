// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a write does to a task (`VTODO`) beyond calcard's conversion
//!without I/O.
//!
//! calcard converts `STATUS` ↔ `progress` and `PERCENT-COMPLETE` ↔
//! `percentComplete`, but not `COMPLETED` (the date it was done): that one
//! stays under `iCalendar.properties`, so the web never sets it. Other
//! clients (Apple Reminders, Nextcloud Tasks, Thunderbird, tasks.org) do,
//! and some show a task as done only with it. So, on each write, for every
//! `VTODO` (an override of a recurring task on its own, compared with the
//! same occurrence before, or with the series if it is new):
//!
//! - **becomes `COMPLETED`**: `COMPLETED` = now (UTC), unless the write
//!   brings one where there was none; `PERCENT-COMPLETE:100` (as Apple
//!   and Nextcloud write it).
//! - **stops being `COMPLETED`**: `COMPLETED` removed, and a
//!   `PERCENT-COMPLETE:100` with it (a task that is not done is not at
//!   100 %; another value stays).
//! - otherwise nothing changes (a task done elsewhere keeps its date).

use calcard::common::PartialDateTime;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarProperty,
    ICalendarStatus, ICalendarValue,
};

fn is_completed(component: &ICalendarComponent) -> bool {
    component
        .property(&ICalendarProperty::Status)
        .and_then(|entry| entry.values.first())
        .is_some_and(|value| matches!(value, ICalendarValue::Status(ICalendarStatus::Completed)))
}

fn recurrence_id(component: &ICalendarComponent) -> Option<&ICalendarValue> {
    component
        .property(&ICalendarProperty::RecurrenceId)
        .and_then(|entry| entry.values.first())
}

fn completed_value(component: &ICalendarComponent) -> Option<&ICalendarValue> {
    component
        .property(&ICalendarProperty::Completed)
        .and_then(|entry| entry.values.first())
}

/// The component of `before` that `component` was: the same occurrence
/// (`RECURRENCE-ID`), else the series (what a new override inherits).
fn previous<'a>(
    before: &'a ICalendar,
    component: &ICalendarComponent,
) -> Option<&'a ICalendarComponent> {
    let todos = || {
        before
            .components
            .iter()
            .filter(|c| c.component_type == ICalendarComponentType::VTodo)
    };
    let own = recurrence_id(component);
    own.and_then(|own| todos().find(|c| recurrence_id(c) == Some(own)))
        .or_else(|| todos().find(|c| recurrence_id(c).is_none()))
}

fn set(component: &mut ICalendarComponent, name: ICalendarProperty, value: ICalendarValue) {
    component.entries.retain(|entry| entry.name != name);
    component
        .entries
        .push(ICalendarEntry::new(name).with_value(value));
}

/// Sets or clears `COMPLETED` and `PERCENT-COMPLETE` on the tasks of
/// `ical` that were completed or reopened by this write (`before`: the
/// stored object, `None` for a new one). `now`: UTC seconds.
pub(crate) fn mark_completion(ical: &mut ICalendar, before: Option<&ICalendar>, now: i64) {
    let mut changes = Vec::new();
    for (index, component) in ical.components.iter().enumerate() {
        if component.component_type != ICalendarComponentType::VTodo {
            continue;
        }
        let old = before.and_then(|before| previous(before, component));
        let (was, is) = (old.is_some_and(is_completed), is_completed(component));
        if was == is {
            continue;
        }
        // A COMPLETED the write itself brings (the stored one had none) is
        // kept. One that was there already is stale (or copied back from
        // `iCalendar.properties`): now.
        let brings_own =
            completed_value(component).is_some() && old.and_then(completed_value).is_none();
        changes.push((index, is, brings_own));
    }
    for (index, completed, brings_own) in changes {
        let Some(component) = ical.components.get_mut(index) else {
            continue;
        };
        if completed {
            if !brings_own {
                set(
                    component,
                    ICalendarProperty::Completed,
                    ICalendarValue::PartialDateTime(PartialDateTime::from_utc_timestamp(now)),
                );
            }
            set(
                component,
                ICalendarProperty::PercentComplete,
                ICalendarValue::Integer(100),
            );
        } else {
            component.entries.retain(|entry| {
                entry.name != ICalendarProperty::Completed
                    && !(entry.name == ICalendarProperty::PercentComplete
                        && entry.values.first().and_then(ICalendarValue::as_integer) == Some(100))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_000_000; // 2026-10-14T17:46:40Z

    fn ical(todos: &str) -> ICalendar {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n{}END:VCALENDAR\r\n",
            todos.replace('\n', "\r\n")
        );
        ICalendar::parse(&text).unwrap()
    }

    fn text(ical: &ICalendar) -> String {
        ical.to_string()
    }

    const OPEN: &str = "BEGIN:VTODO\nUID:t@example.com\nSUMMARY:Milk\n\
        DUE;VALUE=DATE:20261030\nSTATUS:NEEDS-ACTION\nEND:VTODO\n";
    const DONE: &str = "BEGIN:VTODO\nUID:t@example.com\nSUMMARY:Milk\n\
        DUE;VALUE=DATE:20261030\nSTATUS:COMPLETED\nCOMPLETED:20261001T100000Z\n\
        PERCENT-COMPLETE:100\nEND:VTODO\n";

    #[test]
    fn completing_sets_the_date_and_percent() {
        let before = ical(OPEN);
        let mut after = ical(&OPEN.replace("NEEDS-ACTION", "COMPLETED"));
        mark_completion(&mut after, Some(&before), NOW);
        let out = text(&after);
        assert!(out.contains("COMPLETED:20261014T174640Z"), "{out}");
        assert!(out.contains("PERCENT-COMPLETE:100"), "{out}");
        // A new task already done gets them too.
        let mut new = ical(&OPEN.replace("NEEDS-ACTION", "COMPLETED"));
        mark_completion(&mut new, None, NOW);
        assert!(text(&new).contains("COMPLETED:20261014T174640Z"));
    }

    #[test]
    fn reopening_clears_them() {
        let before = ical(DONE);
        let mut after = ical(&DONE.replace("STATUS:COMPLETED", "STATUS:NEEDS-ACTION"));
        mark_completion(&mut after, Some(&before), NOW);
        let out = text(&after);
        assert!(!out.contains("COMPLETED:2026"), "{out}");
        assert!(!out.contains("PERCENT-COMPLETE"), "{out}");
        assert!(out.contains("STATUS:NEEDS-ACTION"), "{out}");
        // Another percentage stays.
        let mut half = ical(
            &DONE
                .replace("STATUS:COMPLETED", "STATUS:IN-PROCESS")
                .replace("PERCENT-COMPLETE:100", "PERCENT-COMPLETE:50"),
        );
        mark_completion(&mut half, Some(&before), NOW);
        assert!(text(&half).contains("PERCENT-COMPLETE:50"));
    }

    #[test]
    fn unchanged_status_keeps_the_stored_date() {
        let before = ical(DONE);
        let mut after = ical(&DONE.replace("Milk", "Oat milk"));
        mark_completion(&mut after, Some(&before), NOW);
        assert!(text(&after).contains("COMPLETED:20261001T100000Z"));
        // Done without a date elsewhere, and still done: left as it is.
        let bare = DONE.replace("COMPLETED:20261001T100000Z\n", "");
        let before = ical(&bare);
        let mut after = ical(&bare.replace("Milk", "Oat milk"));
        mark_completion(&mut after, Some(&before), NOW);
        assert!(!text(&after).contains("COMPLETED:2026"));
    }

    #[test]
    fn a_date_the_write_brings_is_kept() {
        let before = ical(OPEN);
        let mut after = ical(&OPEN.replace(
            "STATUS:NEEDS-ACTION",
            "STATUS:COMPLETED\nCOMPLETED:20261010T080000Z",
        ));
        mark_completion(&mut after, Some(&before), NOW);
        let out = text(&after);
        assert!(out.contains("COMPLETED:20261010T080000Z"), "{out}");
        assert_eq!(out.matches("COMPLETED:2026").count(), 1, "{out}");
    }

    #[test]
    fn an_occurrence_is_compared_with_the_series() {
        let series = "BEGIN:VTODO\nUID:r@example.com\nSUMMARY:Water plants\n\
            DTSTART;VALUE=DATE:20261019\nDUE;VALUE=DATE:20261019\nRRULE:FREQ=WEEKLY\n\
            STATUS:NEEDS-ACTION\nEND:VTODO\n";
        let before = ical(series);
        let done = "BEGIN:VTODO\nUID:r@example.com\nSUMMARY:Water plants\n\
            RECURRENCE-ID;VALUE=DATE:20261026\nDTSTART;VALUE=DATE:20261026\n\
            DUE;VALUE=DATE:20261026\nSTATUS:COMPLETED\nEND:VTODO\n";
        let mut after = ical(&format!("{series}{done}"));
        mark_completion(&mut after, Some(&before), NOW);
        let out = text(&after);
        // Only the override is done.
        assert_eq!(
            out.matches("COMPLETED:20261014T174640Z").count(),
            1,
            "{out}"
        );
        assert_eq!(out.matches("PERCENT-COMPLETE:100").count(), 1, "{out}");
        // Reopening that occurrence later clears it.
        let before = after.clone();
        let mut reopened =
            ICalendar::parse(out.replace("STATUS:COMPLETED", "STATUS:NEEDS-ACTION")).unwrap();
        assert!(text(&reopened).contains("COMPLETED:2026"));
        mark_completion(&mut reopened, Some(&before), NOW);
        let out = text(&reopened);
        assert!(!out.contains("COMPLETED:2026"), "{out}");
        assert!(!out.contains("PERCENT-COMPLETE"), "{out}");
    }
}
