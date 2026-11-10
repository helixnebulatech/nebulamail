// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Tasks (`VTODO`, JSCalendar `Task`) through the
//! bridge against a real CalDAV server: Stalwart's DAV or Radicale.
//! Skipped unless `NEBULA_TEST_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test server tasks -- --test-threads=1
//! ```
//!
//! What the web's Tasks app does, with the bridge's capability in `using`:
//! a task list (`components: ["VTODO"]`), a task due on a day with
//! priority, keywords and a reminder, a subtask, completing and reopening,
//! a weekly task with one occurrence completed, an undated task, a VTODO
//! another client wrote (Apple Reminders style) seen and edited, and the
//! deletes. Each step is checked on the DAV server with the DAV client as
//! another client. No mail is sent (none of these tasks has participants;
//! assigning one is `calendar_set::task_assignment_round_trip`).

use nebula_dav::proto::{Component, ResourceName};
use nebula_dav::{PutCondition, ResourceKind};
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{DavTestServer, User};
use serde_json::{Value, json};

use super::calendar_set::{
    backend, calendar_href, created, dav_text, event_href, mail_server, other_client, unfolded,
    unique, wait_for_calendars,
};

const COMPONENTS: &str = "https://helixnebula.tech/jmap/bridge/components";

/// One request with what the web's Tasks app puts in `using` (the
/// bridge's capability or not).
async fn call_with(
    backend: &JmapBackend,
    account: &AccountContext,
    bridge: bool,
    calls: Value,
) -> Vec<Value> {
    let mut using = vec![
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:calendars",
    ];
    if bridge {
        using.push("https://helixnebula.tech/jmap/bridge");
    }
    let body = json!({"using": using, "methodCalls": calls});
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        &[Capability::Core, Capability::Calendars, Capability::Bridge],
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                // Push subscriptions are per session; nothing else looks at it.
                session: nebula_store::GrantId::from_raw(1).unwrap(),
                session_state: State::new("s"),
                languages: nebula_http::AcceptLanguage::default(),
                limits: MethodLimits {
                    max_objects_in_get: 500,
                    max_objects_in_set: 500,
                    max_size_request: 10_000_000,
                },
            },
        )
        .await
        .unwrap();
    let value = serde_json::to_value(response).unwrap();
    value["methodResponses"].as_array().unwrap().clone()
}

async fn one(backend: &JmapBackend, account: &AccountContext, method: &str, args: Value) -> Value {
    let mut args = args;
    args["accountId"] = Value::from(account.jmap_id.as_str());
    let responses = call_with(backend, account, true, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], method, "{responses:?}");
    responses[0][1].clone()
}

/// The tasks of a list due between two days (Madrid), expanded, with what
/// the Tasks app reads.
async fn due_between(
    backend: &JmapBackend,
    account: &AccountContext,
    list: &str,
    (after, before): (&str, &str),
) -> Vec<Value> {
    let a = account.jmap_id.as_str();
    let responses = call_with(
        backend,
        account,
        true,
        json!([
            ["CalendarEvent/query", {"accountId": a, "expandRecurrences": true,
                "filter": {"after": after, "before": before, "inCalendar": list},
                "sort": [{"property": "start"}], "timeZone": "Europe/Madrid"}, "0"],
            ["CalendarEvent/get", {"#ids": {"name": "CalendarEvent/query", "path": "/ids", "resultOf": "0"},
                "accountId": a, "timeZone": "Europe/Madrid",
                "properties": ["@type", "title", "due", "start", "progress", "baseEventId",
                               "recurrenceId", "showWithoutTime", "utcStart"]}, "1"]
        ]),
    )
    .await;
    responses[1][1]["list"].as_array().unwrap().clone()
}

/// Every task of a list (the Tasks app's "Todas"): no dates.
async fn all_in(backend: &JmapBackend, account: &AccountContext, list: &str) -> Vec<Value> {
    let a = account.jmap_id.as_str();
    let responses = call_with(
        backend,
        account,
        true,
        json!([
            ["CalendarEvent/query", {"accountId": a, "filter": {"inCalendar": list}}, "0"],
            ["CalendarEvent/get", {"#ids": {"name": "CalendarEvent/query", "path": "/ids", "resultOf": "0"},
                "accountId": a, "properties": ["@type", "title", "due", "progress", "relatedTo"]}, "1"]
        ]),
    )
    .await;
    responses[1][1]["list"].as_array().unwrap().clone()
}

