// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! RFC 8620 §5 and RFC 9425 rules every method must keep, against the real
//! servers (Stalwart or Dovecot; the quota parts need a server with
//! `QUOTA`, Stalwart):
//!
//! ```sh
//! NEBULA_TEST_IMAP=1 cargo test -p nebula-jmap --test server rfc_limits
//! NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=dovecot cargo test -p nebula-jmap --test server rfc_limits
//! ```
//!
//! - `/get`: a repeated id is answered once (§5.1).
//! - `/changes`: never more ids than `maxChanges` (§5.2).
//! - `/query`: a collation is honoured or `unsupportedSort` (§5.5).
//! - Quota: `updatedProperties`, `Quota/queryChanges`, types filtered by
//!   `using` and push (RFC 9425 §4.1, §4.3, §4.5, §6).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use nebula_http::{AccountContext, ApiCall, Backend, PushEvent};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::types::DataType;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{TestServer, User};
use serde_json::{Value, json};

use super::{call, mailbox, new_backend, start};

/// Runs one request with this `using`.
async fn call_using(
    backend: &JmapBackend,
    account: &AccountContext,
    using: &[Capability],
    calls: Value,
) -> Vec<Value> {
    let body = json!({
        "using": using.iter().map(Capability::as_str).collect::<Vec<_>>(),
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        using,
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
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

fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap_or_else(|| o.as_str().unwrap()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario against a real server")]
async fn rfc8620_rules() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let inbox = mailbox(&backend, &alice, "inbox").await;
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Two drafts, so there are emails and threads to ask for.
    let created = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "create": {
            "d1": {"mailboxIds": {drafts.clone(): true}, "keywords": {"$draft": true},
                   "subject": format!("rfc-limits {stamp} a"),
                   "textBody": [{"partId": "t", "type": "text/plain"}],
                   "bodyValues": {"t": {"value": "uno"}}},
            "d2": {"mailboxIds": {drafts.clone(): true}, "keywords": {"$draft": true},
                   "subject": format!("rfc-limits {stamp} b"),
                   "textBody": [{"partId": "t", "type": "text/plain"}],
                   "bodyValues": {"t": {"value": "dos"}}}
        }}, "0"]]),
    )
    .await;
    let e1 = created[0][1]["created"]["d1"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created:?}"))
        .to_owned();
    let e2 = created[0][1]["created"]["d2"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let t1 = created[0][1]["created"]["d1"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();

    // §5.1: each repeated id once, in the list or in notFound; a
    // `#creationId` is the same id as the one it stands for.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Mailbox/get", {"accountId": a, "ids": [inbox, drafts, inbox, "zz-none", "zz-none"], "properties": ["name"]}, "0"],
            ["Email/get", {"accountId": a, "ids": [e1, e2, e1, "zz-none", "zz-none"], "properties": ["subject"]}, "1"],
            ["Thread/get", {"accountId": a, "ids": [t1, t1, "zz-none", "zz-none"]}, "2"],
            ["Identity/get", {"accountId": a, "ids": ["zz-none", "zz-none"]}, "3"],
            ["EmailSubmission/get", {"accountId": a, "ids": ["zz-none", "zz-none"]}, "4"],
            ["Quota/get", {"accountId": a, "ids": ["zz-none", "zz-none"]}, "5"]
        ]),
    )
    .await;
    assert_eq!(ids(&responses[0][1]["list"]), [inbox.as_str(), &drafts]);
    assert_eq!(ids(&responses[1][1]["list"]), [e1.as_str(), &e2]);
    assert_eq!(ids(&responses[2][1]["list"]), [t1.as_str()]);
    for response in &responses {
        assert_eq!(response[1]["notFound"], json!(["zz-none"]), "{response}");
    }
    let made = call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"d3": {
                "mailboxIds": {drafts.clone(): true}, "keywords": {"$draft": true},
                "subject": format!("rfc-limits {stamp} c")
            }}}, "0"],
            ["Email/get", {"accountId": a, "ids": ["#d3", "#d3"], "properties": ["subject"]}, "1"]
        ]),
    )
    .await;
    let e3 = made[0][1]["created"]["d3"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{made:?}"))
        .to_owned();
    assert_eq!(ids(&made[1][1]["list"]), [e3.as_str()], "{made:?}");

    // §5.2: never more ids than `maxChanges`; a first state bigger than
    // it is `cannotCalculateChanges` (the bridge never splits a state).
    for since in ["0", "1"] {
        for max in [1, 2] {
            let changes = call(
                &backend,
                &alice,
                json!([["Email/changes", {"accountId": a, "sinceState": since, "maxChanges": max}, "0"]]),
            )
            .await;
            let answer = &changes[0][1];
            if changes[0][0] == "error" {
                assert_eq!(answer["type"], "cannotCalculateChanges", "{changes:?}");
                continue;
            }
            let count = ["created", "updated", "destroyed"]
                .iter()
                .map(|list| answer[list].as_array().unwrap().len())
                .sum::<usize>();
            assert!(count <= max, "{changes:?}");
        }
    }

    // §5.5: an offered collation is honoured or `unsupportedSort`; one not
    // offered (`i;octet`) is never accepted and ignored.
    let sort = |property: &str, collation: &str| json!(["Email/query", {"accountId": a, "sort": [{"property": property, "collation": collation}], "limit": 1}, "0"]);
    for (property, collation, ok) in [
        ("subject", "i;unicode-casemap", true),
        ("subject", "i;octet", false),
        ("from", "i;ascii-numeric", false),
        ("receivedAt", "i;octet", false),
        ("receivedAt", "i;ascii-casemap", true),
    ] {
        let answer = call(&backend, &alice, json!([sort(property, collation)])).await;
        if ok {
            assert_eq!(answer[0][0], "Email/query", "{answer:?}");
        } else {
            assert_eq!(answer[0][1]["type"], "unsupportedSort", "{answer:?}");
        }
    }

    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [e1, e2, e3]}, "0"]]),
    )
    .await;
    backend.account_stopped(alice.id).await;
}

