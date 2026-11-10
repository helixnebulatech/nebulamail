// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Held messages (RFC 4865 FUTURERELEASE) against real
//! servers: undo send (`HOLDFOR`) and scheduled send (`HOLDUNTIL`) with the
//! web app's requests, cancel, the limits, a release after the bridge
//! restarts and one that waits for the account to sign in again, and the
//! vendor property `sendWhenSignedOut` (sent after sign-out). Best with
//! Postfix (nothing goes through Stalwart):
//!
//! ```sh
//! NEBULA_TEST_IMAP=1 NEBULA_TEST_SMTP=1 NEBULA_TEST_SERVER=postfix cargo test -p nebula-jmap --test server held -- --test-threads=1
//! ```
//!
//! Three messages go from alice to bob.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_crypto::AccountKey;
use nebula_http::{AccountContext, Backend, LoginOutcome, StartReason, jmap_account_id};
use nebula_jmap::JmapBackend;
use nebula_store::{NewAccount, Store, UnixTime};
use nebula_tests::{TestServer, User};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;

use super::{call, find, mailbox, new_backend, new_backend_full};

/// The bridge's vendor property (`EmailSubmission`).
const SIGNED_OUT: &str = "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut";

/// Logs a user in as the HTTP layer would, with this data key (the
/// bridge keeps the same one across restarts: it is stored wrapped).
async fn start_with(
    backend: &JmapBackend,
    store: &Store,
    server: TestServer,
    user: User,
    key: AccountKey,
) -> AccountContext {
    let password = || server.password(user);
    let login = user.login();
    let outcome = backend
        .verify_login(&login.parse().unwrap(), &password())
        .await;
    let LoginOutcome::Success { email } = outcome else {
        panic!("login failed: {outcome:?}")
    };
    let account = store
        .get_or_create_account(
            NewAccount {
                login,
                email: &email,
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap();
    let context = AccountContext {
        id: account.id,
        jmap_id: jmap_account_id(account.id).unwrap(),
        login: login.to_owned(),
        email,
    };
    backend
        .account_started(&context, password(), key, StartReason::Login)
        .await
        .unwrap();
    context
}

/// The backend with its background work (the scheduler) running.
fn running(backend: &JmapBackend) -> CancellationToken {
    let shutdown = CancellationToken::new();
    let task = backend.clone();
    let token = shutdown.clone();
    tokio::spawn(async move { task.run(token).await });
    shutdown
}

fn utc(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).unwrap().as_secs();
    OffsetDateTime::from_unix_timestamp(i64::try_from(secs).unwrap())
        .unwrap()
        .format(&Rfc3339)
        .unwrap()
}

struct Mailboxes {
    drafts: String,
    sent: String,
    identity: String,
}

/// The web app's request (compose.ts): a draft and its submission with
/// `parameters` in `mailFrom`, filed in Sent on success.
async fn submit(
    backend: &JmapBackend,
    alice: &AccountContext,
    boxes: &Mailboxes,
    subject: &str,
    parameters: &Value,
) -> Vec<Value> {
    submit_with(backend, alice, boxes, subject, parameters, json!({})).await
}

/// [`submit`] with more properties in the submission's create.
async fn submit_with(
    backend: &JmapBackend,
    alice: &AccountContext,
    boxes: &Mailboxes,
    subject: &str,
    parameters: &Value,
    extra: Value,
) -> Vec<Value> {
    let a = alice.jmap_id.as_str();
    let mut submission = json!({
        "emailId": "#draft",
        "identityId": boxes.identity,
        "envelope": {
            "mailFrom": {"email": User::Alice.address(), "parameters": parameters},
            "rcptTo": [{"email": User::Bob.address()}]
        }
    });
    if let (Some(object), Value::Object(extra)) = (submission.as_object_mut(), extra) {
        object.extend(extra);
    }
    let drafts = &boxes.drafts;
    let sent = &boxes.sent;
    call(
        backend,
        alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"draft": {
                "mailboxIds": {drafts: true},
                "keywords": {"$draft": true, "$seen": true},
                "from": [{"email": User::Alice.address(), "name": "Alice"}],
                "to": [{"email": User::Bob.address()}],
                "subject": subject,
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "bodyValues": {"t": {"value": "Envío retenido del bridge."}}
            }}}, "0"],
            ["EmailSubmission/set", {"accountId": a,
              "create": {"send": submission},
              "onSuccessUpdateEmail": {"#send": {
                  "keywords/$draft": null,
                  format!("mailboxIds/{drafts}"): null,
                  format!("mailboxIds/{sent}"): true
              }}}, "1"]
        ]),
    )
    .await
}

