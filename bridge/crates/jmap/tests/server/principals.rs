// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Principals and free-busy (libre/ocupado) through the bridge against a
//! real CalDAV server: Stalwart's DAV (with Stalwart's IMAP), Radicale or
//! Nextcloud (with Dovecot's), see `nebula-tests`. Skipped unless
//! `NEBULA_TEST_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=stalwart cargo test -p nebula-jmap --test server principals
//! ```
//!
//! The requests are the web's (`getAvailability` in
//! apps/web/src/calendar/api.ts, `listPeople` in mail/sharing.ts).
//! Stalwart and Nextcloud announce `principals:availability` (their
//! outboxes answer free-busy; Nextcloud lists nobody, the bridge finds
//! its users by searching); Radicale only `principals` (no scheduling).
//! Alice's own busy time comes from the bridge's copy of all her calendars
//!one event put by another client in her default
//! calendar (seen after a sync) and one made through the bridge in a new
//! calendar, which Stalwart's outbox would not count, and a private one
//! after it (busy, but no details with `showDetails`). All without
//! attendees (nothing is sent), deleted before the checks. Her
//! `timeZone` must be what the server says of her calendars.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_dav::proto::ResourceName;
use nebula_dav::{PutCondition, ResourceKind};
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{DavKind, DavTestServer, User};
use serde_json::{Value, json};

/// One request with the `using` of jmap-client's `Principal/*` calls.
async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:principals",
            "urn:ietf:params:jmap:principals:availability",
            "urn:ietf:params:jmap:calendars"
        ],
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 1_000_000,
            max_calls_in_request: 32,
        },
        &[
            Capability::Core,
            Capability::Principals,
            Capability::PrincipalsAvailability,
            Capability::Calendars,
        ],
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
    serde_json::to_value(response).unwrap()["methodResponses"]
        .as_array()
        .unwrap()
        .clone()
}

/// The arguments of the only response, checking its name.
async fn one(backend: &JmapBackend, account: &AccountContext, method: &str, args: Value) -> Value {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], method, "{responses:?}");
    responses[0][1].clone()
}

/// The error type of the only response.
async fn error(
    backend: &JmapBackend,
    account: &AccountContext,
    method: &str,
    args: Value,
) -> String {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], "error", "{responses:?}");
    responses[0][1]["type"].as_str().unwrap().to_owned()
}