fn titles(list: &[Value]) -> Vec<&str> {
    list.iter().map(|t| t["title"].as_str().unwrap()).collect()
}

/// An Apple Reminders VTODO, as another client puts it.
fn apple_reminder(uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//iOS 18.1//EN\r\n\
         BEGIN:VTODO\r\nUID:{uid}\r\nDTSTAMP:20261020T081512Z\r\nCREATED:20261020T081500Z\r\n\
         SUMMARY:De Apple\r\nDUE;VALUE=DATE:20261112\r\nSTATUS:NEEDS-ACTION\r\nPRIORITY:9\r\n\
         CLASS:PRIVATE\r\nX-APPLE-SORT-ORDER:745834512\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
    )
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one session of the Tasks app, step by step"
)]
async fn tasks_like_the_web() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let alice = super::start(&backend, &store, mail_server(dav), User::Alice).await;
    let a = alice.jmap_id.as_str();
    let other = other_client(dav, User::Alice);
    wait_for_calendars(&backend, &alice).await;

    // The lists: every calendar says what it takes; some list takes tasks
    // (Stalwart's default calendar, or the "Tasks" the bridge made for a
    // new Radicale user).
    // The bridge makes that one after its first round: wait for it.
    let takes_tasks = |c: &Value| c[COMPONENTS].as_array().unwrap().contains(&json!("VTODO"));
    let mut lists = Value::Null;
    for _ in 0..100 {
        lists = one(&backend, &alice, "Calendar/get", json!({})).await;
        if lists["list"].as_array().unwrap().iter().any(takes_tasks) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let all = lists["list"].as_array().unwrap();
    assert!(all.iter().all(|c| c[COMPONENTS].is_array()), "{lists}");
    assert!(all.iter().any(takes_tasks), "{lists}");
    eprintln!(
        "{}: lists {:?}",
        dav.kind,
        all.iter()
            .map(|c| (c["name"].clone(), c[COMPONENTS].clone()))
            .collect::<Vec<_>>()
    );
    // Without the capability the vendor property is neither sent nor
    // accepted.
    let plain = call_with(
        &backend,
        &alice,
        false,
        json!([["Calendar/get", {"accountId": a}, "0"],
               ["Calendar/set", {"accountId": a, "create": {"x": {"name": "x", COMPONENTS: ["VTODO"]}}}, "1"]]),
    )
    .await;
    assert!(
        plain[0][1]["list"][0].get(COMPONENTS).is_none(),
        "{plain:?}"
    );
    assert_eq!(
        plain[1][1]["notCreated"]["x"]["type"], "invalidProperties",
        "{plain:?}"
    );

    // A task list of its own (VTODO only on the server).
    let name = unique("tasks");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"create": {"l": {"name": name, "color": "#3366cc", COMPONENTS: ["VTODO"]}}}),
    )
    .await;
    let list = created(&set, "l");
    assert_eq!(set["created"]["l"][COMPONENTS], json!(["VTODO"]), "{set}");
    let home = &other.discover().await.unwrap().calendar_homes[0];
    let remote = other.list_collections(home).await.unwrap();
    let found = remote
        .iter()
        .find(|c| c.display_name.as_deref() == Some(name.as_str()))
        .expect("the list on the DAV server");
    let components = found.components.as_ref().expect("its component set");
    assert!(components.contains(Component::Todo), "{components:?}");
    assert!(!components.contains(Component::Event), "{components:?}");

    // A task due on a day, with priority, keywords, notes and a reminder;
    // and a subtask of it.
    let parent_uid = unique("parent");
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"create": {"t": {
            "@type": "Task", "uid": parent_uid, "calendarIds": {list.clone(): true},
            "title": "Preparar la mudanza", "description": "Cajas y cinta",
            "due": "2026-11-05T00:00:00", "showWithoutTime": true, "priority": 1,
            "keywords": {"casa": true}, "progress": "needs-action",
            "alerts": {"a1": {"@type": "Alert", "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15H", "relativeTo": "end"}}}
        }}, "sendSchedulingMessages": true}),
    )
    .await;
    let parent = created(&set, "t");
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"create": {"s": {
            "@type": "Task", "calendarIds": {list.clone(): true}, "title": "Comprar cajas",
            "relatedTo": {parent_uid.clone(): {"@type": "Relation", "relation": {"parent": true}}}
        }}}),
    )
    .await;
    let child = created(&set, "s");
    assert!(set["created"]["s"]["uid"].is_string(), "{set}");
    let parent_href = event_href(&store, &alice, &parent).await;
    let child_href = event_href(&store, &alice, &child).await;
    let text = unfolded(&dav_text(&other, &parent_href).await.unwrap());
    for line in [
        "BEGIN:VTODO",
        "DUE;VALUE=DATE:20261105",
        "PRIORITY:1",
        "CATEGORIES:casa",
        "STATUS:NEEDS-ACTION",
        "TRIGGER;RELATED=END:-PT15H",
    ] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    assert!(!text.contains("COMPLETED:"), "{text}");
    let text = unfolded(&dav_text(&other, &child_href).await.unwrap());
    assert!(
        text.contains(&format!("RELATED-TO;RELTYPE=PARENT:{parent_uid}")),
        "{text}"
    );

    // Today/Upcoming: the task is on its due day, not before; the
    // subtask (no date) only without dates.
    let week = due_between(
        &backend,
        &alice,
        &list,
        ("2026-11-02T00:00:00", "2026-11-09T00:00:00"),
    )
    .await;
    assert_eq!(titles(&week), ["Preparar la mudanza"], "{week:?}");
    assert_eq!(week[0]["@type"], "Task");
    assert_eq!(week[0]["due"], "2026-11-05T00:00:00");
    assert_eq!(week[0]["showWithoutTime"], true);
    let day_before = due_between(
        &backend,
        &alice,
        &list,
        ("2026-11-04T00:00:00", "2026-11-05T00:00:00"),
    )
    .await;
    assert!(day_before.is_empty(), "{day_before:?}");
    let everything = all_in(&backend, &alice, &list).await;
    let mut names = titles(&everything);
    names.sort_unstable();
    assert_eq!(names, ["Comprar cajas", "Preparar la mudanza"]);

    // Complete the subtask and the task (x / Space), then reopen the task.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {child.clone(): {"progress": "completed"},
                          parent.clone(): {"progress": "completed"}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    for href in [&parent_href, &child_href] {
        let text = dav_text(&other, href).await.unwrap();
        assert!(text.contains("STATUS:COMPLETED"), "{text}");
        assert!(text.contains("COMPLETED:20"), "{text}");
        assert!(text.contains("PERCENT-COMPLETE:100"), "{text}");
    }
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {parent.clone(): {"progress": "needs-action"}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = dav_text(&other, &parent_href).await.unwrap();
    assert!(text.contains("STATUS:NEEDS-ACTION"), "{text}");
    assert!(!text.contains("COMPLETED:20"), "{text}");
    assert!(!text.contains("PERCENT-COMPLETE"), "{text}");
    let text = dav_text(&other, &child_href).await.unwrap();
    assert!(text.contains("STATUS:COMPLETED"), "{text}");

    // A weekly task (start = due, as the web writes a repeating task):
    // each occurrence is completed on its own.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"create": {"w": {
            "@type": "Task", "calendarIds": {list.clone(): true}, "title": "Regar las plantas",
            "start": "2026-11-02T09:00:00", "due": "2026-11-02T09:00:00",
            "timeZone": "Europe/Madrid", "showWithoutTime": false,
            "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "weekly", "count": 4}
        }}}),
    )
    .await;
    let weekly = created(&set, "w");
    let weekly_href = event_href(&store, &alice, &weekly).await;
    let month = due_between(
        &backend,
        &alice,
        &list,
        ("2026-11-02T00:00:00", "2026-11-30T00:00:00"),
    )
    .await;
    let plants: Vec<&Value> = month
        .iter()
        .filter(|t| t["title"] == "Regar las plantas")
        .collect();
    assert_eq!(plants.len(), 4, "{month:?}");
    assert!(plants.iter().all(|t| t["baseEventId"] == weekly.as_str()));
    let second = plants[1]["id"].as_str().unwrap().to_owned();
    assert_eq!(plants[1]["recurrenceId"], "2026-11-09T09:00:00");
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {second.clone(): {"progress": "completed"}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &weekly_href).await.unwrap());
    assert_eq!(text.matches("BEGIN:VTODO").count(), 2, "{text}");
    assert!(
        text.contains("RECURRENCE-ID;TZID=Europe/Madrid:20261109T090000"),
        "{text}"
    );
    assert!(text.contains("RRULE:FREQ=WEEKLY;COUNT=4"), "{text}");
    assert_eq!(text.matches("STATUS:COMPLETED").count(), 1, "{text}");
    assert_eq!(text.matches("COMPLETED:20").count(), 1, "{text}");
    let month = due_between(
        &backend,
        &alice,
        &list,
        ("2026-11-02T00:00:00", "2026-11-30T00:00:00"),
    )
    .await;
    let progress: Vec<&str> = month
        .iter()
        .filter(|t| t["title"] == "Regar las plantas")
        .map(|t| t["progress"].as_str().unwrap_or("needs-action"))
        .collect();
    assert_eq!(
        progress,
        ["needs-action", "completed", "needs-action", "needs-action"],
        "{month:?}"
    );

    // A reminder written by another client (Apple style) is seen; an edit
    // through the bridge keeps what the web does not know.
    let apple_uid = unique("apple");
    let apple_href = calendar_href(&store, &alice, &list)
        .await
        .member(
            &ResourceName::try_from(format!("{apple_uid}.ics").as_str()).unwrap(),
            false,
        )
        .unwrap();
    other
        .put(
            &apple_href,
            ResourceKind::Calendar,
            &apple_reminder(&apple_uid),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    // Another list makes the bridge list again (a write kicks a round).
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"create": {"l2": {"name": format!("{name}-2"), COMPONENTS: ["VTODO", "VEVENT"]}}}),
    )
    .await;
    let second_list = created(&set, "l2");
    assert_eq!(
        set["created"]["l2"][COMPONENTS],
        json!(["VEVENT", "VTODO"]),
        "{set}"
    );
    let found = one(
        &backend,
        &alice,
        "CalendarEvent/query",
        json!({"filter": {"uid": apple_uid}}),
    )
    .await;
    let apple = found["ids"][0]
        .as_str()
        .expect("the Apple reminder seen")
        .to_owned();
    let got = one(
        &backend,
        &alice,
        "CalendarEvent/get",
        json!({"ids": [apple.clone()], "properties": ["@type", "title", "due", "priority", "privacy", "iCalendar"]}),
    )
    .await;
    let reminder = &got["list"][0];
    assert_eq!(reminder["@type"], "Task", "{got}");
    assert_eq!(reminder["title"], "De Apple");
    assert_eq!(reminder["due"], "2026-11-12T00:00:00");
    assert_eq!(reminder["priority"], 9);
    let unknown = reminder["iCalendar"].to_string();
    assert!(unknown.contains("x-apple-sort-order"), "{got}");
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {apple.clone(): {"title": "De Apple (editada)", "progress": "completed"}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &apple_href).await.unwrap());
    for line in [
        "SUMMARY:De Apple (editada)",
        "X-APPLE-SORT-ORDER:745834512",
        "CLASS:PRIVATE",
        "PRIORITY:9",
        "STATUS:COMPLETED",
        "PERCENT-COMPLETE:100",
    ] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }

    // Deletes: one task, then the lists with what is left.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"destroy": [child.clone()]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([child]), "{set}");
    assert!(dav_text(&other, &child_href).await.is_none());
    let gone = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"destroy": [list.clone(), second_list.clone()], "onDestroyRemoveEvents": true}),
    )
    .await;
    assert_eq!(gone["destroyed"].as_array().unwrap().len(), 2, "{gone}");
    let remote = other.list_collections(home).await.unwrap();
    assert!(!remote.iter().any(|c| {
        c.display_name
            .as_deref()
            .is_some_and(|n| n.starts_with(&name))
    }));
}

