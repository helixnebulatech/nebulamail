// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Nextcloud's calendar extras through the bridge
//! publishing, webcal subscriptions and the trash bin, with
//! `NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=nextcloud`. With another DAV
//! server (Stalwart, Radicale) it checks that none of it is offered.
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=nextcloud cargo test -p nebula-jmap --test server nextcloud_extras
//! ```
//!
//! The subscription's URL is under `.invalid` (RFC 6761): nobody can
//! fetch it, and the bridge never tries.

use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{DavKind, DavTestServer, User};
use serde_json::{Value, json};

use super::calendar_set::{
    backend, calendar_href, created, mail_server, other_client, unique, wait_for_calendars,
};

const BRIDGE: &str = "https://helixnebula.tech/jmap/bridge";
const IS_PUBLISHED: &str = "https://helixnebula.tech/jmap/bridge/isPublished";
const PUBLIC_URL: &str = "https://helixnebula.tech/jmap/bridge/publicUrl";
const SUBSCRIPTION_URL: &str = "https://helixnebula.tech/jmap/bridge/subscriptionUrl";

/// The key of a patch of a vendor property (a JSON pointer: `/` is `~1`).
fn pointer(name: &str) -> String {
    name.replace('/', "~1")
}

async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
            BRIDGE
        ],
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 1_000_000,
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
    serde_json::to_value(response).unwrap()["methodResponses"]
        .as_array()
        .unwrap()
        .clone()
}

/// One call; its response (or its error, as `["error", …]`).
async fn one(backend: &JmapBackend, account: &AccountContext, method: &str, args: Value) -> Value {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    if responses[0][0] == "error" {
        return json!({"error": responses[0][1].clone()});
    }
    assert_eq!(responses[0][0], method, "{responses:?}");
    responses[0][1].clone()
}

async fn calendar(backend: &JmapBackend, account: &AccountContext, id: &str) -> Option<Value> {
    let got = one(
        backend,
        account,
        "Calendar/get",
        json!({"accountId": account.jmap_id.as_str(), "ids": [id]}),
    )
    .await;
    got["list"].as_array().and_then(|l| l.first()).cloned()
}

async fn trash(backend: &JmapBackend, account: &AccountContext) -> Value {
    one(
        backend,
        account,
        "CalendarTrash/get",
        json!({"accountId": account.jmap_id.as_str()}),
    )
    .await
}

