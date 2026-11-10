// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Tasks (`VTODO`, JSCalendar `Task`) through the
//! JMAP model, with the `task-*` files of the corpus: Apple Reminders,
//! Nextcloud Tasks (a list with a subtask), Thunderbird, tasks.org through
//! `DAVx5` (a weekly task with one occurrence done) and an undated one.
//! Every file reads as a `Task`, keeps what JMAP does not know after a
//! write, is placed by its `DUE`, and its reminders ring relative to it.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    reason = "test helpers; y/m/d/h are the usual names of a date's parts"
)]

use std::path::PathBuf;

use nebula_groupware::model::alarm::alerts_between;
use nebula_groupware::model::calendar::{
    EventOptions, ItemKind, ParsedCalendar, Zone, parse_events,
};
use nebula_groupware::model::convert::icalendar_index;
use nebula_groupware::model::edit::{EditableEvent, new_icalendar, occurrence_target};
use serde_json::{Value, json};

fn tasks() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/corpus");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_stem()
                .is_some_and(|stem| stem.to_string_lossy().starts_with("task-"))
        })
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace('\n', "\r\n");
            (name, text.into_bytes())
        })
        .collect();
    files.sort();
    files
}

fn file(name: &str) -> Vec<u8> {
    tasks().into_iter().find(|(n, _)| n == name).unwrap().1
}

/// UTC seconds of `YYYY-MM-DDTHH:MM` (days from civil, Howard Hinnant).
fn utc(text: &str) -> i64 {
    let n: Vec<i64> = text
        .split(['-', 'T', ':'])
        .map(|p| p.parse().unwrap())
        .collect();
    let [y, m, d, h, mi] = n[..] else {
        panic!("{text}")
    };
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60
}

const NOW: i64 = 1_793_000_000;

fn madrid() -> Zone {
    Zone::named("Europe/Madrid").unwrap()
}

fn with_unconverted() -> EventOptions {
    EventOptions {
        keep_unconverted: true,
        ..EventOptions::default()
    }
}

#[test]
fn the_corpus_has_every_style() {
    let names: Vec<String> = tasks().into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        [
            "task-apple-reminders",
            "task-nextcloud-child",
            "task-nextcloud-parent",
            "task-tasksorg-recurring",
            "task-thunderbird",
            "task-undated"
        ]
    );
}

#[test]
fn every_task_reads_as_a_task() {
    for (name, bytes) in tasks() {
        let parsed = ParsedCalendar::parse(&bytes).unwrap();
        assert!(parsed.has_events(), "{name}");
        assert_eq!(parsed.kind(), Some(ItemKind::Task), "{name}");
        let base = parsed
            .base_event(madrid(), &with_unconverted())
            .unwrap()
            .unwrap_or_else(|| panic!("{name}: base"));
        assert_eq!(base.members["@type"], "Task", "{name}");
        let parsed_list = parse_events(&bytes).unwrap();
        assert_eq!(parsed_list.len(), 1, "{name}");
        assert_eq!(parsed_list[0]["@type"], "Task", "{name}");
    }
}

#[test]
fn what_jmap_does_not_know_survives_a_write() {
    let kept: &[(&str, &[&str])] = &[
        (
            "task-apple-reminders",
            &[
                "X-APPLE-SORT-ORDER:745834512",
                "CLASS:PRIVATE",
                "PRIORITY:1",
                "X-WR-ALARMUID:5A1B2C3D-0000-4000-8000-000000000001",
                "TRIGGER;VALUE=DATE-TIME:20261103T080000Z",
                "DUE;TZID=Europe/Madrid:20261103T100000",
                "LAST-MODIFIED",
            ],
        ),
        (
            "task-nextcloud-parent",
            &[
                "X-OC-HIDESUBTASKS:0",
                "X-APPLE-SORT-ORDER:2",
                "CATEGORIES:",
                "PERCENT-COMPLETE:30",
                "STATUS:IN-PROCESS",
                "DUE;VALUE=DATE:20261105",
                "CLASS:PUBLIC",
            ],
        ),
        (
            "task-nextcloud-child",
            &[
                "RELATED-TO;RELTYPE=PARENT:nebula-corpus-task-nextcloud-parent",
                "COMPLETED:20261022T180000Z",
                "PERCENT-COMPLETE:100",
                "STATUS:COMPLETED",
            ],
        ),
        (
            "task-thunderbird",
            &[
                "X-MOZ-GENERATION:3",
                "TRIGGER;RELATED=END:-PT15M",
                "PRIORITY:9",
                "DTSTART;TZID=Europe/Madrid:20261102T090000",
            ],
        ),
        (
            "task-tasksorg-recurring",
            &[
                "X-APPLE-SORT-ORDER:-17",
                "RRULE:FREQ=WEEKLY;COUNT=8;BYDAY=MO",
                "RECURRENCE-ID;VALUE=DATE:20261026",
                "COMPLETED:20261026T183000Z",
            ],
        ),
        ("task-undated", &["X-APPLE-SORT-ORDER:745000000"]),
    ];
    for (name, wanted) in kept {
        let mut edit = EditableEvent::open(&file(name)).unwrap();
        assert!(edit.is_task(), "{name}");
        edit.event.insert("title".to_owned(), json!("Cambiado"));
        let text = edit.to_icalendar(NOW).unwrap();
        assert!(text.contains("SUMMARY:Cambiado"), "{name}:\n{text}");
        assert!(text.contains("BEGIN:VTODO"), "{name}:\n{text}");
        assert!(!text.contains("BEGIN:VEVENT"), "{name}:\n{text}");
        for line in *wanted {
            assert!(text.contains(line), "{name}: {line} missing:\n{text}");
        }
    }
}