/// Whether `text` has a `JSPROP` for `pointer` with this JSON `value`;
/// the parameter quoted or not (Radicale drops quotes it doesn't need,
/// RFC 5545 §3.1).
fn has_jsprop(text: &str, pointer: &str, value: &str) -> bool {
    text.contains(&format!("JSPROP;JSPTR=\"{pointer}\":{value}"))
        || text.contains(&format!("JSPROP;JSPTR={pointer}:{value}"))
}

/// RFC 8984 §5.2.3 `estimatedDuration` and §5.2.6 `progressUpdated` as the
/// web writes them: the estimate is `ESTIMATED-DURATION` (RFC 10051
/// §10.1; draft-ietf-calext-jscalendar-icalendar §2.3.19), and
/// `progressUpdated` (no iCalendar property) a `JSPROP` (that draft,
/// §4.1.2), for the task, one occurrence's override and an assignee.
/// Both come back from the server, are patched, and the estimate goes.
#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one task through every place the two properties live"
)]
async fn estimate_and_progress_updated_round_trip() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let alice = super::start(&backend, &store, mail_server(dav), User::Alice).await;
    let other = other_client(dav, User::Alice);
    wait_for_calendars(&backend, &alice).await;
    let name = unique("estimate");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"create": {"l": {"name": name, COMPONENTS: ["VTODO"]}}}),
    )
    .await;
    let list = created(&set, "l");

    // A weekly task with an estimate, its progress dated, one occurrence
    // done (dated in its override) and an assignee with a dated progress.
    // No `sendSchedulingMessages`: nobody is mailed.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"create": {"t": {
            "@type": "Task", "calendarIds": {list.clone(): true}, "title": "Informe",
            "start": "2026-11-02T09:00:00", "due": "2026-11-02T09:00:00",
            "timeZone": "Europe/Madrid",
            "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "weekly", "count": 4},
            "estimatedDuration": "PT1H30M",
            "progress": "in-process", "progressUpdated": "2026-11-08T09:10:11Z",
            "recurrenceOverrides": {"2026-11-09T09:00:00": {
                "start": "2026-11-09T09:00:00",
                "progress": "completed", "progressUpdated": "2026-11-08T09:10:12Z"}},
            "organizerCalendarAddress": "mailto:organizer@example.com",
            "participants": {
                "o": {"@type": "Participant", "calendarAddress": "mailto:organizer@example.com",
                      "roles": {"owner": true}},
                "b": {"@type": "Participant", "calendarAddress": "mailto:assignee@example.com",
                      "roles": {"attendee": true}, "participationStatus": "accepted",
                      "progress": "in-process", "progressUpdated": "2026-11-08T09:10:13Z"}}
        }}}),
    )
    .await;
    let task = created(&set, "t");
    let href = event_href(&store, &alice, &task).await;
    let text = unfolded(&dav_text(&other, &href).await.unwrap());
    assert!(text.contains("ESTIMATED-DURATION:PT1H30M"), "{text}");
    for (pointer, value) in [
        ("progressUpdated", "\"2026-11-08T09:10:11Z\""),
        ("progressUpdated", "\"2026-11-08T09:10:12Z\""),
        ("participants/b/progressUpdated", "\"2026-11-08T09:10:13Z\""),
    ] {
        assert!(
            has_jsprop(&text, pointer, value),
            "{pointer} {value} missing:\n{text}"
        );
    }
    let properties = json!([
        "estimatedDuration",
        "progress",
        "progressUpdated",
        "recurrenceOverrides",
        "participants"
    ]);
    let got = one(
        &backend,
        &alice,
        "CalendarEvent/get",
        json!({"ids": [task.clone()], "properties": properties}),
    )
    .await;
    let back = &got["list"][0];
    assert_eq!(back["estimatedDuration"], "PT1H30M", "{got}");
    assert_eq!(back["progressUpdated"], "2026-11-08T09:10:11Z", "{got}");
    assert_eq!(
        back["recurrenceOverrides"]["2026-11-09T09:00:00"]["progressUpdated"],
        "2026-11-08T09:10:12Z",
        "{got}"
    );
    assert_eq!(
        back["participants"]["b"]["progressUpdated"], "2026-11-08T09:10:13Z",
        "{got}"
    );

    // What the web patches: completing the task, the occurrence reopened
    // (by path into its override), the assignee's progress, a new estimate.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {task.clone(): {
            "progress": "completed", "progressUpdated": "2026-11-08T10:00:00Z",
            "recurrenceOverrides/2026-11-09T09:00:00/progress": "needs-action",
            "recurrenceOverrides/2026-11-09T09:00:00/progressUpdated": "2026-11-08T10:00:01Z",
            "participants/b/progress": "completed",
            "participants/b/progressUpdated": "2026-11-08T10:00:02Z",
            "estimatedDuration": "PT45M"
        }}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let got = one(
        &backend,
        &alice,
        "CalendarEvent/get",
        json!({"ids": [task.clone()], "properties": properties}),
    )
    .await;
    let back = &got["list"][0];
    assert_eq!(back["estimatedDuration"], "PT45M", "{got}");
    assert_eq!(back["progress"], "completed", "{got}");
    assert_eq!(back["progressUpdated"], "2026-11-08T10:00:00Z", "{got}");
    let occurrence = &back["recurrenceOverrides"]["2026-11-09T09:00:00"];
    assert_eq!(
        occurrence["progressUpdated"], "2026-11-08T10:00:01Z",
        "{got}"
    );
    assert_eq!(
        back["participants"]["b"]["progressUpdated"], "2026-11-08T10:00:02Z",
        "{got}"
    );
    let text = unfolded(&dav_text(&other, &href).await.unwrap());
    assert!(text.contains("ESTIMATED-DURATION:PT45M"), "{text}");
    assert!(!text.contains("PT1H30M"), "{text}");

    // Clearing the estimate (a null patch) removes the property.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {task.clone(): {"estimatedDuration": null}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &href).await.unwrap());
    assert!(!text.contains("ESTIMATED-DURATION"), "{text}");
    assert!(
        has_jsprop(&text, "progressUpdated", "\"2026-11-08T10:00:00Z\""),
        "{text}"
    );

    // A wrong value is refused, not stored (§1.4.4, §1.4.6).
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {task.clone(): {"progressUpdated": "2026-11-08T10:00:00"}}}),
    )
    .await;
    assert_eq!(
        set["notUpdated"][&task]["type"], "invalidProperties",
        "{set}"
    );
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {task.clone(): {"estimatedDuration": "1h"}}}),
    )
    .await;
    assert_eq!(
        set["notUpdated"][&task]["type"], "invalidProperties",
        "{set}"
    );
    // Also inside an override and a participant (RFC 8620 §5.3), by path
    // or as a whole override.
    for patch in [
        json!({"recurrenceOverrides/2026-11-09T09:00:00/progressUpdated": "2026-11-08T10:00:00"}),
        json!({"recurrenceOverrides/2026-11-09T09:00:00/estimatedDuration": "1h"}),
        json!({"participants/b/progressUpdated": "x"}),
        json!({"recurrenceOverrides/2026-11-16T09:00:00": {"estimatedDuration": "1h"}}),
    ] {
        let set = one(
            &backend,
            &alice,
            "CalendarEvent/set",
            json!({"update": {task.clone(): patch}}),
        )
        .await;
        assert_eq!(
            set["notUpdated"][&task]["type"], "invalidProperties",
            "{patch}: {set}"
        );
    }

    // RFC 8984 §1.4.6 allows `P1W2D`; RFC 5545 §3.3.6 does not (a week
    // goes alone): it is written as the same nine nominal days.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"update": {task.clone(): {"estimatedDuration": "P1W2D"}}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &href).await.unwrap());
    assert!(text.contains("ESTIMATED-DURATION:P9D"), "{text}");
    assert!(!text.contains("P1W2D"), "{text}");

    let gone = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"destroy": [list.clone()], "onDestroyRemoveEvents": true}),
    )
    .await;
    assert_eq!(gone["destroyed"], json!([list]), "{gone}");
}
