// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Smoke tests of the vendored calcard through our wrapper: iCalendar and
//! vCard (3.0, 4.0) to JSCalendar/JSContact and back, and expansion across
//! a DST change.

use nebula_groupware::ConvertError;
use nebula_groupware::model::calendar::{ParsedCalendar, Zone};
use nebula_groupware::model::convert::{
    Occurrence, VcardVersion, expand_icalendar, icalendar_index, icalendar_to_jscalendar,
    jscalendar_to_icalendar, jscontact_to_vcard, jscontact_to_vcard_as, timezone_id, vcard_index,
    vcard_to_jscontact,
};
use nebula_groupware::model::edit::new_icalendar;
use serde_json::{Value, json};

const WEEKLY: &[u8] = include_bytes!("data/weekly.ics");
const CARD_30: &[u8] = include_bytes!("data/card30.vcf");
const CARD_40: &[u8] = include_bytes!("data/card40.vcf");

/// The only event of a `Group` (the override stays inside its series).
fn only_entry(group: &Value) -> Option<&Value> {
    if group.get("@type") != Some(&json!("Group")) {
        return None;
    }
    match group
        .get("entries")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([event]) => Some(event),
        _ => None,
    }
}

#[test]
fn icalendar_to_jscalendar_and_back() {
    let group = icalendar_to_jscalendar(WEEKLY).unwrap();
    let event = only_entry(&group).unwrap();

    // JSCalendar "bis" names, as the web reads them (packages/jmap-client).
    assert_eq!(event["@type"], "Event");
    assert_eq!(event["uid"], "weekly-sync@example.com");
    assert_eq!(event["title"], "Reunión semanal");
    assert_eq!(event["start"], "2026-10-19T10:00:00");
    assert_eq!(event["timeZone"], "Europe/Madrid");
    assert_eq!(event["duration"], "PT30M");
    assert_eq!(
        event["recurrenceRule"],
        json!({"frequency": "weekly", "byDay": [{"day": "mo"}], "count": 4})
    );
    assert_eq!(event["organizerCalendarAddress"], "mailto:ana@example.com");
    let participants: Vec<&Value> = event["participants"]
        .as_object()
        .unwrap()
        .values()
        .collect();
    assert_eq!(participants.len(), 2);
    let luis = participants
        .iter()
        .find(|p| p["calendarAddress"] == "mailto:luis@example.com")
        .unwrap();
    assert_eq!(luis["participationStatus"], "needs-action");
    assert_eq!(luis["expectReply"], true);
    let alert = event["alerts"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(
        alert["trigger"],
        json!({"@type": "OffsetTrigger", "offset": "-PT15M"})
    );
    let moved = &event["recurrenceOverrides"]["2026-11-02T10:00:00"];
    assert_eq!(moved["start"], "2026-11-02T12:00:00");
    // Unknown properties are kept for the way back.
    assert_eq!(
        event["iCalendar"]["properties"],
        json!([["x-nebula-test", {}, "unknown", "valor desconocido"]])
    );
    // The VTIMEZONE stays on the group, also for the way back.
    assert_eq!(group["iCalendar"]["components"][0][0], "vtimezone");

    let ics = jscalendar_to_icalendar(&group.to_string()).unwrap();
    for line in [
        "RRULE:FREQ=WEEKLY;COUNT=4;BYDAY=MO",
        "DTSTART;TZID=Europe/Madrid:20261019T100000",
        "RECURRENCE-ID;TZID=Europe/Madrid:20261102T100000",
        "TRIGGER:-PT15M",
        "X-NEBULA-TEST:valor desconocido",
        "TZID:Europe/Madrid",
    ] {
        assert!(ics.contains(line), "missing {line:?} in\n{ics}");
    }
    // And the same JSCalendar again: nothing lost in the round trip.
    let again = icalendar_to_jscalendar(ics.as_bytes()).unwrap();
    let again_event = only_entry(&again).unwrap();
    for key in [
        "uid",
        "title",
        "start",
        "timeZone",
        "duration",
        "recurrenceRule",
        "recurrenceOverrides",
        "participants",
        "organizerCalendarAddress",
    ] {
        assert_eq!(again_event[key], event[key], "{key} changed");
    }
}

#[test]
fn expansion_follows_the_time_zone_and_overrides() {
    let expansion = expand_icalendar(WEEKLY, 100).unwrap();
    assert!(expansion.complete);
    let starts: Vec<i64> = expansion
        .occurrences
        .iter()
        .map(|o: &Occurrence| o.utc_start)
        .collect();
    assert_eq!(
        starts,
        [
            1_792_396_800, // 2026-10-19 08:00Z (CEST, +02)
            1_793_005_200, // 2026-10-26 09:00Z (CET after 25 Oct, +01)
            1_793_617_200, // 2026-11-02 11:00Z: the override, moved to 12:00
            1_794_214_800, // 2026-11-09 09:00Z
        ]
    );
    for occurrence in &expansion.occurrences {
        assert_eq!(occurrence.utc_end - occurrence.utc_start, 30 * 60);
    }

    let capped = expand_icalendar(WEEKLY, 2).unwrap();
    assert_eq!(capped.occurrences.len(), 2);
}

#[test]
fn an_endless_rule_stops_at_the_limit() {
    let ics = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nDTSTART:20260101T000000Z\r\n\
RRULE:FREQ=SECONDLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let expansion = expand_icalendar(ics, usize::MAX).unwrap();
    assert_eq!(expansion.occurrences.len(), 10_000);
    // A rule that never matches ends too (calcard caps its own work).
    let never = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:y\r\nDTSTART:20260101T000000Z\r\n\
RRULE:FREQ=SECONDLY;BYMONTH=2;BYMONTHDAY=30\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let expansion = expand_icalendar(never, 100).unwrap();
    assert!(expansion.occurrences.is_empty());
}

#[test]
fn vcard_30_to_jscontact_and_back() {
    let card = vcard_to_jscontact(CARD_30).unwrap();
    assert_eq!(card["@type"], "Card");
    assert_eq!(card["name"]["full"], "Lucía García");
    let email = card["emails"].as_object().unwrap().values().next().unwrap();
    assert_eq!(email["address"], "lucia@example.com");
    assert_eq!(email["contexts"]["work"], true);
    let birthday = card["anniversaries"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(birthday["kind"], "birth");
    assert_eq!(
        birthday["date"],
        json!({"@type": "PartialDate", "year": 1990, "month": 5, "day": 17})
    );
    assert_eq!(card["keywords"], json!({"Trabajo": true, "Amigos": true}));

    // Written back as 3.0: the original version is kept in `vCard`.
    let vcf = jscontact_to_vcard(&card.to_string()).unwrap();
    assert!(vcf.contains("VERSION:3.0"), "{vcf}");
    assert!(
        vcf.contains("EMAIL;TYPE=INTERNET,WORK:lucia@example.com"),
        "{vcf}"
    );
    assert!(vcf.contains("NOTE:Contacto de prueba\\, con coma"), "{vcf}");
    assert_eq!(vcard_to_jscontact(vcf.as_bytes()).unwrap(), card);
}

#[test]
fn vcard_40_to_jscontact_and_back() {
    let card = vcard_to_jscontact(CARD_40).unwrap();
    assert_eq!(card["uid"], "urn:uuid:4fbe8971-0bc3-424c-9c26-36c3e1eff6b1");
    assert_eq!(card["kind"], "individual");
    let phone = card["phones"].as_object().unwrap().values().next().unwrap();
    assert_eq!(phone["number"], "tel:+34-600-111-222");
    assert_eq!(phone["features"], json!({"mobile": true, "voice": true}));
    assert_eq!(
        card["vCard"]["properties"][1],
        json!(["x-nebula-test", {}, "unknown", "desconocido"])
    );

    let vcf = jscontact_to_vcard(&card.to_string()).unwrap();
    assert!(vcf.contains("VERSION:4.0"), "{vcf}");
    assert!(vcf.contains("X-NEBULA-TEST:desconocido"), "{vcf}");
    // calcard writes the name order as `N;JSCOMPS=...`, read back as
    // `isOrdered`; nothing else changes, and from then on it is stable.
    let mut again = vcard_to_jscontact(vcf.as_bytes()).unwrap();
    assert_eq!(again["name"]["isOrdered"], true);
    let third = vcard_to_jscontact(jscontact_to_vcard(&again.to_string()).unwrap().as_bytes());
    assert_eq!(third.unwrap(), again);
    again["name"].as_object_mut().unwrap().remove("isOrdered");
    assert_eq!(again, card);
}

#[test]
fn event_indexes() {
    let index = icalendar_index(WEEKLY, 1000).unwrap();
    assert_eq!(index.uid.as_deref(), Some("weekly-sync@example.com"));
    assert!(index.recurring);
    let all = expand_icalendar(WEEKLY, 1000).unwrap().occurrences;
    assert_eq!(index.utc_start, all.iter().map(|o| o.utc_start).min());
    assert_eq!(index.utc_end, all.iter().map(|o| o.utc_end).max());

    // An endless rule is open-ended; so is one past the limit.
    let endless = String::from_utf8(WEEKLY.to_vec())
        .unwrap()
        .replace("RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4", "RRULE:FREQ=DAILY");
    let index = icalendar_index(endless.as_bytes(), 1000).unwrap();
    assert!(index.utc_start.is_some());
    assert_eq!(index.utc_end, None);
    assert_eq!(icalendar_index(WEEKLY, 2).unwrap().utc_end, None);

    // A single event: its own start and end.
    let single = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:one\r\n\
        DTSTAMP:20261027T120000Z\r\nDTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let index = icalendar_index(single, 1000).unwrap();
    assert!(!index.recurring);
    assert_eq!(index.utc_start, Some(1_793_181_600));
    assert_eq!(index.utc_end, Some(1_793_185_200));
    // No event, task or journal: not a calendar object.
    let empty = b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nEND:VCALENDAR\r\n";
    assert!(icalendar_index(empty, 10).is_err());
}

#[test]
fn card_indexes() {
    let index = vcard_index(CARD_30).unwrap();
    assert_eq!(index.sort_name.as_deref(), Some("garcía lucía"));
    let only_fn = b"BEGIN:VCARD\r\nVERSION:4.0\r\nUID:x1\r\nFN:  Nebula   Team \r\nEND:VCARD\r\n";
    let index = vcard_index(only_fn).unwrap();
    assert_eq!(index.uid.as_deref(), Some("x1"));
    assert_eq!(index.sort_name.as_deref(), Some("nebula team"));
    assert!(vcard_index(b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").is_err());

    let tz = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\n\
        BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n\
        END:VTIMEZONE\r\nEND:VCALENDAR\r\n";
    assert_eq!(timezone_id(tz).as_deref(), Some("Europe/Madrid"));
    assert_eq!(timezone_id("garbage"), None);
}

/// A name taken from JSON (property, group, parameter or component) with a
/// line break would start a new line in the written text, e.g. a second
/// `VCARD` or `VEVENT` (calcard patch, `vendor/calcard/PATCHES.md`): the
/// export fails instead.
#[test]
fn names_with_line_breaks_are_refused() {
    let card = |vcard: Value| {
        json!({"@type": "Card", "version": "1.0", "uid": "u1", "kind": "individual",
            "name": {"full": "A"}, "vCard": vcard})
        .to_string()
    };
    for vcard in [
        json!({"properties": [["x-a:b\r\nEND:VCARD\r\nBEGIN:VCARD\r\nX-B", {}, "text", "v"]]}),
        json!({"properties": [["x-a", {"group": "g\r\nEND:VCARD\r\nBEGIN:VCARD\r\nX-B"}, "text", "v"]]}),
        json!({"properties": [["x-a", {"x-p\r\nEND:VCARD\r\nBEGIN:VCARD\r\nX-B": "1"}, "text", "v"]]}),
    ] {
        for version in [VcardVersion::V3, VcardVersion::V4] {
            let result = jscontact_to_vcard_as(&card(vcard.clone()), version);
            assert!(
                matches!(result, Err(ConvertError::Export { .. })),
                "{vcard} {result:?}"
            );
        }
    }
    // An ordinary unknown property still goes through.
    let plain = card(json!({"properties": [["x-a", {"group": "item1"}, "text", "v"]]}));
    let text = jscontact_to_vcard_as(&plain, VcardVersion::V4).unwrap();
    assert!(text.contains("item1.X-A:v"), "{text}");

    let group = |event: Value, components: Value| {
        let mut event = event;
        event["@type"] = json!("Event");
        event["uid"] = json!("e1");
        event["start"] = json!("2026-11-02T10:00:00");
        json!({"@type": "Group", "entries": [event], "iCalendar": {"components": components}})
    };
    let cases = [
        group(
            json!({"iCalendar": {"properties": [["x-a\r\nEND:VEVENT\r\nBEGIN:VEVENT", {}, "unknown", "v"]]}}),
            json!([]),
        ),
        group(
            json!({"iCalendar": {"properties": [["x-a", {"x-p\r\nATTENDEE": "1"}, "unknown", "v"]]}}),
            json!([]),
        ),
        group(
            json!({}),
            json!([["X-A\r\nEND:VEVENT\r\nBEGIN:VEVENT", [], []]]),
        ),
    ];
    for case in cases {
        let result = jscalendar_to_icalendar(&case.to_string());
        assert!(
            matches!(result, Err(ConvertError::Export { .. })),
            "{case} {result:?}"
        );
        // The path of `CalendarEvent/set` (`edit::new_icalendar`) too.
        let Value::Object(event) = case["entries"][0].clone() else {
            unreachable!()
        };
        if case["iCalendar"]["components"] == json!([]) {
            let result = new_icalendar(&event, 1_790_000_000);
            assert!(
                matches!(result, Err(ConvertError::Export { .. })),
                "{case} {result:?}"
            );
        }
    }
}

fn daily(rule: &str, extra: &str) -> Vec<u8> {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:d\r\n\
         DTSTAMP:20261001T000000Z\r\nDTSTART:20261019T100000Z\r\nDURATION:PT1H\r\n\
         RRULE:{rule}\r\n{extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    .into_bytes()
}

/// An `EXDATE` uses up calcard's limit without an instance: an endless
/// series with one is still endless (it used to be indexed with an end).
#[test]
fn an_excluded_date_does_not_end_a_series() {
    let endless = daily("FREQ=DAILY", "EXDATE:20261020T100000Z\r\n");
    let index = icalendar_index(&endless, 10).unwrap();
    assert_eq!(index.utc_end, None);
    assert_eq!(index.utc_start, Some(1_792_404_000));
    assert!(!expand_icalendar(&endless, 10).unwrap().complete);
    let parsed = ParsedCalendar::parse(&endless).unwrap();
    assert!(!parsed.instances(Zone::UTC).unwrap().complete);

    // Five days, one excluded: complete, ends on the fifth.
    let five = daily("FREQ=DAILY;COUNT=5", "EXDATE:20261020T100000Z\r\n");
    let index = icalendar_index(&five, 10).unwrap();
    assert_eq!(index.utc_end, Some(1_792_404_000 + 4 * 86_400 + 3600));
    let expansion = expand_icalendar(&five, 10).unwrap();
    assert!(expansion.complete);
    assert_eq!(expansion.occurrences.len(), 4);
    let parsed = ParsedCalendar::parse(&five).unwrap();
    assert!(parsed.instances(Zone::UTC).unwrap().complete);
}

/// RDATEs come on top of the limit, and the cut drops them: not complete.
#[test]
fn a_cut_expansion_is_not_complete() {
    let more = daily(
        "FREQ=DAILY;COUNT=3",
        "RDATE:20261201T100000Z\r\nRDATE:20261202T100000Z\r\n",
    );
    let expansion = expand_icalendar(&more, 3).unwrap();
    assert_eq!(expansion.occurrences.len(), 3);
    assert!(!expansion.complete);
    assert!(expand_icalendar(&more, 10).unwrap().complete);
}

/// calcard's patch (vendor/calcard/PATCHES.md): `truncated` says whether a
/// rule used up the limit.
#[test]
fn calcards_truncated_flag() {
    let text = String::from_utf8(daily("FREQ=DAILY;COUNT=3", "")).unwrap();
    let ical = calcard::icalendar::ICalendar::parse(&text).unwrap();
    assert!(
        ical.expand_dates(calcard::common::timezone::Tz::UTC, 2)
            .truncated
    );
    assert!(
        !ical
            .expand_dates(calcard::common::timezone::Tz::UTC, 10)
            .truncated
    );
}

/// RFC 5545 §3.3.6: `dur-value = "P" (dur-date / dur-time / dur-week)`, so
/// a week never goes with days or a time; RFC 8984 §1.4.6 allows `P1W2D`.
/// Weeks and days are both nominal (RFC 5545 §3.3.6, RFC 8984 §1.4.6: "a
/// week is always the same as seven days"), so `P1W2D` is written `P9D`.
#[test]
fn weeks_with_days_are_written_as_days() {
    let task = json!({
        "@type": "Task", "uid": "t-weeks", "title": "x",
        "start": "2026-11-09T09:00:00", "timeZone": "Europe/Madrid",
        "duration": "P1W2D",
        "estimatedDuration": "P1WT2H",
        "alerts": {"a": {"@type": "Alert", "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-P2WT30M"}}}
    });
    let ics = jscalendar_to_icalendar(&task.to_string()).unwrap();
    for line in [
        "DURATION:P9D\r\n",
        "ESTIMATED-DURATION:P7DT2H\r\n",
        "TRIGGER:-P14DT30M\r\n",
    ] {
        assert!(ics.contains(line), "missing {line:?} in\n{ics}");
    }
    // A week alone stays a week (valid in both).
    let weeks = json!({"@type": "Task", "uid": "t-w", "start": "2026-11-09T09:00:00",
        "estimatedDuration": "P2W"});
    let ics = jscalendar_to_icalendar(&weeks.to_string()).unwrap();
    assert!(ics.contains("ESTIMATED-DURATION:P2W\r\n"), "{ics}");
    // Back to JSCalendar: the same lengths, valid there too.
    let back = icalendar_to_jscalendar(
        jscalendar_to_icalendar(&task.to_string())
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let back = only_entry(&back).unwrap();
    assert_eq!(back["duration"], "P9D");
    assert_eq!(back["estimatedDuration"], "P7DT2H");
}

/// RFC 8984 §5.2.3: `estimatedDuration` is a Task property, and RFC 10051
/// §10.1 allows `ESTIMATED-DURATION` only in a `VTODO`. On an Event it has
/// no iCalendar mapping, so it goes as `JSPROP`
/// (draft-ietf-calext-jscalendar-icalendar §4.1.2: "Other properties MAY
/// be converted to JSPROP properties") and comes back the same.
#[test]
fn estimated_duration_on_an_event_is_a_jsprop() {
    let event = json!({
        "@type": "Event", "uid": "e-est", "title": "x",
        "start": "2026-11-09T09:00:00", "timeZone": "Europe/Madrid",
        "duration": "PT1H", "estimatedDuration": "PT30M"
    });
    let ics = jscalendar_to_icalendar(&event.to_string()).unwrap();
    assert!(!ics.contains("ESTIMATED-DURATION"), "{ics}");
    assert!(ics.contains("JSPROP"), "{ics}");
    let back = icalendar_to_jscalendar(ics.as_bytes()).unwrap();
    assert_eq!(only_entry(&back).unwrap()["estimatedDuration"], "PT30M");
}

/// The way back: iCalendar → JSCalendar gives valid JSCalendar. A lenient
/// `P1W2D` reads as `P9D`; a negative `DURATION` or `ESTIMATED-DURATION`
/// (RFC 5545 §3.8.2.5 and RFC 10051 §10.1 want a positive one; a
/// JSCalendar Duration has no sign, RFC 8984 §1.4.6) is not converted and
/// stays in `iCalendar.properties`, written back as it was.
#[test]
fn icalendar_durations_read_as_valid_jscalendar() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\n\
        BEGIN:VTODO\r\nUID:p\r\nDTSTAMP:20261108T100000Z\r\nDTSTART:20261109T090000Z\r\n\
        DURATION:-PT1H\r\nESTIMATED-DURATION:-P1D\r\n\
        BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:x\r\nTRIGGER:-P1W2D\r\nEND:VALARM\r\n\
        END:VTODO\r\n\
        BEGIN:VEVENT\r\nUID:q\r\nDTSTAMP:20261108T100000Z\r\nDTSTART:20261109T090000Z\r\n\
        DURATION:P1WT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let group = icalendar_to_jscalendar(ics.as_bytes()).unwrap();
    let entries = group["entries"].as_array().unwrap();
    let task = entries.iter().find(|e| e["uid"] == "p").unwrap();
    let event = entries.iter().find(|e| e["uid"] == "q").unwrap();
    assert_eq!(event["duration"], "P7DT1H");
    assert!(task.get("duration").is_none(), "{task}");
    assert!(task.get("estimatedDuration").is_none(), "{task}");
    let alert = task["alerts"].as_object().unwrap().values().next().unwrap();
    assert_eq!(alert["trigger"]["offset"], "-P9D");
    let back = jscalendar_to_icalendar(&group.to_string()).unwrap();
    for line in [
        "DURATION:-PT1H\r\n",
        "ESTIMATED-DURATION:-P1D\r\n",
        "TRIGGER:-P9D\r\n",
        "DURATION:P7DT1H\r\n",
    ] {
        assert!(back.contains(line), "missing {line:?} in\n{back}");
    }
}