/// The next push of a `Quota` state within `wait`.
async fn quota_push(
    events: &mut (impl futures::Stream<Item = PushEvent> + Unpin),
    account: &AccountContext,
    wait: Duration,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let event = tokio::time::timeout_at(deadline, events.next())
            .await
            .ok()??;
        if let PushEvent::State(change) = event
            && let Some(state) = change
                .changed
                .get(&account.jmap_id)
                .and_then(|states| states.get(&DataType::Quota))
        {
            return Some(state.as_str().to_owned());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario against a real server")]
async fn quota_rules() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    if !backend.account_capabilities(&alice).await.unwrap().quota {
        // Dovecot of the tests: no QUOTA, nothing to check.
        backend.account_stopped(alice.id).await;
        return;
    }
    let mut events = backend.events(&alice, false).await.unwrap();
    let base = backend.push_states(&alice).await.unwrap();
    let pushed_base = base.changed[&alice.jmap_id]
        .get(&DataType::Quota)
        .map(|s| s.as_str().to_owned());

    let before = call(
        &backend,
        &alice,
        json!([
            ["Quota/get", {"accountId": a, "ids": null}, "0"],
            ["Quota/query", {"accountId": a, "sort": [{"property": "name"}]}, "1"]
        ]),
    )
    .await;
    let state = before[0][1]["state"].as_str().unwrap().to_owned();
    // RFC 9425 §6: the push base has the Quota state.
    assert_eq!(pushed_base.as_deref(), Some(state.as_str()), "{base:?}");
    assert_eq!(before[1][1]["canCalculateChanges"], true, "{before:?}");
    assert_eq!(before[1][1]["queryState"], state.as_str());
    let quota_ids: Vec<String> = ids(&before[0][1]["list"])
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert!(!quota_ids.is_empty(), "{before:?}");

    // RFC 9425 §4.1: without `urn:ietf:params:jmap:mail` in `using` the
    // client knows none of the types (Email, Mailbox): no quota is shown.
    let blind = call_using(
        &backend,
        &alice,
        &[Capability::Core, Capability::Quota],
        json!([
            ["Quota/get", {"accountId": a, "ids": null}, "0"],
            ["Quota/get", {"accountId": a, "ids": [quota_ids[0]]}, "1"],
            ["Quota/query", {"accountId": a}, "2"],
            ["Quota/changes", {"accountId": a, "sinceState": state}, "3"]
        ]),
    )
    .await;
    assert_eq!(blind[0][1]["list"], json!([]), "{blind:?}");
    assert_eq!(blind[1][1]["notFound"], json!([quota_ids[0]]), "{blind:?}");
    assert_eq!(blind[2][1]["ids"], json!([]), "{blind:?}");
    assert_eq!(blind[3][1]["created"], json!([]), "{blind:?}");

    // A message moves the storage quota: `Quota/changes` says only `used`
    // changed, `Quota/queryChanges` has it, and the push tells.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let created = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "create": {"d": {
            "mailboxIds": {drafts.clone(): true}, "keywords": {"$draft": true},
            "subject": format!("quota-rules {stamp}"),
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "x".repeat(20_000)}}
        }}}, "0"]]),
    )
    .await;
    let email = created[0][1]["created"]["d"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created:?}"))
        .to_owned();
    let pushed = quota_push(&mut events, &alice, Duration::from_secs(30))
        .await
        .expect("a Quota push after a new message");
    assert_ne!(pushed, state);
    let after = call(
        &backend,
        &alice,
        json!([
            ["Quota/changes", {"accountId": a, "sinceState": state}, "0"],
            ["Quota/get", {"accountId": a,
                "#ids": {"resultOf": "0", "name": "Quota/changes", "path": "/updated"},
                "#properties": {"resultOf": "0", "name": "Quota/changes", "path": "/updatedProperties"}}, "1"],
            ["Quota/queryChanges", {"accountId": a, "sort": [{"property": "name"}], "sinceQueryState": state, "calculateTotal": true}, "2"],
            ["Quota/queryChanges", {"accountId": a, "sinceQueryState": "nope"}, "3"],
            ["Quota/query", {"accountId": a, "sort": [{"property": "name", "collation": "i;octet"}]}, "4"]
        ]),
    )
    .await;
    assert_eq!(after[0][0], "Quota/changes", "{after:?}");
    let updated = after[0][1]["updated"].as_array().unwrap();
    assert!(!updated.is_empty(), "{after:?}");
    assert_eq!(
        after[0][1]["updatedProperties"],
        json!(["used"]),
        "{after:?}"
    );
    // The RFC 9425 §5.2 request: only `id` and `used`.
    for quota in after[1][1]["list"].as_array().unwrap() {
        let mut keys: Vec<&String> = quota.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["id", "used"], "{after:?}");
    }
    assert_eq!(after[2][0], "Quota/queryChanges", "{after:?}");
    assert_eq!(after[2][1]["total"], quota_ids.len(), "{after:?}");
    for id in updated {
        assert!(
            after[2][1]["removed"].as_array().unwrap().contains(id),
            "{after:?}"
        );
        assert!(
            after[2][1]["added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == *id),
            "{after:?}"
        );
    }
    assert_eq!(after[3][1]["type"], "cannotCalculateChanges", "{after:?}");
    assert_eq!(after[4][1]["type"], "unsupportedSort", "{after:?}");

    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
    )
    .await;
    backend.account_stopped(alice.id).await;
}