/// `YYYY-MM-DDTHH:MM:SSZ` and `YYYYMMDDTHHMMSSZ` of a Unix time.
fn times(seconds: i64) -> (String, String) {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (h, m, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
    (
        format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z"),
        format!("{year:04}{month:02}{day:02}T{h:02}{m:02}{s:02}Z"),
    )
}

/// After a calendar in Tokyo: if none of hers had a zone before, hers is
/// Tokyo's; if hers was unknown (zones that disagree), it stays so.
fn tokyo_after(before: &(Option<String>, usize), after: &(Option<String>, usize)) {
    match before {
        (None, 0) => assert_eq!(after.0.as_deref(), Some("Asia/Tokyo"), "{after:?}"),
        (None, _) => assert_eq!(after.0, None, "{after:?}"),
        (Some(_), _) => {}
    }
}

/// Checks the user's `timeZone` against what `Calendar/get` says (the
/// default calendar's zone, else the one zone they agree on), and the
/// `timeZone` filter of `Principal/query`. Returns it, and how many
/// different zones her calendars have.
async fn check_time_zone(
    backend: &JmapBackend,
    alice: &AccountContext,
    me: &str,
) -> (Option<String>, usize) {
    let a = alice.jmap_id.as_str();
    let calendars = one(
        backend,
        alice,
        "Calendar/get",
        json!({"accountId": a, "properties": ["timeZone", "isDefault"]}),
    )
    .await;
    let calendars = calendars["list"].as_array().unwrap().clone();
    let default_zone = calendars
        .iter()
        .find(|c| c["isDefault"] == true)
        .and_then(|c| c["timeZone"].as_str());
    let mut zones: Vec<&str> = calendars
        .iter()
        .filter_map(|c| c["timeZone"].as_str())
        .collect();
    zones.sort_unstable();
    zones.dedup();
    let expected = default_zone.or_else(|| (zones.len() == 1).then(|| zones[0]));
    let responses = call(
        backend,
        alice,
        json!([
            ["Principal/get", {"accountId": a, "ids": [me], "properties": ["timeZone"]}, "0"],
            ["Principal/query", {"accountId": a, "filter": {"timeZone": expected.unwrap_or("Pacific/Nauru")}}, "1"]
        ]),
    )
    .await;
    assert_eq!(
        responses[0][1]["list"][0]["timeZone"],
        json!(expected),
        "{calendars:?} {responses:?}"
    );
    let matched = responses[1][1]["ids"].as_array().unwrap();
    assert_eq!(
        matched.iter().any(|id| id == me),
        expected.is_some(),
        "{responses:?}"
    );

    (expected.map(str::to_owned), zones.len())
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one session of the web, step by step"
)]
async fn principals_and_availability_like_the_web() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    // Stalwart and Nextcloud schedule (and show bob); Radicale does not.
    let scheduling = matches!(dav.kind, DavKind::Stalwart | DavKind::Nextcloud);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    // A short poll: the event another client puts must reach the copy.
    let backend =
        super::contacts::backend_with(&store, &dav.base_url(), dav.mail(), Duration::from_secs(2));
    let alice = super::start(&backend, &store, dav.mail(), User::Alice).await;
    let a = alice.jmap_id.as_str();

    // The capabilities come with DAV discovery; availability only with a
    // scheduling server.
    let mut capabilities = None;
    for _ in 0..200 {
        let found = backend.account_capabilities(&alice).await.unwrap();
        if found.principals.is_some() && found.calendars.is_some() {
            capabilities = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let capabilities = capabilities.expect("principals never announced");
    assert_eq!(capabilities.principals_availability.is_some(), scheduling);
    let me = serde_json::to_value(&capabilities).unwrap()["urn:ietf:params:jmap:principals"]
        ["currentUserPrincipalId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(me.starts_with('r'), "{me}");
    let supported = capabilities.supported();
    assert!(supported.contains(&Capability::Principals));
    assert_eq!(
        supported.contains(&Capability::PrincipalsAvailability),
        scheduling
    );

    // getAvailability (calendar/api.ts): one query per guest, limit 1.
    let alice_address = User::Alice.address();
    let bob_address = User::Bob.address();
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Principal/query", {"accountId": a, "filter": {"email": alice_address.to_uppercase()}, "limit": 1}, "0"],
            ["Principal/query", {"accountId": a, "filter": {"email": bob_address}, "limit": 1}, "1"],
            ["Principal/query", {"accountId": a, "filter": {"email": "nobody@example.com"}, "limit": 1}, "2"],
            ["Principal/query", {"accountId": a, "filter": {"email": "not an address"}, "limit": 1}, "3"]
        ]),
    )
    .await;
    assert_eq!(responses[0][1]["ids"], json!([me]), "{responses:?}");
    let bob = responses[1][1]["ids"].as_array().unwrap().clone();
    if scheduling {
        assert_eq!(bob.len(), 1, "{responses:?}");
    } else {
        // Radicale shows nobody else.
        assert!(bob.is_empty(), "{responses:?}");
    }
    assert_eq!(responses[2][1]["ids"], json!([]));
    assert_eq!(responses[3][1]["ids"], json!([]));

    // listPeople (mail/sharing.ts): everyone, then their names.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Principal/query", {"accountId": a, "limit": 500}, "0"],
            ["Principal/get", {"accountId": a,
                "#ids": {"resultOf": "0", "name": "Principal/query", "path": "/ids"},
                "properties": ["name", "email", "type"]}, "1"]
        ]),
    )
    .await;
    let people = responses[1][1]["list"].as_array().unwrap().clone();
    let emails: Vec<&str> = people.iter().filter_map(|p| p["email"].as_str()).collect();
    assert!(emails.contains(&alice_address.as_str()), "{people:?}");
    assert_eq!(
        emails.contains(&bob_address.as_str()),
        scheduling,
        "{people:?}"
    );
    // People, and on Stalwart its groups too (docker/setup.ts makes
    // ventas@, listed with the others), as groups.
    assert!(
        people.iter().all(|p| p["type"] == "individual"
            || (dav.kind == DavKind::Stalwart
                && p["type"] == "group"
                && p["email"]
                    .as_str()
                    .is_some_and(|e| e.starts_with("ventas@")))),
        "{people:?}"
    );
    let own = one(
        &backend,
        &alice,
        "Principal/get",
        json!({"accountId": a, "ids": [me, "r1", "x"]}),
    )
    .await;
    assert_eq!(own["notFound"], json!(["r1", "x"]));
    // Principal/changes: no change log; the same state has no changes,
    // another one cannot be calculated.
    let state = own["state"].as_str().unwrap().to_owned();
    let same = one(
        &backend,
        &alice,
        "Principal/changes",
        json!({"accountId": a, "sinceState": state}),
    )
    .await;
    assert_eq!(same["newState"], state.as_str());
    assert_eq!(same["created"], json!([]));
    assert_eq!(same["hasMoreChanges"], false);
    let kind = error(
        &backend,
        &alice,
        "Principal/changes",
        json!({"accountId": a, "sinceState": "r0"}),
    )
    .await;
    assert_eq!(kind, "cannotCalculateChanges");
    let own = &own["list"][0];
    assert_eq!(own["email"], alice_address.as_str());
    assert_eq!(
        own["capabilities"]["urn:ietf:params:jmap:calendars"]["mayGetAvailability"],
        scheduling
    );
    assert_eq!(
        own["accounts"][a]["urn:ietf:params:jmap:principals:owner"]["principalId"],
        me.as_str()
    );

    // timeZone: as her calendars say (also
    // after a new one in Tokyo, below). The capabilities come with
    // discovery, the calendars with the first sync: until then she has
    // none, and "no zone before" would be wrong (seen with Radicale just
    // started).
    let mut synced = false;
    for _ in 0..200 {
        let got = one(
            &backend,
            &alice,
            "Calendar/get",
            json!({"accountId": a, "properties": ["name"]}),
        )
        .await;
        if got["list"].as_array().is_some_and(|list| !list.is_empty()) {
            synced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(synced, "her calendars never arrived");
    let before_zone = check_time_zone(&backend, &alice, &me).await;

    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    // 03:00 UTC, day after tomorrow: never on an event of the seed data.
    let start = (now / 86_400 + 2) * 86_400 + 3 * 3600;
    let (start_text, start_ical) = times(start);
    let (end_text, end_ical) = times(start + 3600);
    let (before, _) = times(start - 3600);
    let (after, _) = times(start + 2 * 3600);

    if !scheduling {
        // No outbox: the method is not for this account.
        let kind = error(
            &backend,
            &alice,
            "Principal/getAvailability",
            json!({"accountId": a, "id": me, "utcStart": before, "utcEnd": after}),
        )
        .await;
        assert_eq!(kind, "accountNotSupportedByMethod");
        let tokyo = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "create": {"t": {"name": "nebula-test-tokyo", "timeZone": "Asia/Tokyo"}}}),
        )
        .await;
        let tokyo = tokyo["created"]["t"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{tokyo}"))
            .to_owned();
        let with_tokyo = check_time_zone(&backend, &alice, &me).await;
        one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "destroy": [tokyo], "onDestroyRemoveEvents": true}),
        )
        .await;
        tokyo_after(&before_zone, &with_tokyo);
        return;
    }

    // Alice busy tomorrow for an hour: put by another client (no
    // attendees: nothing is sent) in her default calendar.
    let other = super::contacts::other_client(dav);
    let home = other.discover().await.unwrap().calendar_homes[0].clone();
    let calendars = other.list_collections(&home).await.unwrap();
    let calendar = calendars
        .iter()
        // Stalwart's "default", Nextcloud's "personal" (not its read-only
        // birthday calendar).
        .find(|c| matches!(c.href.last_segment(), Some("default" | "personal")))
        .or_else(|| calendars.first())
        .expect("a calendar")
        .href
        .clone();
    let uid = format!(
        "nebula-test-freebusy-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let event = calendar
        .member(
            &ResourceName::try_from(format!("{uid}.ics").as_str()).unwrap(),
            false,
        )
        .unwrap();
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//bridge tests//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261027T120000Z\r\n\
         DTSTART:{start_ical}\r\nDTEND:{end_ical}\r\nSUMMARY:Busy\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    other
        .put(&event, ResourceKind::Calendar, &text, &PutCondition::Create)
        .await
        .unwrap();

    // And an hour later in a new calendar, through the bridge: Stalwart
    // 0.16.27's outbox only counts the default calendar; the bridge counts
    // every one of hers.
    let (second_start, _) = times(start + 3600);
    let (second_end, _) = times(start + 2 * 3600);
    let made = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"c": {"name": format!("{uid}-cal"), "isSubscribed": true, "isVisible": true,
            "timeZone": "Asia/Tokyo"}}}),
    )
    .await;
    let second = made["created"]["c"]["id"]
        .as_str()
        .expect("calendar")
        .to_owned();
    let made = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {"e": {
            "@type": "Event", "calendarIds": {second.clone(): true}, "title": "Busy too",
            "start": second_start.trim_end_matches('Z'), "timeZone": "Etc/UTC", "duration": "PT1H"
        }}}),
    )
    .await;
    assert!(made["created"]["e"]["id"].is_string(), "{made}");
    let with_tokyo = check_time_zone(&backend, &alice, &me).await;
    tokyo_after(&before_zone, &with_tokyo);
    // A private one right after: busy, but no details (showDetails).
    let (third_start, _) = times(start + 2 * 3600);
    let (third_end, _) = times(start + 3 * 3600);
    let made = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {"p": {
            "@type": "Event", "calendarIds": {second.clone(): true}, "title": "Doctor",
            "start": third_start.trim_end_matches('Z'), "timeZone": "Etc/UTC", "duration": "PT1H",
            "privacy": "private"
        }}}),
    )
    .await;
    assert!(made["created"]["p"]["id"].is_string(), "{made}");
    // Wait for the sync to bring the other client's event.
    let mut seen = false;
    for _ in 0..150 {
        let found = one(
            &backend,
            &alice,
            "CalendarEvent/query",
            json!({"accountId": a, "filter": {"uid": uid.clone()}}),
        )
        .await;
        if found["ids"].as_array().is_some_and(|ids| !ids.is_empty()) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let responses = call(
        &backend,
        &alice,
        json!([
            ["Principal/getAvailability", {"accountId": a, "id": me,
                "utcStart": before, "utcEnd": after}, "0"],
            ["Principal/getAvailability", {"accountId": a, "id": bob[0],
                "utcStart": before, "utcEnd": after}, "1"],
            ["Principal/getAvailability", {"accountId": a, "id": "r1",
                "utcStart": before, "utcEnd": after}, "2"],
            ["Principal/getAvailability", {"accountId": a, "id": me,
                "utcStart": "2026-01-01T00:00:00Z", "utcEnd": "2028-01-01T00:00:00Z"}, "3"],
            ["Principal/getAvailability", {"accountId": a, "id": me,
                "utcStart": after, "utcEnd": before}, "4"],
            ["Principal/getAvailability", {"accountId": a, "id": me,
                "utcStart": before, "utcEnd": third_end, "showDetails": true}, "5"],
            ["Principal/getAvailability", {"accountId": a, "id": me,
                "utcStart": before, "utcEnd": third_end, "showDetails": true,
                "eventProperties": ["title", "calendarIds"]}, "6"],
            ["Principal/getAvailability", {"accountId": a, "id": bob[0],
                "utcStart": before, "utcEnd": third_end, "showDetails": true}, "7"]
        ]),
    )
    .await;
    // Cleaned up before checking, so a failure leaves nothing behind.
    other.delete(&event, None).await.unwrap();
    let gone = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [second], "onDestroyRemoveEvents": true}),
    )
    .await;
    assert_eq!(
        gone["destroyed"].as_array().map(Vec::len),
        Some(1),
        "{gone}"
    );
    assert!(seen, "the other client's event never reached the bridge");
    let periods = responses[0][1]["list"].as_array().unwrap();
    // Both hours, one period per event, from the bridge's copy.
    for (from, to) in [(&start_text, &end_text), (&second_start, &second_end)] {
        assert!(
            periods.iter().any(|period| {
                period["busyStatus"] == "confirmed"
                    && period["event"].is_null()
                    && period["utcStart"] == from.as_str()
                    && period["utcEnd"] == to.as_str()
            }),
            "{from}: {responses:?}"
        );
    }
    // Bob shares nothing with alice: an empty list, as Stalwart's JMAP.
    // Nextcloud shows every user's busy times to the others (bob's
    // calendar is empty here unless another test left something).
    if dav.kind == DavKind::Nextcloud {
        assert!(responses[1][1]["list"].is_array(), "{responses:?}");
    } else {
        assert_eq!(responses[1][1], json!({"list": []}), "{responses:?}");
    }
    assert_eq!(responses[2][1]["type"], "notFound", "{responses:?}");
    assert_eq!(responses[3][1]["type"], "tooLarge", "{responses:?}");
    assert_eq!(responses[4][1], json!({"list": []}), "{responses:?}");

    // showDetails: her own events as CalendarEvent/get gives them (these
    // don't repeat, so each is itself); the private one busy without
    // details.
    let detailed = responses[5][1]["list"].as_array().unwrap();
    let at = |list: &[Value], from: &str| {
        list.iter()
            .find(|p| p["utcStart"] == from)
            .cloned()
            .unwrap_or_else(|| panic!("{from}: {list:?}"))
    };
    let first = at(detailed, &start_text);
    assert_eq!(first["event"]["title"], "Busy", "{first}");
    assert!(first["event"]["id"].is_string(), "{first}");
    assert!(first["event"]["baseEventId"].is_null(), "{first}");
    let made_here = at(detailed, &second_start);
    assert_eq!(made_here["event"]["title"], "Busy too", "{made_here}");
    assert_eq!(
        made_here["event"]["calendarIds"],
        json!({second.as_str(): true}),
        "{made_here}"
    );
    let private = at(detailed, &third_start);
    assert_eq!(private["busyStatus"], "confirmed", "{private}");
    assert!(private["event"].is_null(), "{private}");
    // eventProperties: only those.
    let selected = at(responses[6][1]["list"].as_array().unwrap(), &second_start);
    assert_eq!(
        selected["event"],
        json!({"title": "Busy too", "calendarIds": {second.as_str(): true}}),
        "{selected}"
    );
    // Someone else: never details (a DAV free-busy reply has only times).
    let others = responses[7][1]["list"].as_array().unwrap();
    assert!(others.iter().all(|p| p["event"].is_null()), "{others:?}");
}