fn trash_id(list: &Value, field: &str, value: &str) -> Option<String> {
    list.as_array()?
        .iter()
        .find(|item| item[field] == value)
        .and_then(|item| item["id"].as_str())
        .map(str::to_owned)
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one session, step by step")]
async fn nextcloud_extras() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let alice = super::start(&backend, &store, mail_server(dav), User::Alice).await;
    let a = alice.jmap_id.as_str();
    wait_for_calendars(&backend, &alice).await;
    let capabilities = backend.account_capabilities(&alice).await.unwrap();
    let extras = capabilities.bridge.unwrap();
    let nextcloud = dav.kind == DavKind::Nextcloud;
    assert_eq!(extras.calendar_publishing, nextcloud);
    assert_eq!(extras.calendar_subscriptions, nextcloud);
    assert_eq!(extras.calendar_trash, nextcloud);

    // A calendar of the user's own.
    let name = unique("pub");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"c": {"name": name, "color": "#3366cc"}}}),
    )
    .await;
    let id = created(&set, "c");

    if !nextcloud {
        let got = calendar(&backend, &alice, &id).await.unwrap();
        assert!(got.get(IS_PUBLISHED).is_none() && got.get(PUBLIC_URL).is_none());
        let refused = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "update": {&id: {pointer(IS_PUBLISHED): true}}}),
        )
        .await;
        assert_eq!(refused["notUpdated"][&id]["type"], "forbidden", "{refused}");
        let sub = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "create": {"s": {"name": "x",
                SUBSCRIPTION_URL: "webcal://nebula-test.invalid/x.ics"}}}),
        )
        .await;
        assert_eq!(sub["notCreated"]["s"]["type"], "forbidden", "{sub}");
        let bin = trash(&backend, &alice).await;
        assert_eq!(bin["error"]["type"], "accountNotSupportedByMethod", "{bin}");
        one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "destroy": [id], "onDestroyRemoveEvents": true}),
        )
        .await;
        return;
    }

    // Publishing.
    let got = calendar(&backend, &alice, &id).await.unwrap();
    assert_eq!(got[IS_PUBLISHED], false, "{got}");
    assert_eq!(got[PUBLIC_URL], Value::Null);
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {&id: {pointer(IS_PUBLISHED): true}}}),
    )
    .await;
    assert!(set["updated"].get(&id).is_some(), "{set}");
    let got = calendar(&backend, &alice, &id).await.unwrap();
    assert_eq!(got[IS_PUBLISHED], true, "{got}");
    let link = got[PUBLIC_URL].as_str().unwrap().to_owned();
    assert!(
        link.starts_with("http") && link.contains("/public-calendars/"),
        "{link}"
    );
    // The other client sees the same link.
    let other = other_client(dav, User::Alice);
    let href = calendar_href(&store, &alice, &id).await;
    let home = other.discover().await.unwrap().calendar_homes[0].clone();
    let listed = other.list_collections(&home).await.unwrap();
    let remote = listed.iter().find(|c| c.href.same_location(&href)).unwrap();
    assert_eq!(
        remote
            .publish_url
            .as_ref()
            .map(nebula_dav::proto::ExternalUrl::as_str),
        Some(link.as_str())
    );
    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {&id: {pointer(IS_PUBLISHED): false}}}),
    )
    .await;
    let got = calendar(&backend, &alice, &id).await.unwrap();
    assert_eq!(got[IS_PUBLISHED], false, "{got}");
    assert_eq!(got[PUBLIC_URL], Value::Null);
    // `publicUrl` is the server's.
    let refused = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {&id: {pointer(PUBLIC_URL): "https://x/"}}}),
    )
    .await;
    assert_eq!(refused["notUpdated"][&id]["type"], "invalidProperties");

    // Subscriptions: bad URLs never reach the server.
    for bad in [
        "http://nebula-test.invalid/x.ics",
        "https://user:pw@nebula-test.invalid/x.ics",
        "file:///etc/passwd",
    ] {
        let refused = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "create": {"s": {"name": "x", SUBSCRIPTION_URL: bad}}}),
        )
        .await;
        assert_eq!(
            refused["notCreated"]["s"]["type"], "invalidProperties",
            "{bad}"
        );
    }
    let sub_name = unique("sub");
    let source = "webcal://nebula-test.invalid/holidays.ics";
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"s": {"name": sub_name, "color": "#aa0000",
            SUBSCRIPTION_URL: source}}}),
    )
    .await;
    let sub = created(&set, "s");
    assert_eq!(set["created"]["s"][SUBSCRIPTION_URL], source, "{set}");
    let got = calendar(&backend, &alice, &sub).await.unwrap();
    assert_eq!(got["name"], sub_name.as_str());
    assert_eq!(got[SUBSCRIPTION_URL], source);
    assert!(got.get(IS_PUBLISHED).is_none());
    assert_eq!(got["myRights"]["mayReadItems"], true);
    assert_eq!(got["myRights"]["mayWriteAll"], false);
    assert_eq!(got["myRights"]["mayDelete"], true);
    // Read-only: no events in it.
    let refused = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {"e": {"@type": "Event", "calendarIds": {&sub: true},
            "title": "x", "start": "2026-11-20T10:00:00", "duration": "PT1H"}}}),
    )
    .await;
    assert!(refused["notCreated"].get("e").is_some(), "{refused}");
    // Renamed (Nextcloud keeps it with the subscription).
    let renamed = format!("{sub_name}-renamed");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {&sub: {"name": renamed}}}),
    )
    .await;
    assert!(set["updated"].get(&sub).is_some(), "{set}");
    assert_eq!(
        calendar(&backend, &alice, &sub).await.unwrap()["name"],
        renamed.as_str()
    );
    // Unsubscribed.
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [sub]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([sub]), "{set}");
    let listed = other.list_collections(&home).await.unwrap();
    assert!(
        listed.iter().all(
            |c| c.subscription.is_none() || c.display_name.as_deref() != Some(renamed.as_str())
        )
    );

    // The trash bin: an event.
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {"e": {"@type": "Event", "calendarIds": {&id: true},
            "title": "Nebula trash test", "start": "2026-11-20T10:00:00",
            "timeZone": "Europe/Madrid", "duration": "PT1H"}}}),
    )
    .await;
    let event = created(&set, "e");
    one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "destroy": [event]}),
    )
    .await;
    let bin = trash(&backend, &alice).await;
    assert!(bin["retentionSeconds"].as_u64().is_some(), "{bin}");
    let trashed = bin["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == "Nebula trash test")
        .cloned()
        .unwrap_or_else(|| panic!("the event in the bin: {bin}"));
    assert_eq!(trashed["calendarId"], id.as_str());
    assert_eq!(trashed["utcStart"], "2026-11-20T09:00:00Z");
    assert!(trashed["deletedAt"].as_str().is_some());
    let trashed_id = trashed["id"].as_str().unwrap().to_owned();
    let set = one(
        &backend,
        &alice,
        "CalendarTrash/set",
        json!({"accountId": a, "restore": [trashed_id, "e0000000000000001"]}),
    )
    .await;
    assert_eq!(set["restored"], json!([trashed_id]), "{set}");
    assert_eq!(set["notRestored"]["e0000000000000001"]["type"], "notFound");
    let found = one(
        &backend,
        &alice,
        "CalendarEvent/query",
        json!({"accountId": a, "filter": {"inCalendar": id}}),
    )
    .await;
    let back = found["ids"].as_array().unwrap();
    assert_eq!(back.len(), 1, "the event is back: {found}");
    // Deleted again, then for good.
    one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "destroy": [back[0]]}),
    )
    .await;
    let bin = trash(&backend, &alice).await;
    let again = trash_id(&bin["events"], "title", "Nebula trash test").unwrap();
    let set = one(
        &backend,
        &alice,
        "CalendarTrash/set",
        json!({"accountId": a, "destroy": [again]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([again]), "{set}");
    let bin = trash(&backend, &alice).await;
    assert!(trash_id(&bin["events"], "title", "Nebula trash test").is_none());

    // The trash bin: the calendar.
    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [id], "onDestroyRemoveEvents": true}),
    )
    .await;
    assert!(calendar(&backend, &alice, &id).await.is_none());
    let bin = trash(&backend, &alice).await;
    let gone = trash_id(&bin["calendars"], "name", &name)
        .unwrap_or_else(|| panic!("the calendar in the bin: {bin}"));
    let set = one(
        &backend,
        &alice,
        "CalendarTrash/set",
        json!({"accountId": a, "restore": [gone]}),
    )
    .await;
    assert_eq!(set["restored"], json!([gone]), "{set}");
    let back = calendar(&backend, &alice, &id)
        .await
        .expect("the same calendar is back, with its id");
    assert_eq!(back["name"], name.as_str());
    // Deleted again, then for good.
    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [id], "onDestroyRemoveEvents": true}),
    )
    .await;
    let bin = trash(&backend, &alice).await;
    let gone = trash_id(&bin["calendars"], "name", &name).unwrap();
    let set = one(
        &backend,
        &alice,
        "CalendarTrash/set",
        json!({"accountId": a, "destroy": [gone]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([gone]), "{set}");
    let bin = trash(&backend, &alice).await;
    assert!(trash_id(&bin["calendars"], "name", &name).is_none());
    let listed = other.list_collections(&home).await.unwrap();
    assert!(listed.iter().all(|c| !c.href.same_location(&href)));
}