#[test]
fn tasks_are_placed_by_their_due() {
    // DTSTART and DUE: the index spans both; the occurrence sorts by DUE.
    let thunderbird = file("task-thunderbird");
    let index = icalendar_index(&thunderbird, 1000).unwrap();
    assert_eq!(index.utc_start, Some(utc("2026-11-02T08:00")));
    assert_eq!(index.utc_end, Some(utc("2026-11-04T16:00")));
    let parsed = ParsedCalendar::parse(&thunderbird).unwrap();
    let instances = parsed.instances(madrid()).unwrap().list;
    assert_eq!(instances.len(), 1);
    let task = instances[0];
    assert!(task.is_task());
    assert_eq!(task.placed_at(), utc("2026-11-04T16:00"));
    // On its due day, not on the days it is open.
    assert!(task.overlaps(utc("2026-11-03T23:00"), utc("2026-11-04T23:00")));
    assert!(!task.overlaps(utc("2026-11-01T23:00"), utc("2026-11-02T23:00")));
    // A point: a due at midnight is on the day that starts then.
    let parent = ParsedCalendar::parse(&file("task-nextcloud-parent")).unwrap();
    let due = parent.instances(madrid()).unwrap().list[0];
    assert_eq!(due.placed_at(), utc("2026-11-04T23:00"));
    assert!(due.overlaps(utc("2026-11-04T23:00"), utc("2026-11-05T23:00")));
    assert!(!due.overlaps(utc("2026-11-03T23:00"), utc("2026-11-04T23:00")));
}

#[test]
fn undated_tasks_have_no_place() {
    // calcard would put them at CREATED; here they only come in queries
    // without dates.
    for name in ["task-undated", "task-nextcloud-child"] {
        let bytes = file(name);
        let index = icalendar_index(&bytes, 1000).unwrap();
        assert_eq!(index.utc_start, None, "{name}");
        assert_eq!(index.utc_end, None, "{name}");
        assert!(index.uid.is_some(), "{name}");
        let parsed = ParsedCalendar::parse(&bytes).unwrap();
        assert!(
            parsed.instances(madrid()).unwrap().list.is_empty(),
            "{name}"
        );
        let base = parsed
            .base_event(madrid(), &EventOptions::default())
            .unwrap()
            .unwrap();
        assert_eq!(base.utc_range, None, "{name}");
    }
}

#[test]
fn recurring_tasks_expand_with_their_dates() {
    let bytes = file("task-tasksorg-recurring");
    let parsed = ParsedCalendar::parse(&bytes).unwrap();
    let instances = parsed.instances(madrid()).unwrap();
    assert!(instances.complete);
    assert_eq!(instances.list.len(), 8);
    // All-day dues: midnight in the query's zone.
    assert_eq!(instances.list[1].placed_at(), utc("2026-10-25T23:00"));
    let done = parsed
        .occurrence(instances.list[1].key, madrid(), &with_unconverted())
        .unwrap()
        .unwrap();
    assert_eq!(done.members["@type"], "Task");
    assert_eq!(done.members["progress"], "completed");
    assert_eq!(done.members["due"], "2026-10-26T00:00:00");
    assert_eq!(done.members["showWithoutTime"], true);
    assert_eq!(done.members["recurrenceId"], "2026-10-26T00:00:00");
    assert!(done.members.get("start").is_none());
    assert!(done.members.get("recurrenceRule").is_none());
    let next = parsed
        .occurrence(instances.list[2].key, madrid(), &with_unconverted())
        .unwrap()
        .unwrap();
    assert_eq!(next.members["due"], "2026-11-02T00:00:00");
    assert_eq!(next.members["showWithoutTime"], true);
    assert_eq!(next.members["progress"], "needs-action");

    // The occurrence is where a write goes: under its date, with its due.
    let target = occurrence_target(&bytes, instances.list[2].key)
        .unwrap()
        .unwrap();
    assert!(target.recurring);
    assert_eq!(target.key, "2026-11-02T00:00:00");
    assert_eq!(target.due.as_deref(), Some("2026-11-02T00:00:00"));
    assert_eq!(target.start, None);
}