fn created_submission(sending: &[Value]) -> (String, String) {
    let email = sending[0][1]["created"]["draft"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {sending:?}"))
        .to_owned();
    let send = &sending[1][1]["created"]["send"];
    assert_eq!(send["undoStatus"], "pending", "{sending:?}");
    let submission = send["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not held: {sending:?}"))
        .to_owned();
    // Filed in Sent at once, as with Stalwart.
    assert!(sending[2][1]["updated"][&email].is_null(), "{sending:?}");
    (email, submission)
}

async fn submission(backend: &JmapBackend, alice: &AccountContext, id: &str) -> Value {
    let a = alice.jmap_id.as_str();
    let got = call(
        backend,
        alice,
        json!([["EmailSubmission/get", {"accountId": a, "ids": [id]}, "0"]]),
    )
    .await;
    got[0][1]["list"][0].clone()
}

/// Waits until the submission is no longer pending.
async fn settled(backend: &JmapBackend, alice: &AccountContext, id: &str) -> Value {
    for _ in 0..60 {
        let object = submission(backend, alice, id).await;
        if object["undoStatus"] != "pending" {
            return object;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("{id} still pending")
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn held_messages() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("held.db");
    let store = Store::open(&path).await.unwrap();
    let backend = new_backend(&store, server);
    let shutdown = running(&backend);
    let key = AccountKey::new(
        nebula_crypto::AccountId::new(b"login:alice").unwrap(),
        nebula_crypto::DataKey::generate().unwrap(),
    );
    let alice = start_with(&backend, &store, server, User::Alice, key.duplicate()).await;
    let a = alice.jmap_id.as_str();
    let identities = call(
        &backend,
        &alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let boxes = Mailboxes {
        drafts: mailbox(&backend, &alice, "drafts").await,
        sent: mailbox(&backend, &alice, "sent").await,
        identity: identities[0][1]["list"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut alice_emails = Vec::new();
    let mut subjects = Vec::new();

    // Undo send: held 4 s, then it goes out.
    let undo_subject = format!("nebula-held-undo {stamp}");
    let sending = submit(
        &backend,
        &alice,
        &boxes,
        &undo_subject,
        &json!({"HOLDFOR": "4"}),
    )
    .await;
    let (email, undo) = created_submission(&sending);
    alice_emails.push(email.clone());
    subjects.push(undo_subject.clone());
    // The web app's «Programados» request (schedule.ts).
    let listed = call(
        &backend,
        &alice,
        json!([
            ["EmailSubmission/query", {"accountId": a, "filter": {"undoStatus": "pending"},
              "sort": [{"property": "sentAt", "isAscending": true}], "limit": 200}, "0"],
            ["EmailSubmission/get", {"accountId": a,
              "#ids": {"resultOf": "0", "name": "EmailSubmission/query", "path": "/ids"}}, "1"]
        ]),
    )
    .await;
    assert_eq!(listed[0][1]["ids"], json!([undo]), "{listed:?}");
    let object = &listed[1][1]["list"][0];
    assert_eq!(object["emailId"], email, "{listed:?}");
    assert_eq!(
        object["envelope"]["mailFrom"]["parameters"]["HOLDFOR"], "4",
        "{listed:?}"
    );
    // Can't be destroyed while held (it would still go out).
    let destroy = call(
        &backend,
        &alice,
        json!([["EmailSubmission/set", {"accountId": a, "destroy": [undo]}, "0"]]),
    )
    .await;
    assert_eq!(
        destroy[0][1]["notDestroyed"][&undo]["type"], "forbidden",
        "{destroy:?}"
    );

    // Scheduled for an hour from now, then canceled.
    let later_subject = format!("nebula-held-later {stamp}");
    let in_an_hour = utc(SystemTime::now() + Duration::from_hours(1));
    let sending = submit(
        &backend,
        &alice,
        &boxes,
        &later_subject,
        &json!({"HOLDUNTIL": in_an_hour}),
    )
    .await;
    let (later_email, later) = created_submission(&sending);
    alice_emails.push(later_email.clone());
    assert_eq!(
        sending[1][1]["created"]["send"]["sendAt"],
        in_an_hour.replace("+00:00", "Z"),
        "{sending:?}"
    );
    let cancel = json!([["EmailSubmission/set", {"accountId": a,
        "update": {later.clone(): {"undoStatus": "canceled"}}}, "0"]]);
    let canceled = call(&backend, &alice, cancel.clone()).await;
    assert!(canceled[0][1]["updated"][&later].is_null(), "{canceled:?}");
    let again = call(&backend, &alice, cancel).await;
    assert_eq!(
        again[0][1]["notUpdated"][&later]["type"], "cannotUnsend",
        "{again:?}"
    );
    assert_eq!(
        submission(&backend, &alice, &later).await["undoStatus"],
        "canceled"
    );

    // Times the bridge refuses, with Stalwart's wording.
    let refused = |parameters: Value| {
        let backend = &backend;
        let alice = &alice;
        let identity = boxes.identity.clone();
        let email = later_email.clone();
        async move {
            let response = call(
                backend,
                alice,
                json!([["EmailSubmission/set", {"accountId": a, "create": {"x": {
                    "emailId": email, "identityId": identity,
                    "envelope": {
                        "mailFrom": {"email": User::Alice.address(), "parameters": parameters},
                        "rcptTo": [{"email": User::Bob.address()}]
                    }
                }}}, "0"]]),
            )
            .await;
            response[0][1]["notCreated"]["x"].clone()
        }
    };
    let past =
        refused(json!({"HOLDUNTIL": utc(SystemTime::now() - Duration::from_hours(1))})).await;
    assert_eq!(past["type"], "forbiddenMailFrom", "{past:?}");
    let far =
        refused(json!({"HOLDUNTIL": utc(SystemTime::now() + Duration::from_hours(8 * 24))})).await;
    assert_eq!(far["type"], "forbiddenMailFrom", "{far:?}");
    assert!(
        far["description"]
            .as_str()
            .unwrap()
            .contains("exceeds maximum of"),
        "{far:?}"
    );
    let both = refused(json!({"HOLDFOR": "5", "HOLDUNTIL": in_an_hour})).await;
    assert_eq!(both["type"], "invalidProperties", "{both:?}");

    // The undo one went out.
    let object = settled(&backend, &alice, &undo).await;
    assert_eq!(object["undoStatus"], "final", "{object:?}");
    let reply = object["deliveryStatus"][User::Bob.address()]["smtpReply"]
        .as_str()
        .unwrap();
    assert!(reply.starts_with('2'), "{object:?}");

    // Held 3 s, and the bridge stops and starts again before that.
    let restart_subject = format!("nebula-held-restart {stamp}");
    let sending = submit(
        &backend,
        &alice,
        &boxes,
        &restart_subject,
        &json!({"HOLDFOR": "3"}),
    )
    .await;
    let (email, restarted) = created_submission(&sending);
    alice_emails.push(email);
    subjects.push(restart_subject.clone());
    shutdown.cancel();
    backend.account_stopped(alice.id).await;
    drop(backend);
    drop(store);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let store = Store::open(&path).await.unwrap();
    let backend = new_backend(&store, server);
    let shutdown = running(&backend);
    let alice = start_with(&backend, &store, server, User::Alice, key.duplicate()).await;
    let object = settled(&backend, &alice, &restarted).await;
    assert_eq!(object["undoStatus"], "final", "{object:?}");

    // Due while alice is signed out: it waits, then goes at her next
    // sign-in.
    let waiting_subject = format!("nebula-held-signed-out {stamp}");
    let sending = submit(
        &backend,
        &alice,
        &boxes,
        &waiting_subject,
        &json!({"HOLDFOR": "1"}),
    )
    .await;
    let (email, waiting) = created_submission(&sending);
    alice_emails.push(email);
    subjects.push(waiting_subject.clone());
    backend.account_stopped(alice.id).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let alice = start_with(&backend, &store, server, User::Alice, key.duplicate()).await;
    let object = settled(&backend, &alice, &waiting).await;
    assert_eq!(object["undoStatus"], "final", "{object:?}");

    // One copy in Sent for each (the filed draft, no extra APPEND), and
    // bob got the three that went out.
    for subject in &subjects {
        assert_eq!(
            find(&backend, &alice, subject, Duration::from_secs(30))
                .await
                .len(),
            1,
            "{subject}"
        );
    }
    let delivery = server.delivery();
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, delivery);
    let bob = super::start(&bob_backend, &bob_store, delivery, User::Bob).await;
    let mut at_bob = Vec::new();
    for subject in &subjects {
        let found = find(&bob_backend, &bob, subject, Duration::from_secs(30)).await;
        assert_eq!(found.len(), 1, "bob didn't get {subject}");
        at_bob.extend(found);
    }
    // The canceled one never went out.
    assert!(
        find(&bob_backend, &bob, &later_subject, Duration::from_secs(2))
            .await
            .is_empty()
    );

    // Clean up.
    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": alice_emails}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    let b = bob.jmap_id.as_str();
    let destroyed = call(
        &bob_backend,
        &bob,
        json!([["Email/set", {"accountId": b, "destroy": at_bob}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    shutdown.cancel();
    backend.account_stopped(alice.id).await;
    bob_backend.account_stopped(bob.id).await;
}

/// `sendWhenSignedOut` on a held message: set on create, returned by
/// get, changed while pending, refused once it isn't; and refused
/// everywhere when the bridge doesn't send after sign-out.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn send_when_signed_out_property() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("signed-out.db"))
        .await
        .unwrap();
    let key = || {
        AccountKey::new(
            nebula_crypto::AccountId::new(b"login:alice").unwrap(),
            nebula_crypto::DataKey::generate().unwrap(),
        )
    };
    let in_an_hour = utc(SystemTime::now() + Duration::from_hours(1));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut alice_emails = Vec::new();

    // Allowed.
    let backend = new_backend_full(
        &store,
        server,
        |_| {},
        |config| {
            config.hold.send_while_signed_out = true;
        },
    );
    let alice = start_with(&backend, &store, server, User::Alice, key()).await;
    let a = alice.jmap_id.as_str();
    let boxes = Mailboxes {
        drafts: mailbox(&backend, &alice, "drafts").await,
        sent: mailbox(&backend, &alice, "sent").await,
        identity: call(
            &backend,
            &alice,
            json!([["Identity/get", {"accountId": a}, "0"]]),
        )
        .await[0][1]["list"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
    };
    let sending = submit_with(
        &backend,
        &alice,
        &boxes,
        &format!("nebula-held-flag {stamp}"),
        &json!({"HOLDUNTIL": in_an_hour}),
        json!({SIGNED_OUT: true}),
    )
    .await;
    let (email, flagged) = created_submission(&sending);
    alice_emails.push(email);
    assert_eq!(
        sending[1][1]["created"]["send"][SIGNED_OUT], true,
        "{sending:?}"
    );
    assert_eq!(
        submission(&backend, &alice, &flagged).await[SIGNED_OUT],
        true
    );
    assert_eq!(
        store
            .held_while_signed_out(alice.id, UnixTime::now().unwrap(), Duration::from_hours(1))
            .await
            .unwrap(),
        1
    );
    // Turned off and on again while pending (the key is a JSON pointer).
    let pointer = SIGNED_OUT.replace('/', "~1");
    let set = |on: bool| {
        let backend = &backend;
        let alice = &alice;
        let flagged = flagged.clone();
        let pointer = pointer.clone();
        async move {
            call(
                backend,
                alice,
                json!([["EmailSubmission/set", {"accountId": a,
                    "update": {flagged: {pointer: on}}}, "0"]]),
            )
            .await
        }
    };
    let off = set(false).await;
    assert!(off[0][1]["updated"][&flagged].is_null(), "{off:?}");
    assert_eq!(
        submission(&backend, &alice, &flagged).await[SIGNED_OUT],
        false
    );
    let on = set(true).await;
    assert!(on[0][1]["updated"][&flagged].is_null(), "{on:?}");
    // Canceled: it can no longer change.
    call(
        &backend,
        &alice,
        json!([["EmailSubmission/set", {"accountId": a,
            "update": {flagged.clone(): {"undoStatus": "canceled"}}}, "0"]]),
    )
    .await;
    let late = set(false).await;
    assert_eq!(
        late[0][1]["notUpdated"][&flagged]["type"], "invalidProperties",
        "{late:?}"
    );
    assert_eq!(
        submission(&backend, &alice, &flagged).await[SIGNED_OUT],
        false
    );
    backend.account_stopped(alice.id).await;
    drop(backend);

    // Not allowed: asking for it is refused, and get doesn't show it.
    let backend = new_backend(&store, server);
    let alice = start_with(&backend, &store, server, User::Alice, key()).await;
    let refused = submit_with(
        &backend,
        &alice,
        &boxes,
        &format!("nebula-held-flag-off {stamp}"),
        &json!({"HOLDUNTIL": in_an_hour}),
        json!({SIGNED_OUT: true}),
    )
    .await;
    let error = &refused[1][1]["notCreated"]["send"];
    assert_eq!(error["type"], "invalidProperties", "{refused:?}");
    assert_eq!(error["properties"], json!([SIGNED_OUT]), "{refused:?}");
    if let Some(draft) = refused[0][1]["created"]["draft"]["id"].as_str() {
        alice_emails.push(draft.to_owned());
    }
    let sending = submit(
        &backend,
        &alice,
        &boxes,
        &format!("nebula-held-plain {stamp}"),
        &json!({"HOLDUNTIL": in_an_hour}),
    )
    .await;
    let (email, plain) = created_submission(&sending);
    alice_emails.push(email);
    assert!(
        submission(&backend, &alice, &plain)
            .await
            .get(SIGNED_OUT)
            .is_none(),
        "shown where it can't be used"
    );
    let update = call(
        &backend,
        &alice,
        json!([["EmailSubmission/set", {"accountId": a,
            "update": {plain.clone(): {pointer.clone(): true}}}, "0"]]),
    )
    .await;
    assert_eq!(
        update[0][1]["notUpdated"][&plain]["type"], "invalidProperties",
        "{update:?}"
    );

    // Clean up: nothing goes out.
    call(
        &backend,
        &alice,
        json!([["EmailSubmission/set", {"accountId": a,
            "update": {plain: {"undoStatus": "canceled"}}}, "0"]]),
    )
    .await;
    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": alice_emails}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend.account_stopped(alice.id).await;
}
