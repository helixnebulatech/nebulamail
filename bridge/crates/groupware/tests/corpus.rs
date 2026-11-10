// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The calendar corpus (`tests/data/corpus`, also used by the differential
//! test against Stalwart in `nebula-jmap`) through the JMAP model: every
//! file parses, expands, gives its stored event and each occurrence as an
//! event, and `CalendarEvent/parse` reads it.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    reason = "test helpers; y/m/d/h are the usual names of a date's parts"
)]

use std::path::PathBuf;

use nebula_groupware::model::calendar::{
    EventOptions, Instance, ParsedCalendar, Zone, parse_events,
};
use serde_json::Value;

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/corpus");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            // CRLF, as a CalDAV server sends it.
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace('\n', "\r\n");
            (name, text.into_bytes())
        })
        // Tasks (`task-*`) have their own tests (`tasks.rs`).
        .filter(|(name, _)| !name.starts_with("task-"))
        .collect();
    files.sort();
    files
}

fn file(name: &str) -> Vec<u8> {
    corpus().into_iter().find(|(n, _)| n == name).unwrap().1
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

fn in_window(instances: &[Instance], after: &str, before: &str) -> Vec<Instance> {
    let (after, before) = (utc(after), utc(before));
    instances
        .iter()
        .filter(|i| i.utc_start < before && i.utc_end > after)
        .copied()
        .collect()
}

#[test]
fn every_file_goes_through_the_model() {
    let madrid = Zone::named("Europe/Madrid").unwrap();
    let options = EventOptions {
        keep_unconverted: true,
        ..EventOptions::default()
    };
    for (name, bytes) in corpus() {
        let events = parse_events(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!events.is_empty(), "{name}: parse");
        let parsed = ParsedCalendar::parse(&bytes).unwrap();
        assert!(parsed.has_events(), "{name}");
        let instances = parsed.instances(madrid).unwrap();
        assert!(!instances.list.is_empty(), "{name}: no occurrences");
        let base = parsed.base_event(madrid, &options).unwrap().unwrap();
        assert!(base.members.contains_key("start"), "{name}: base");
        for instance in in_window(&instances.list, "2026-10-19T00:00", "2026-11-16T00:00") {
            let event = parsed
                .occurrence(instance.key, madrid, &options)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: occurrence"));
            assert_eq!(
                event.utc_range,
                Some((instance.utc_start, instance.utc_end))
            );
            assert!(event.members.get("recurrenceRule").is_none(), "{name}");
        }
    }
}

#[test]
fn series_with_exdate_and_override() {
    let parsed = ParsedCalendar::parse(&file("recurring-overrides")).unwrap();
    let instances = parsed.instances(Zone::UTC).unwrap();
    // Six weeks, one excluded.
    assert_eq!(instances.list.len(), 5);
    let november = in_window(&instances.list, "2026-11-02T00:00", "2026-11-09T00:00");
    assert_eq!(november.len(), 1);
    // Moved to 12:00 Madrid (11:00 UTC); its key is still 10:00.
    assert_eq!(november[0].utc_start, utc("2026-11-02T11:00"));
    assert_eq!(november[0].key.naive(), utc("2026-11-02T10:00"));
    let event = parsed
        .occurrence(november[0].key, Zone::UTC, &EventOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(event.members["title"], "Reunión semanal (movida)");
    assert_eq!(event.members["recurrenceId"], "2026-11-02T10:00:00");
}

#[test]
fn floating_follows_the_zone_and_long_series_reach_today() {
    let floating = ParsedCalendar::parse(&file("floating")).unwrap();
    let new_york = Zone::named("America/New_York").unwrap();
    let at = floating.instances(new_york).unwrap().list[0];
    assert_eq!(at.utc_start, utc("2026-11-03T14:00"));
    let event = floating
        .occurrence(at.key, new_york, &EventOptions::default())
        .unwrap()
        .unwrap();
    // As Stalwart: the occurrence comes in the query's zone.
    assert_eq!(event.members["timeZone"], "America/New_York");
    assert_eq!(event.members["start"], "2026-11-03T09:00:00");

    // Daily since 2015: more than 3000 occurrences before November 2026.
    let daily = ParsedCalendar::parse(&file("daily-old")).unwrap();
    let all = daily.instances(Zone::UTC).unwrap();
    assert_eq!(
        in_window(&all.list, "2026-11-02T00:00", "2026-11-09T00:00").len(),
        7
    );
}

#[test]
fn a_flight_ends_in_another_zone() {
    let parsed = ParsedCalendar::parse(&file("multi-tz")).unwrap();
    let at = parsed.instances(Zone::UTC).unwrap().list[0];
    assert_eq!(
        (at.utc_start, at.utc_end),
        (utc("2026-11-05T23:30"), utc("2026-11-06T06:30"))
    );
}

#[test]
fn parse_converts_a_date_recurrence_id() {
    let events = parse_events(&file("reply-allday-occurrence")).unwrap();
    let event = &events[0];
    assert_eq!(event["uid"], "nebula-corpus-allday-series");
    // As Stalwart: the web reads only `recurrenceId`.
    let text = serde_json::to_string(&Value::Object(event.clone())).unwrap();
    assert_eq!(
        event.get("recurrenceId"),
        Some(&Value::from("2026-11-30T00:00:00")),
        "{text}"
    );
}