#[test]
fn completing_writes_completed_and_reopening_clears_it() {
    let mut edit = EditableEvent::open(&file("task-apple-reminders")).unwrap();
    edit.event
        .insert("progress".to_owned(), Value::from("completed"));
    let done = edit.to_icalendar(NOW).unwrap();
    // 1_793_000_000 = 2026-10-26T07:33:20Z.
    assert!(done.contains("STATUS:COMPLETED"), "{done}");
    assert!(done.contains("COMPLETED:20261026T073320Z"), "{done}");
    assert!(done.contains("PERCENT-COMPLETE:100"), "{done}");
    assert!(done.contains("X-APPLE-SORT-ORDER:745834512"), "{done}");

    let mut edit = EditableEvent::open(done.as_bytes()).unwrap();
    assert_eq!(edit.event["progress"], "completed");
    edit.event
        .insert("progress".to_owned(), Value::from("needs-action"));
    let open = edit.to_icalendar(NOW + 60).unwrap();
    assert!(open.contains("STATUS:NEEDS-ACTION"), "{open}");
    assert!(!open.contains("COMPLETED:2026"), "{open}");
    assert!(!open.contains("PERCENT-COMPLETE"), "{open}");

    // A title change of a task done elsewhere keeps its date.
    let mut edit = EditableEvent::open(&file("task-nextcloud-child")).unwrap();
    edit.event
        .insert("title".to_owned(), json!("Cajas grandes"));
    let text = edit.to_icalendar(NOW).unwrap();
    assert!(text.contains("COMPLETED:20261022T180000Z"), "{text}");
}

#[test]
fn completing_one_occurrence_of_a_series() {
    let bytes = file("task-tasksorg-recurring");
    let mut edit = EditableEvent::open(&bytes).unwrap();
    let overrides = edit
        .event
        .entry("recurrenceOverrides")
        .or_insert_with(|| json!({}));
    overrides["2026-11-02T00:00:00"] =
        json!({"progress": "completed", "due": "2026-11-02T00:00:00"});
    let text = edit.to_icalendar(NOW).unwrap();
    assert_eq!(text.matches("BEGIN:VTODO").count(), 3, "{text}");
    assert!(text.contains("RECURRENCE-ID;VALUE=DATE:20261102"), "{text}");
    assert!(text.contains("COMPLETED:20261026T073320Z"), "{text}");
    // The one done before keeps its own date.
    assert!(text.contains("COMPLETED:20261026T183000Z"), "{text}");
    let parsed = ParsedCalendar::parse(text.as_bytes()).unwrap();
    let list = parsed.instances(madrid()).unwrap().list;
    let third = parsed
        .occurrence(list[2].key, madrid(), &EventOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(third.members["progress"], "completed");
    let fourth = parsed
        .occurrence(list[3].key, madrid(), &EventOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(fourth.members["progress"], "needs-action");
}

#[test]
fn new_tasks_from_the_web() {
    // What the web creates: a subtask due on a day, priority, keywords.
    let task = json!({
        "@type": "Task", "uid": "web-task@example.com", "title": "Hija",
        "due": "2026-11-06T00:00:00", "showWithoutTime": true, "priority": 1,
        "keywords": {"casa": true}, "progress": "completed",
        "relatedTo": {"web-parent@example.com": {"@type": "Relation", "relation": {"parent": true}}},
        "alerts": {"a1": {"@type": "Alert", "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT1H", "relativeTo": "end"}}}
    });
    let text = new_icalendar(task.as_object().unwrap(), NOW).unwrap();
    for wanted in [
        "BEGIN:VTODO",
        "DUE;VALUE=DATE:20261106",
        "PRIORITY:1",
        "CATEGORIES:casa",
        "RELATED-TO;RELTYPE=PARENT:web-parent@example.com",
        "STATUS:COMPLETED",
        "COMPLETED:20261026T073320Z",
        "PERCENT-COMPLETE:100",
        "TRIGGER;RELATED=END:-PT1H",
        "DTSTAMP:",
        "PRODID:",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    assert!(!text.contains("VEVENT"), "{text}");
    // And it reads back the same.
    let back = parse_events(text.as_bytes()).unwrap();
    assert_eq!(back[0]["@type"], "Task");
    assert_eq!(back[0]["due"], "2026-11-06T00:00:00");
    assert_eq!(
        back[0]["relatedTo"]["web-parent@example.com"]["relation"],
        json!({"parent": true})
    );
}

#[test]
fn task_reminders_ring_relative_to_the_due() {
    // Thunderbird: 15 minutes before the due.
    let parsed = ParsedCalendar::parse(&file("task-thunderbird")).unwrap();
    let due = alerts_between(
        &parsed,
        madrid(),
        (utc("2026-11-01T00:00"), utc("2026-11-10T00:00")),
        10,
    )
    .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].fire_at, utc("2026-11-04T15:45"));
    assert_eq!(due[0].alert_id, "k1");

    // Apple: an absolute trigger.
    let apple = ParsedCalendar::parse(&file("task-apple-reminders")).unwrap();
    let due = alerts_between(
        &apple,
        madrid(),
        (utc("2026-11-01T00:00"), utc("2026-11-10T00:00")),
        10,
    )
    .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].fire_at, utc("2026-11-03T08:00"));

    // tasks.org: at 09:00 on each due day, except the occurrence done.
    let weekly = ParsedCalendar::parse(&file("task-tasksorg-recurring")).unwrap();
    let due = alerts_between(
        &weekly,
        madrid(),
        (utc("2026-10-19T00:00"), utc("2026-11-03T00:00")),
        10,
    )
    .unwrap();
    let times: Vec<i64> = due.iter().map(|a| a.fire_at).collect();
    assert_eq!(
        times,
        [utc("2026-10-19T07:00"), utc("2026-11-02T08:00")],
        "{due:?}"
    );
    assert!(due.iter().all(|a| a.recurrence.is_some()));

    // Done: no reminder.
    let mut edit = EditableEvent::open(&file("task-thunderbird")).unwrap();
    edit.event
        .insert("progress".to_owned(), Value::from("completed"));
    let done = ParsedCalendar::parse(edit.to_icalendar(NOW).unwrap().as_bytes()).unwrap();
    assert!(
        alerts_between(
            &done,
            madrid(),
            (utc("2026-11-01T00:00"), utc("2026-11-10T00:00")),
            10
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn estimate_and_progress_updated_survive() {
    // RFC 8984 §5.2.3 ↔ ESTIMATED-DURATION (RFC 10051 §10.1); §5.2.6 has
    // no iCalendar property: a JSPROP (draft-ietf-calext-jscalendar-
    // icalendar §4.1.2), for the task, an override and an assignee.
    let task = json!({
        "@type": "Task", "uid": "estimate@example.com", "title": "Informe",
        "start": "2026-11-02T09:00:00", "due": "2026-11-02T09:00:00",
        "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "weekly"},
        "estimatedDuration": "PT1H30M",
        "progress": "in-process", "progressUpdated": "2026-11-08T09:10:11Z",
        "recurrenceOverrides": {"2026-11-09T09:00:00": {"start": "2026-11-09T09:00:00",
            "progress": "completed", "progressUpdated": "2026-11-08T09:10:12Z"}},
        "participants": {"b": {"@type": "Participant",
            "calendarAddress": "mailto:b@example.com", "roles": {"attendee": true},
            "participationStatus": "accepted", "progress": "in-process",
            "progressUpdated": "2026-11-08T09:10:13Z"}}
    });
    let text = new_icalendar(task.as_object().unwrap(), NOW).unwrap();
    assert!(text.contains("ESTIMATED-DURATION:PT1H30M"), "{text}");
    for wanted in [
        "JSPROP;JSPTR=\"progressUpdated\":\"2026-11-08T09:10:11Z\"",
        "JSPROP;JSPTR=\"progressUpdated\":\"2026-11-08T09:10:12Z\"",
        "JSPROP;JSPTR=\"participants/b/progressUpdated\":\"2026-11-08T09:10:13Z\"",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    let back = parse_events(text.as_bytes()).unwrap();
    assert_eq!(back[0]["estimatedDuration"], "PT1H30M");
    assert_eq!(back[0]["progressUpdated"], "2026-11-08T09:10:11Z");
    assert_eq!(
        back[0]["recurrenceOverrides"]["2026-11-09T09:00:00"]["progressUpdated"],
        "2026-11-08T09:10:12Z"
    );
    assert_eq!(
        back[0]["participants"]["b"]["progressUpdated"],
        "2026-11-08T09:10:13Z"
    );
    // An edit elsewhere keeps them.
    let mut edit = EditableEvent::open(text.as_bytes()).unwrap();
    edit.event.insert("title".to_owned(), json!("Otro"));
    let text = edit.to_icalendar(NOW + 60).unwrap();
    assert_eq!(
        text.matches("ESTIMATED-DURATION:PT1H30M").count(),
        2,
        "{text}"
    );
    assert_eq!(
        text.matches("JSPTR=\"progressUpdated\"").count(),
        2,
        "{text}"
    );
}
