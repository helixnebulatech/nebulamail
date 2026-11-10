// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! End to end through the JMAP backend against the servers of
//! `docker/compose.yaml` (Stalwart or Dovecot, see `nebula-tests`).
//!
//! Skipped unless `NEBULA_TEST_IMAP=1` (the contacts tests, in
//! `server/contacts.rs`, unless `NEBULA_TEST_DAV=1`):
//!
//! ```sh
//! NEBULA_TEST_IMAP=1 cargo test -p nebula-jmap --test server
//! ```
//!
//! Alice lists her mailboxes and inbox, writes a draft and sends it to bob
//! with `onSuccessUpdateEmail` (like the web app); the email must end up in
//! Sent without `$draft`, and bob must get it (in Stalwart: Dovecot relays
//! there). Both copies are destroyed at the end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_http::{
    AccountContext, ApiCall, Backend, BlobBody, LoginOutcome, StartReason, UploadBody,
    jmap_account_id,
};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{NewAccount, Store, UnixTime};
use nebula_tests::{ServerKind, TestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use serde_json::{Value, json};

// Contacts over CardDAV, with `NEBULA_TEST_DAV=1`.
#[path = "server/contacts.rs"]
mod contacts;

// Calendar writes and invitations over CalDAV, with
// `NEBULA_TEST_DAV=1`.
#[path = "server/calendar_set.rs"]
mod calendar_set;

// Principals and free-busy over CalDAV, with `NEBULA_TEST_DAV=1`.
#[path = "server/principals.rs"]
mod principals;
// Tasks (VTODO) over CalDAV, with
// `NEBULA_TEST_DAV=1`.
#[path = "server/tasks.rs"]
mod tasks;
// Web Push with a fake push service.
#[path = "server/web_push.rs"]
mod web_push;

// Sharing calendars and address books, with
// `NEBULA_TEST_DAV=1` (or `NEBULA_TEST_BAIKAL=1`).
#[path = "server/sharing.rs"]
mod sharing;

// Calendar defaults (zone, default alerts), text filters and
// `/queryChanges` of events and cards, with `NEBULA_TEST_DAV=1` (or
// `NEBULA_TEST_BAIKAL=1`).
#[path = "server/defaults_and_queries.rs"]
mod defaults_and_queries;

// Nextcloud's publishing, subscriptions and trash bin
// with `NEBULA_TEST_DAV=1` (elsewhere: none offered).
#[path = "server/nextcloud_extras.rs"]
mod nextcloud_extras;

// Calendars on one DAV server and contacts on another (`contacts_url`, as
// Fastmail), with `NEBULA_TEST_SPLIT_DAV=1`: Radicale and Baïkal.
#[path = "server/split_dav.rs"]
mod split_dav;

#[path = "server/held.rs"]
mod held;

// RFC 8620 §5 and RFC 9425 rules (repeated ids, maxChanges, collations,
// Quota), with `NEBULA_TEST_IMAP=1`.
#[path = "server/rfc_limits.rs"]
mod rfc_limits;

// Email/parse, header forms, Mailbox/queryChanges, Email/copy and
// Blob/copy, with `NEBULA_TEST_IMAP=1`.
#[path = "server/mail_methods.rs"]
mod mail_methods;

fn new_backend(store: &Store, server: TestServer) -> JmapBackend {
    new_backend_with(store, server, |_| {})
}

fn new_backend_with(
    store: &Store,
    server: TestServer,
    configure: impl FnOnce(&mut MailConfig),
) -> JmapBackend {
    new_backend_full(store, server, configure, |_| {})
}

fn new_backend_full(
    store: &Store,
    server: TestServer,
    configure: impl FnOnce(&mut MailConfig),
    configure_jmap: impl FnOnce(&mut JmapConfig),
) -> JmapBackend {
    let imap_port = Port::try_from(server.imap.port).unwrap();
    let smtp_port = Port::try_from(server.smtp.port).unwrap();
    let host: net_guard::Host = nebula_tests::HOST.parse().unwrap();
    let imap = ImapServer::new(host.clone(), imap_port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![imap_port]));
    let resolver: Arc<dyn net_guard::Resolver> = Arc::new(SystemResolver::new().unwrap());
    let mut mail_config = MailConfig::new(imap);
    configure(&mut mail_config);
    let mail = Mail::new(store.clone(), mail_config, Arc::clone(&resolver)).unwrap();
    let smtp = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(SmtpCertificates::InsecureLoopbackOnly);
    let settings = SmtpSettings::new(
        SmtpClient::new(smtp).unwrap(),
        Target::new(host, smtp_port),
        Policy::new(
            PortPolicy::only(vec![smtp_port]),
            NetworkAccess::PrivateAllowed,
        ),
    );
    let mut config = JmapConfig::new(settings);
    config.email_domain = Some(nebula_tests::DOMAIN.to_owned());
    configure_jmap(&mut config);
    JmapBackend::new(store.clone(), mail, resolver, config)
}

/// Nextcloud keeps deleted calendars in its trash bin, and they count
/// toward its limit of calendars per user: what earlier runs deleted goes
/// for good before a test logs in (a throwaway test server).
async fn empty_nextcloud_trash_bin(user: User) {
    let Some(dav) = nebula_tests::DavTestServer::selected() else {
        return;
    };
    if dav.kind != nebula_tests::DavKind::Nextcloud {
        return;
    }
    let config = nebula_dav::DavServer::new(&dav.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials = nebula_dav::Credentials::new(dav.login(user), dav.password(user)).unwrap();
    let client = nebula_dav::DavClient::new(config, Some(credentials)).unwrap();
    let Ok(found) = client.discover().await else {
        return;
    };
    for home in &found.calendar_homes {
        let Ok(bin) = client
            .trash_bin(home, nebula_dav::TrashContents::HrefsOnly)
            .await
        else {
            continue;
        };
        for href in bin
            .calendars
            .iter()
            .map(|c| &c.href)
            .chain(bin.events.iter().map(|e| &e.href))
        {
            let _ = client.delete_forever(href).await;
        }
    }
}

/// Logs a user in as the HTTP layer would.
async fn start(
    backend: &JmapBackend,
    store: &Store,
    server: TestServer,
    user: User,
) -> AccountContext {
    empty_nextcloud_trash_bin(user).await;
    let password = || server.password(user);
    let user = user.login();
    let login = user.parse().unwrap();
    let outcome = backend.verify_login(&login, &password()).await;
    let LoginOutcome::Success { email } = outcome else {
        panic!("login failed: {outcome:?}")
    };
    let account = store
        .get_or_create_account(
            NewAccount {
                login: user,
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
        login: user.to_owned(),
        email,
    };
    let key = nebula_crypto::AccountKey::new(
        nebula_crypto::AccountId::new(format!("login:{user}").as_bytes()).unwrap(),
        nebula_crypto::DataKey::generate().unwrap(),
    );
    backend
        .account_started(&context, password(), key, StartReason::Login)
        .await
        .unwrap();
    context
}

/// Runs one request; returns the method responses as JSON.
async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    call_in(
        backend,
        account,
        nebula_store::GrantId::from_raw(1).unwrap(),
        calls,
    )
    .await
}

/// [`call`] in the login session `session` (push subscriptions are per
/// session).
async fn call_in(
    backend: &JmapBackend,
    account: &AccountContext,
    session: nebula_store::GrantId,
    calls: Value,
) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:mail",
            "urn:ietf:params:jmap:submission",
            "urn:ietf:params:jmap:quota",
            "urn:ietf:params:jmap:mdn"
        ],
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        &[
            Capability::Core,
            Capability::Mail,
            Capability::Submission,
            Capability::Quota,
            Capability::Mdn,
        ],
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                session,
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

/// The id of the mailbox with `role`, waiting for the first sync.
async fn mailbox(backend: &JmapBackend, account: &AccountContext, role: &str) -> String {
    let id = account.jmap_id.as_str();
    for _ in 0..60 {
        let responses = call(
            backend,
            account,
            json!([["Mailbox/get", {"accountId": id, "properties": ["role", "name"]}, "0"]]),
        )
        .await;
        let found = responses[0][1]["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == role)
            .map(|m| m["id"].as_str().unwrap().to_owned());
        if let Some(found) = found {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("no {role} mailbox")
}

/// Ids of the account's emails with this subject (waits up to `wait`).
async fn find(
    backend: &JmapBackend,
    account: &AccountContext,
    subject: &str,
    wait: Duration,
) -> Vec<String> {
    let id = account.jmap_id.as_str();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let responses = call(
            backend,
            account,
            json!([["Email/query", {"accountId": id, "filter": {"subject": subject}}, "0"]]),
        )
        .await;
        let ids: Vec<String> = responses[0][1]["ids"]
            .as_array()
            .map(|ids| ids.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
            .unwrap_or_default();
        if !ids.is_empty() || tokio::time::Instant::now() > deadline {
            return ids;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn alice_writes_and_sends_to_bob() {
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
    let sent = mailbox(&backend, &alice, "sent").await;

    // The web app's inbox request.
    let list = call(
        &backend,
        &alice,
        json!([
            ["Email/query", {"accountId": a, "filter": {"inMailbox": inbox}, "collapseThreads": true,
              "calculateTotal": true, "limit": 5,
              "sort": [{"property": "receivedAt", "isAscending": false}]}, "0"],
            ["Email/get", {"accountId": a, "#ids": {"resultOf": "0", "name": "Email/query", "path": "/ids"},
              "properties": ["threadId", "subject", "from", "preview", "keywords", "mailboxIds"]}, "1"],
            ["Thread/get", {"accountId": a, "#ids": {"resultOf": "1", "name": "Email/get", "path": "/list/*/threadId"}}, "2"]
        ]),
    )
    .await;
    assert_eq!(list[0][0], "Email/query", "{list:?}");
    assert_eq!(list[1][0], "Email/get", "{list:?}");
    assert_eq!(list[2][0], "Thread/get", "{list:?}");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-jmap-test {stamp}");
    let identities = call(
        &backend,
        &alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let identity = identities[0][1]["list"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        identities[0][1]["list"][0]["email"], "alice@nebulamail.test",
        "{identities:?}"
    );

    let sending = call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"draft": {
                "mailboxIds": {drafts.clone(): true},
                "keywords": {"$draft": true, "$seen": true},
                "from": [{"email": "alice@nebulamail.test", "name": "Alice"}],
                "to": [{"email": "bob@nebulamail.test"}],
                "subject": subject,
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "htmlBody": [{"partId": "h", "type": "text/html"}],
                "bodyValues": {"t": {"value": "Prueba del bridge."}, "h": {"value": "<p>Prueba del bridge.</p>"}}
            }}}, "0"],
            ["EmailSubmission/set", {"accountId": a,
              "create": {"send": {"emailId": "#draft", "identityId": identity}},
              "onSuccessUpdateEmail": {"#send": {
                  "keywords/$draft": null,
                  format!("mailboxIds/{drafts}"): null,
                  format!("mailboxIds/{sent}"): true
              }}}, "1"]
        ]),
    )
    .await;
    let email = sending[0][1]["created"]["draft"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {sending:?}"))
        .to_owned();
    let submission = sending[1][1]["created"]["send"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not sent: {sending:?}"))
        .to_owned();
    assert_eq!(
        sending[2][0], "Email/set",
        "implicit Email/set: {sending:?}"
    );
    assert!(sending[2][1]["updated"][&email].is_null(), "{sending:?}");

    // The web's request for the delivery status in Sent (delivery.ts):
    // `EmailSubmission/query` by `emailIds`, then `/get` by `#ids`.
    let statuses = call(
        &backend,
        &alice,
        json!([
            ["EmailSubmission/query", {"accountId": a, "filter": {"emailIds": [email.clone()]}}, "0"],
            ["EmailSubmission/get", {"accountId": a,
              "#ids": {"resultOf": "0", "name": "EmailSubmission/query", "path": "/ids"},
              "properties": ["emailId", "deliveryStatus", "sendAt", "undoStatus"]}, "1"]
        ]),
    )
    .await;
    assert_eq!(statuses[0][0], "EmailSubmission/query", "{statuses:?}");
    assert_eq!(statuses[0][1]["ids"], json!([submission]), "{statuses:?}");
    assert_eq!(statuses[0][1]["canCalculateChanges"], true, "{statuses:?}");
    let object = &statuses[1][1]["list"][0];
    assert_eq!(object["emailId"], email, "{statuses:?}");
    assert_eq!(object["undoStatus"], "final", "{statuses:?}");
    assert!(
        object["deliveryStatus"]["bob@nebulamail.test"].is_object(),
        "{statuses:?}"
    );

    let after = call(
        &backend,
        &alice,
        json!([["Email/get", {"accountId": a, "ids": [email], "properties": ["mailboxIds", "keywords"]}, "0"]]),
    )
    .await;
    let object = &after[0][1]["list"][0];
    assert_eq!(
        object["mailboxIds"],
        json!({sent.clone(): true}),
        "{after:?}"
    );
    assert!(object["keywords"].get("$draft").is_none(), "{after:?}");
    // Only one copy in Sent: the moved draft (no extra APPEND). Stalwart
    // indexes for search in a background task, so a SEARCH right after the
    // move may find nothing yet (seen in CI): wait for the first hit, then
    // look again a bit later, so a second copy indexed later still counts.
    assert_eq!(
        find(&backend, &alice, &subject, Duration::from_secs(30))
            .await
            .len(),
        1
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        find(&backend, &alice, &subject, Duration::ZERO).await.len(),
        1
    );

    // Bob got it, on the server that delivers (its own bridge and store).
    let delivery = server.delivery();
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, delivery);
    let bob = start(&bob_backend, &bob_store, delivery, User::Bob).await;
    let at_bob = find(&bob_backend, &bob, &subject, Duration::from_secs(30)).await;
    assert_eq!(at_bob.len(), 1, "bob didn't get the message");

    // Clean up both copies.
    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
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
    backend.account_stopped(alice.id).await;
    bob_backend.account_stopped(bob.id).await;
}

/// A send whose request goes away mid-way (the tab closed, the HTTP
/// timeout of a slow transfer) still finishes, and its row gets the
/// server's reply instead of staying "outcome unknown".
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn a_send_finishes_when_its_request_goes_away() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let identities = call(
        &backend,
        &alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let identity = identities[0][1]["list"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Big enough that DATA takes a moment, under Postfix's 10 MB.
    let attachment = base64_lines(&noise(5_000_000, 0x5eed_c0de));
    let raw = format!(
        "From: {alice}\r\nTo: {bob}\r\nSubject: Adjunto {stamp}\r\n\
Message-ID: <cut.{stamp}@nebulamail.test>\r\nMIME-Version: 1.0\r\n\
Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\n{attachment}\r\n",
        alice = User::Alice.address(),
        bob = User::Bob.address(),
    )
    .into_bytes();
    let uploaded = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(raw)], 50_000_000),
        )
        .await
        .unwrap();
    let subject = format!("nebula-cut-test {stamp}");
    let created = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "create": {"draft": {
            "mailboxIds": {drafts.clone(): true},
            "keywords": {"$draft": true, "$seen": true},
            "from": [{"email": User::Alice.address(), "name": "Alice"}],
            "to": [{"email": User::Bob.address()}],
            "subject": subject,
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "Se corta la petición."}},
            "attachments": [{"blobId": uploaded.blob_id.as_str(), "type": "message/rfc822",
                             "name": "adjunto.eml", "disposition": "attachment"}]
        }}}, "0"]]),
    )
    .await;
    let email = created[0][1]["created"]["draft"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {created:?}"))
        .to_owned();

    let statuses = || {
        call(
            &backend,
            &alice,
            json!([
                ["EmailSubmission/query", {"accountId": a, "filter": {"emailIds": [email.clone()]}}, "0"],
                ["EmailSubmission/get", {"accountId": a,
                  "#ids": {"resultOf": "0", "name": "EmailSubmission/query", "path": "/ids"},
                  "properties": ["deliveryStatus"]}, "1"]
            ]),
        )
    };
    let reply_of = |statuses: &[Value]| {
        statuses[1][1]["list"][0]["deliveryStatus"][User::Bob.address()]["smtpReply"]
            .as_str()
            .map(str::to_owned)
    };
    // The request is dropped as soon as its row exists (the send has
    // started), like a closed tab or a timed-out HTTP request.
    let sending = call(
        &backend,
        &alice,
        json!([["EmailSubmission/set", {"accountId": a,
          "create": {"send": {"emailId": email.clone(), "identityId": identity}}}, "0"]]),
    );
    let row_exists = async {
        loop {
            if reply_of(&statuses().await).is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let finished_first = tokio::select! {
        _ = sending => true,
        () = row_exists => false,
    };
    if finished_first {
        eprintln!("the send finished before its request could be dropped");
    }
    // It still finishes, and says so.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let reply = loop {
        let reply = reply_of(&statuses().await).unwrap_or_default();
        if !reply.starts_with("unknown") || tokio::time::Instant::now() > deadline {
            break reply;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(reply.starts_with("250"), "{reply}");

    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    let delivery = server.delivery();
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, delivery);
    let bob = start(&bob_backend, &bob_store, delivery, User::Bob).await;
    let at_bob = find(&bob_backend, &bob, &subject, Duration::from_secs(30)).await;
    assert_eq!(at_bob.len(), 1, "bob didn't get the message");
    let b = bob.jmap_id.as_str();
    call(
        &bob_backend,
        &bob,
        json!([["Email/set", {"accountId": b, "destroy": at_bob}, "0"]]),
    )
    .await;
    backend.account_stopped(alice.id).await;
    bob_backend.account_stopped(bob.id).await;
}

/// `Mailbox/changes` with `updatedProperties`, `Email/queryChanges` and
/// `Thread/get` with `ids: null`.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn changes_and_query_changes() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("changes.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    mailbox(&backend, &alice, "inbox").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let created = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "create": {"m": {"name": format!("nebula-jmap-changes-{stamp}")}}}, "0"]]),
    )
    .await;
    let folder = created[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("mailbox not created: {created:?}"))
        .to_owned();

    let before = call(
        &backend,
        &alice,
        json!([
            ["Mailbox/get", {"accountId": a, "ids": [folder.clone()], "properties": ["id"]}, "0"],
            ["Email/query", {"accountId": a, "filter": {"inMailbox": folder.clone()},
              "sort": [{"property": "receivedAt", "isAscending": false}]}, "1"]
        ]),
    )
    .await;
    let mailbox_state = before[0][1]["state"].as_str().unwrap().to_owned();
    let query_state = before[1][1]["queryState"].as_str().unwrap().to_owned();
    assert_eq!(before[1][1]["canCalculateChanges"], true, "{before:?}");

    {
        let made = call(
            &backend,
            &alice,
            json!([["Email/set", {"accountId": a, "create": {"e": {
                "mailboxIds": {folder.clone(): true},
                "from": [{"email": "alice@nebulamail.test"}],
                "subject": format!("nebula-jmap-changes {stamp}"),
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "bodyValues": {"t": {"value": "cambios"}}
            }}}, "0"]]),
        )
        .await;
        let email = made[0][1]["created"]["e"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("email not created: {made:?}"))
            .to_owned();
        let thread = made[0][1]["created"]["e"]["threadId"]
            .as_str()
            .unwrap()
            .to_owned();

        let changes = call(
            &backend,
            &alice,
            json!([
                ["Mailbox/changes", {"accountId": a, "sinceState": mailbox_state}, "0"],
                ["Email/queryChanges", {"accountId": a, "sinceQueryState": query_state,
                  "filter": {"inMailbox": folder.clone()},
                  "sort": [{"property": "receivedAt", "isAscending": false}],
                  "calculateTotal": true}, "1"],
                ["Thread/get", {"accountId": a, "ids": null}, "2"]
            ]),
        )
        .await;
        // Only the counters of the folder changed.
        assert_eq!(changes[0][0], "Mailbox/changes", "{changes:?}");
        assert!(
            changes[0][1]["updated"]
                .as_array()
                .unwrap()
                .contains(&json!(folder)),
            "{changes:?}"
        );
        assert_eq!(
            changes[0][1]["updatedProperties"],
            json!([
                "totalEmails",
                "unreadEmails",
                "totalThreads",
                "unreadThreads"
            ]),
            "{changes:?}"
        );
        assert_eq!(changes[1][0], "Email/queryChanges", "{changes:?}");
        assert_eq!(
            changes[1][1]["added"],
            json!([{"id": email, "index": 0}]),
            "{changes:?}"
        );
        assert_eq!(changes[1][1]["total"], 1, "{changes:?}");
        // All threads, unless the test account has too many.
        match changes[2][0].as_str() {
            Some("Thread/get") => assert!(
                changes[2][1]["list"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["id"] == thread),
                "{changes:?}"
            ),
            _ => assert_eq!(changes[2][1]["type"], "requestTooLarge", "{changes:?}"),
        }
    }
    let destroyed = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "destroy": [folder], "onDestroyRemoveEmails": true}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend.account_stopped(alice.id).await;
}

/// The ids of a query after `Email/queryChanges` (RFC 8620 §5.6): every
/// removed id goes, then the added ones go in at their index, lowest first.
fn apply_query_changes(ids: &[String], changes: &Value) -> Vec<String> {
    let removed: Vec<&str> = changes["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let mut result: Vec<String> = ids
        .iter()
        .filter(|id| !removed.contains(&id.as_str()))
        .cloned()
        .collect();
    let mut added: Vec<(usize, String)> = changes["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                usize::try_from(item["index"].as_u64().unwrap()).unwrap(),
                item["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    added.sort();
    for (index, id) in added {
        result.retain(|known| *known != id);
        result.insert(index.min(result.len()), id);
    }
    result
}

/// `Email/queryChanges` with `collapseThreads`, as the web's lists use it
/// (apps/web/src/mail/sync.ts): a reply that becomes its thread's row
/// takes the old row's place, and gives it back when it goes. Applying the
/// changes must give what `Email/query` says (the bug
/// Stalwart had until 1.0.2).
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn query_changes_with_collapsed_threads() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("threads.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    mailbox(&backend, &alice, "inbox").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let created = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "create": {"m": {"name": format!("nebula-jmap-threads-{stamp}")}}}, "0"]]),
    )
    .await;
    let folder = created[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("mailbox not created: {created:?}"))
        .to_owned();
    let query = json!({
        "accountId": a,
        "filter": {"inMailbox": folder.clone()},
        "sort": [{"property": "receivedAt", "isAscending": false}],
        "collapseThreads": true,
    });
    // An email in the folder, `minutes` before now (its place in the list).
    let email = |subject: String, minutes: u64, reply_to: Option<String>| {
        let received = SystemTime::now() - Duration::from_secs(minutes * 60);
        let seconds = received.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let received_at =
            nebula_jmap_proto::UtcDate::from_unix_timestamp(i64::try_from(seconds).unwrap())
                .unwrap()
                .to_string();
        let message_id = format!("{subject}-{stamp}@threads.example").replace([' ', ':'], "-");
        let mut create = json!({
            "mailboxIds": {folder.clone(): true},
            "from": [{"email": "[EMAIL]"}],
            "subject": format!("{subject} {stamp}"),
            "messageId": [message_id.clone()],
            "receivedAt": received_at,
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "hilos"}}
        });
        if let Some(parent) = reply_to {
            create["inReplyTo"] = json!([parent.clone()]);
            create["references"] = json!([parent]);
        }
        (create, message_id)
    };
    let create = |create: Value| {
        let backend = &backend;
        let alice = &alice;
        async move {
            let made = call(
                backend,
                alice,
                json!([["Email/set", {"accountId": a, "create": {"e": create}}, "0"]]),
            )
            .await;
            made[0][1]["created"]["e"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("email not created: {made:?}"))
                .to_owned()
        }
    };
    let ids_of = |response: &Value| -> Vec<String> {
        response["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    };

    let (first, first_message) = email("A".to_owned(), 10, None);
    let older = create(first).await;
    let (second, _) = email("B".to_owned(), 5, None);
    let newer = create(second).await;
    let before = call(
        &backend,
        &alice,
        json!([["Email/query", query.clone(), "0"]]),
    )
    .await;
    let mut shown = ids_of(&before[0][1]);
    assert_eq!(shown, [newer.clone(), older.clone()], "{before:?}");
    let mut state = before[0][1]["queryState"].as_str().unwrap().to_owned();

    // Each step: the changes since the last state, applied to what the
    // client shows, must give the query's answer.
    let step = async |shown: &mut Vec<String>, state: &mut String, expected: &[&String]| {
        let mut changes_args = query.clone();
        changes_args["sinceQueryState"] = json!(*state);
        changes_args["calculateTotal"] = json!(true);
        let answer = call(
            &backend,
            &alice,
            json!([
                ["Email/queryChanges", changes_args, "0"],
                ["Email/query", query.clone(), "1"]
            ]),
        )
        .await;
        assert_eq!(answer[0][0], "Email/queryChanges", "{answer:?}");
        *shown = apply_query_changes(shown, &answer[0][1]);
        assert_eq!(*shown, ids_of(&answer[1][1]), "{answer:?}");
        assert_eq!(shown.iter().collect::<Vec<_>>(), expected, "{answer:?}");
        assert_eq!(answer[0][1]["total"], json!(expected.len()), "{answer:?}");
        *state = answer[0][1]["newQueryState"].as_str().unwrap().to_owned();
    };

    // A reply to A, the newest: it is A's row now.
    let (third, _) = email("Re: A".to_owned(), 1, Some(first_message));
    let reply = create(third).await;
    step(&mut shown, &mut state, &[&reply, &newer]).await;

    // The reply goes: A is back, after B. The reply's thread is gone with
    // it, but the Thread log has it (`email/query.rs`).
    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [reply.clone()]}, "0"]]),
    )
    .await;
    assert_eq!(
        destroyed[0][1]["destroyed"],
        json!([reply]),
        "{destroyed:?}"
    );
    step(&mut shown, &mut state, &[&newer, &older]).await;

    // A new thread on top.
    let (fourth, _) = email("C".to_owned(), 0, None);
    let top = create(fourth).await;
    step(&mut shown, &mut state, &[&top, &newer, &older]).await;

    let removed = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "destroy": [folder], "onDestroyRemoveEmails": true}, "0"]]),
    )
    .await;
    assert!(removed[0][1]["destroyed"].is_array(), "{removed:?}");
    backend.account_stopped(alice.id).await;
}

/// The names of a mailbox with `name` (and its id) as a fresh store of
/// `server` sees them after the first sync, or `None`.
async fn mailbox_named(server: TestServer, name: &str) -> Option<(String, Option<String>)> {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("fresh.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    mailbox(&backend, &alice, "inbox").await;
    let list = call(
        &backend,
        &alice,
        json!([["Mailbox/get", {"accountId": alice.jmap_id.as_str(), "properties": ["name", "parentId"]}, "0"]]),
    )
    .await;
    let found = list[0][1]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .map(|m| {
            (
                m["id"].as_str().unwrap().to_owned(),
                m["parentId"].as_str().map(str::to_owned),
            )
        });
    backend.account_stopped(alice.id).await;
    found
}

/// Mailbox names outside ASCII, created, renamed and read back by a
/// fresh sync: modified UTF-7 with rev1 servers, UTF-8 with UTF8=ACCEPT or
/// an IMAP4rev2-only server (RFC 9051 §5.1; `NEBULA_TEST_IMAP_PORT` with
/// `apps/web/e2e-bridge/rev2-proxy.ts`).
#[tokio::test(flavor = "multi_thread")]
async fn non_ascii_mailbox_names() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("names.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    mailbox(&backend, &alice, "inbox").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let parent_name = format!("Año {stamp}");
    // One call, the child's creation id first: the server orders the
    // creates so the parent exists before "#p" is used (RFC 8620 §5.3).
    let created = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "create": {
            "c": {"name": "Café ☕ & más", "parentId": "#p"},
            "p": {"name": parent_name}
        }}, "0"]]),
    )
    .await;
    let parent = created[0][1]["created"]["p"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("parent not created: {created:?}"))
        .to_owned();
    let child = created[0][1]["created"]["c"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("child not created: {created:?}"))
        .to_owned();

    let made = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "create": {"e": {
            "mailboxIds": {child.clone(): true},
            "from": [{"email": "nombres@example.com"}],
            "subject": format!("nombres {stamp}"),
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "hola"}}
        }}}, "0"]]),
    )
    .await;
    assert!(made[0][1]["created"]["e"]["id"].is_string(), "{made:?}");

    // What the server stored, read back from scratch.
    let (_, seen_parent) = mailbox_named(server, "Café ☕ & más")
        .await
        .expect("the child after a fresh sync");
    let (seen_parent_id, _) = mailbox_named(server, &parent_name)
        .await
        .expect("the parent after a fresh sync");
    assert_eq!(seen_parent.as_deref(), Some(seen_parent_id.as_str()));

    let renamed = call(
        &backend,
        &alice,
        json!([
            ["Mailbox/set", {"accountId": a, "update": {child.clone(): {"name": "Niño ✓"}}}, "0"],
            ["Email/query", {"accountId": a, "filter": {"inMailbox": child.clone()}}, "1"]
        ]),
    )
    .await;
    assert!(
        renamed[0][1]["updated"]
            .as_object()
            .is_some_and(|u| u.contains_key(&child)),
        "{renamed:?}"
    );
    assert_eq!(
        renamed[1][1]["ids"].as_array().map(Vec::len),
        Some(1),
        "{renamed:?}"
    );
    let (after, _) = mailbox_named(server, "Niño ✓")
        .await
        .expect("the renamed child after a fresh sync");
    // The old name is gone.
    assert!(!after.is_empty());
    assert!(mailbox_named(server, "Café ☕ & más").await.is_none());

    let destroyed = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "destroy": [child, parent], "onDestroyRemoveEmails": true}, "0"]]),
    )
    .await;
    assert_eq!(
        destroyed[0][1]["destroyed"].as_array().map(Vec::len),
        Some(2),
        "{destroyed:?}"
    );
    backend.account_stopped(alice.id).await;
}

/// Quotas from GETQUOTAROOT: announced only if the server has QUOTA; get,
/// changes and query agree with each other.
#[tokio::test(flavor = "multi_thread")]
async fn quotas() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let capabilities = backend.account_capabilities(&alice).await.unwrap();
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Quota/get", {"accountId": a, "ids": null}, "0"],
            ["Quota/query", {"accountId": a, "sort": [{"property": "name"}]}, "1"]
        ]),
    )
    .await;
    if !capabilities.quota {
        // Not announced: the methods still answer (empty), the web hides it.
        assert_eq!(responses[0][1]["list"], json!([]), "{responses:?}");
        return;
    }
    assert_eq!(responses[0][0], "Quota/get", "{responses:?}");
    let list = responses[0][1]["list"].as_array().unwrap();
    for quota in list {
        assert_eq!(quota["scope"], "account");
        assert!(quota["used"].as_u64().is_some());
        assert!(quota["resourceType"] == "octets" || quota["resourceType"] == "count");
    }
    let mut got: Vec<&str> = list.iter().map(|q| q["id"].as_str().unwrap()).collect();
    let mut queried: Vec<&str> = responses[1][1]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    got.sort_unstable();
    queried.sort_unstable();
    assert_eq!(got, queried);
    let state = responses[0][1]["state"].as_str().unwrap();
    let changes = call(
        &backend,
        &alice,
        json!([["Quota/changes", {"accountId": a, "sinceState": state}, "0"]]),
    )
    .await;
    assert_eq!(changes[0][0], "Quota/changes", "{changes:?}");
    assert_eq!(changes[0][1]["created"], json!([]));
    assert_eq!(changes[0][1]["destroyed"], json!([]));
    let unknown = call(
        &backend,
        &alice,
        json!([["Quota/changes", {"accountId": a, "sinceState": "nope"}, "0"]]),
    )
    .await;
    assert_eq!(unknown[0][1]["type"], "cannotCalculateChanges");
}

/// Previews and search snippets come from the first bytes of the text part
/// (not the whole message): a draft with a long body, matched far from the
/// start. The draft is destroyed at the end.
#[tokio::test(flavor = "multi_thread")]
async fn snippets_from_the_text_part() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let word = format!("zanahoria{stamp}");
    let text = format!("{}la {word} final.", "Relleno del borrador. ".repeat(40));
    let created = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "create": {"d": {
            "mailboxIds": {drafts.clone(): true},
            "keywords": {"$draft": true, "$seen": true},
            "from": [{"email": "alice@nebulamail.test"}],
            "subject": format!("nebula-snippet-test {stamp}"),
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "htmlBody": [{"partId": "h", "type": "text/html"}],
            "bodyValues": {"t": {"value": text}, "h": {"value": format!("<p>{text}</p>")}}
        }}}, "0"]]),
    )
    .await;
    let email = created[0][1]["created"]["d"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {created:?}"))
        .to_owned();
    let found = find(
        &backend,
        &alice,
        &format!("nebula-snippet-test {stamp}"),
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(found, std::slice::from_ref(&email));
    // A text search in a mailbox that isn't ours finds nothing; it is not
    // a server error.
    let unknown = call(
        &backend,
        &alice,
        json!([["Email/query", {"accountId": a, "filter": {"inMailbox": "m999999", "text": word}}, "0"]]),
    )
    .await;
    assert_eq!(unknown[0][0], "Email/query", "{unknown:?}");
    assert_eq!(unknown[0][1]["ids"], json!([]));

    // A second bridge (own database, empty cache) sees the draft through
    // its sync and must read only the text part from the server.
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = Store::open(&dir2.path().join("test.db")).await.unwrap();
    let backend2 = new_backend(&store2, server);
    let alice2 = start(&backend2, &store2, server, User::Alice).await;
    let a2 = alice2.jmap_id.as_str();
    let found2 = find(
        &backend2,
        &alice2,
        &format!("nebula-snippet-test {stamp}"),
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(found2.len(), 1, "{found2:?}");
    let email2 = found2[0].clone();
    let responses = call(
        &backend2,
        &alice2,
        json!([
            ["SearchSnippet/get", {"accountId": a2, "emailIds": [email2], "filter": {"body": word}}, "0"],
            ["Email/get", {"accountId": a2, "ids": [email2], "properties": ["preview"]}, "1"]
        ]),
    )
    .await;
    let snippet = responses[0][1]["list"][0]["preview"]
        .as_str()
        .unwrap_or_default();
    assert!(
        snippet.contains(&format!("<mark>{word}</mark> final.")),
        "{responses:?}"
    );
    assert!(
        !snippet.starts_with("Relleno del borrador. Relleno"),
        "{snippet:?}"
    );
    let preview = responses[1][1]["list"][0]["preview"].as_str().unwrap();
    assert!(
        preview.starts_with("Relleno del borrador."),
        "{responses:?}"
    );

    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend2.account_stopped(alice2.id).await;
    backend.account_stopped(alice.id).await;
}

/// `MDN/send` for an email that asks for one (sent to bob, once), then
/// `mdnAlreadySent`; `MDN/parse` of a message that is not a report.
#[tokio::test(flavor = "multi_thread")]
async fn read_receipts() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let created = call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "create": {
                "asks": {
                    "mailboxIds": {drafts.clone(): true},
                    "keywords": {"$seen": true},
                    "from": [{"email": "bob@nebulamail.test"}],
                    "to": [{"email": "alice@nebulamail.test"}],
                    "subject": format!("nebula-mdn-test {stamp}"),
                    "header:Disposition-Notification-To:asAddresses": [{"email": "bob@nebulamail.test"}],
                    "textBody": [{"partId": "t", "type": "text/plain"}],
                    "bodyValues": {"t": {"value": "Please confirm."}}
                },
                "plain": {
                    "mailboxIds": {drafts.clone(): true},
                    "from": [{"email": "bob@nebulamail.test"}],
                    "subject": format!("nebula-mdn-test plain {stamp}"),
                    "textBody": [{"partId": "t", "type": "text/plain"}],
                    "bodyValues": {"t": {"value": "No receipt wanted."}}
                }
            }}, "0"],
            ["Identity/get", {"accountId": a}, "1"]
        ]),
    )
    .await;
    let asks = created[0][1]["created"]["asks"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    let plain = created[0][1]["created"]["plain"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let identity = created[1][1]["list"][0]["id"].as_str().unwrap().to_owned();
    let disposition = json!({"actionMode": "manual-action",
                             "sendingMode": "mdn-sent-manually", "type": "displayed"});
    let sent = call(
        &backend,
        &alice,
        json!([["MDN/send", {"accountId": a, "identityId": identity, "send": {
            "m": {"forEmailId": asks, "disposition": disposition},
            "none": {"forEmailId": plain, "disposition": disposition}
        }, "onSuccessUpdateEmail": {"#m": {"keywords/$mdnsent": true}}}, "0"]]),
    )
    .await;
    assert_eq!(sent[0][0], "MDN/send", "{sent:?}");
    assert_eq!(
        sent[0][1]["sent"]["m"]["finalRecipient"], "rfc822;alice@nebulamail.test",
        "{sent:?}"
    );
    assert_eq!(
        sent[0][1]["notSent"]["none"]["type"], "invalidProperties",
        "{sent:?}"
    );
    assert_eq!(sent[1][0], "Email/set", "{sent:?}");
    assert!(sent[1][1]["updated"][&asks].is_null(), "{sent:?}");

    let again = call(
        &backend,
        &alice,
        json!([
            ["MDN/send", {"accountId": a, "identityId": identity, "send": {
                "m": {"forEmailId": asks, "disposition": disposition}}}, "0"],
            ["Email/get", {"accountId": a, "ids": [asks], "properties": ["blobId"]}, "1"],
            ["MDN/parse", {"accountId": a, "#blobIds": {"resultOf": "1", "name": "Email/get", "path": "/list/*/blobId"}}, "2"]
        ]),
    )
    .await;
    assert_eq!(
        again[0][1]["notSent"]["m"]["type"], "mdnAlreadySent",
        "{again:?}"
    );
    assert_eq!(
        again[2][1]["notParsable"].as_array().map(Vec::len),
        Some(1),
        "{again:?}"
    );

    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [asks, plain]}, "0"]]),
    )
    .await;

    // Bob got the report; MDN/parse reads it back (and the report is
    // deleted again).
    let delivery = server.delivery();
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, delivery);
    let bob = start(&bob_backend, &bob_store, delivery, User::Bob).await;
    let b = bob.jmap_id.as_str();
    let reports = find(
        &bob_backend,
        &bob,
        &format!("nebula-mdn-test {stamp}"),
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(reports.len(), 1, "bob didn't get the MDN");
    let parsed = call(
        &bob_backend,
        &bob,
        json!([
            ["Email/get", {"accountId": b, "ids": reports, "properties": ["blobId"]}, "0"],
            ["MDN/parse", {"accountId": b, "#blobIds": {"resultOf": "0", "name": "Email/get", "path": "/list/*/blobId"}}, "1"],
            ["Email/set", {"accountId": b, "destroy": reports}, "2"]
        ]),
    )
    .await;
    let report_blob = parsed[0][1]["list"][0]["blobId"].as_str().unwrap();
    let mdn = &parsed[1][1]["parsed"][report_blob];
    assert_eq!(mdn["disposition"]["type"], "displayed", "{parsed:?}");
    assert_eq!(
        mdn["finalRecipient"], "rfc822;alice@nebulamail.test",
        "{parsed:?}"
    );
    assert!(parsed[2][1]["destroyed"].is_array(), "{parsed:?}");
    bob_backend.account_stopped(bob.id).await;
    backend.account_stopped(alice.id).await;
}

/// Pseudo-random bytes (xorshift), so base64 lines are not repetitive.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect()
}

fn base64_lines(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut line = 0;
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(
                    ALPHABET[usize::try_from((n >> (18 - 6 * i)) & 63).unwrap()],
                ));
            } else {
                out.push('=');
            }
        }
        line += 4;
        if line >= 76 {
            out.push_str("\r\n");
            line = 0;
        }
    }
    out
}

/// A body that is downloaded whole.
async fn downloaded(
    backend: &JmapBackend,
    account: &AccountContext,
    blob: &str,
) -> (bool, String, Vec<u8>) {
    use futures::StreamExt as _;
    let blob = backend
        .download(account, &blob.parse().unwrap())
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no blob {blob}"));
    let media_type = blob.media_type.as_str().to_owned();
    match blob.body {
        BlobBody::Stream { size, mut chunks } => {
            let mut out = Vec::new();
            while let Some(piece) = chunks.next().await {
                out.extend_from_slice(&piece.unwrap());
            }
            assert_eq!(
                u64::try_from(out.len()).unwrap(),
                size,
                "Content-Length is exact"
            );
            (true, media_type, out)
        }
        BlobBody::Bytes(bytes) => (false, media_type, bytes.to_vec()),
        _ => panic!("unknown body"),
    }
}

/// A message bigger than the cache's largest item: its parts get `bs…`
/// blob ids and are streamed in pieces (BINARY and our own decoder), the
/// whole message too, with the same bytes as cutting the parsed message.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn big_messages_are_streamed() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-stream-test {stamp}");
    let attachment = noise(
        2_500_000,
        0x9E37_79B9_7F4A_7C15 ^ u64::try_from(stamp % 1000).unwrap(),
    );
    let mut text = String::new();
    for i in 0..30_000 {
        // Escapes and soft line breaks cut by the 1 MiB pieces somewhere.
        let _ = write!(
            text,
            "l=C3=ADnea {i} caf=C3=A9 con espacios   =\r\nsigue\r\n"
        );
    }
    let raw = format!(
        "From: alice@nebulamail.test\r\nTo: alice@nebulamail.test\r\nSubject: {subject}\r\n\
Message-ID: <stream.{stamp}@nebulamail.test>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"XYZ\"\r\n\r\n\
--XYZ\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
{text}\r\n--XYZ\r\nContent-Type: application/octet-stream; name=\"ruido.bin\"\r\n\
Content-Disposition: attachment; filename=\"ruido.bin\"\r\nContent-Transfer-Encoding: base64\r\n\r\n\
{}\r\n--XYZ--\r\n",
        base64_lines(&attachment)
    )
    .into_bytes();
    let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let expected_text = parsed.part(1).unwrap().decoded().into_owned();
    assert_eq!(
        parsed.part(2).unwrap().decoded().as_ref(),
        attachment.as_slice()
    );

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    // Messages over 64 KiB are not cached and so are streamed.
    let backend = new_backend_with(&store, server, |config| {
        config.raw_cache_max_item = 64 * 1024;
    });
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let uploaded = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(
                raw.chunks(300_000)
                    .map(bytes::Bytes::copy_from_slice)
                    .collect(),
                50_000_000,
            ),
        )
        .await
        .unwrap();
    let imported = call(
        &backend,
        &alice,
        json!([["Email/import", {"accountId": a, "emails": {"m": {
            "blobId": uploaded.blob_id.as_str(),
            "mailboxIds": {drafts: true},
            "keywords": {"$seen": true}
        }}}, "0"]]),
    )
    .await;
    let email = imported[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not imported: {imported:?}"))
        .to_owned();
    let got = call(
        &backend,
        &alice,
        json!([["Email/get", {"accountId": a, "ids": [email], "properties": ["blobId", "size", "textBody", "attachments"]}, "0"]]),
    )
    .await;
    let found = &got[0][1]["list"][0];
    let text_blob = found["textBody"][0]["blobId"].as_str().unwrap().to_owned();
    let file_blob = found["attachments"][0]["blobId"]
        .as_str()
        .unwrap()
        .to_owned();
    let message_blob = found["blobId"].as_str().unwrap().to_owned();
    assert!(text_blob.starts_with("bs"), "{found}");
    assert!(file_blob.starts_with("bs"), "{found}");
    assert_eq!(found["attachments"][0]["size"], attachment.len());
    assert_eq!(found["size"], raw.len());

    for (streamed, kind, bytes, what) in [
        {
            let (s, k, b) = downloaded(&backend, &alice, &file_blob).await;
            (s, k, b, "attachment")
        },
        {
            let (s, k, b) = downloaded(&backend, &alice, &text_blob).await;
            (s, k, b, "text")
        },
    ] {
        assert!(streamed, "{what} streamed");
        let expected = if what == "text" {
            &expected_text
        } else {
            &attachment
        };
        assert!(
            bytes == *expected,
            "{what}: {} bytes, expected {}",
            bytes.len(),
            expected.len()
        );
        assert!(
            kind == "application/octet-stream" || kind == "text/plain",
            "{kind}"
        );
    }
    let (streamed, kind, bytes) = downloaded(&backend, &alice, &message_blob).await;
    assert!(streamed && kind == "message/rfc822");
    assert!(
        bytes == raw,
        "whole message: {} bytes, expected {}",
        bytes.len(),
        raw.len()
    );
    // The plain part id still works (cut from the whole message).
    let plain = file_blob
        .split('_')
        .take(2)
        .collect::<Vec<_>>()
        .join("_")
        .replacen("bs", "bp", 1);
    // bp… downloads are sent in chunks now (the body keeps its share of
    // the memory budget until the last chunk), so they are streamed too.
    let (streamed, _, bytes) = downloaded(&backend, &alice, &plain).await;
    assert!(streamed && bytes == attachment);

    // Without BINARY: BODY[2] decoded by the bridge.
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = Store::open(&dir2.path().join("test.db")).await.unwrap();
    let backend2 = new_backend_with(&store2, server, |config| {
        config.raw_cache_max_item = 64 * 1024;
        config.binary_downloads = false;
    });
    let alice2 = start(&backend2, &store2, server, User::Alice).await;
    let found2 = find(&backend2, &alice2, &subject, Duration::from_secs(30)).await;
    assert_eq!(found2.len(), 1, "{found2:?}");
    let a2 = alice2.jmap_id.as_str();
    let got2 = call(
        &backend2,
        &alice2,
        json!([["Email/get", {"accountId": a2, "ids": [found2[0]], "properties": ["textBody", "attachments"]}, "0"]]),
    )
    .await;
    let found2 = &got2[0][1]["list"][0];
    for (blob, expected) in [
        (
            found2["attachments"][0]["blobId"].as_str().unwrap(),
            &attachment,
        ),
        (
            found2["textBody"][0]["blobId"].as_str().unwrap(),
            &expected_text,
        ),
    ] {
        assert!(blob.starts_with("bs"), "{blob}");
        let (streamed, _, bytes) = downloaded(&backend2, &alice2, blob).await;
        assert!(
            streamed && bytes == *expected,
            "{blob}: {} bytes",
            bytes.len()
        );
    }

    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend2.account_stopped(alice2.id).await;
    backend.account_stopped(alice.id).await;
}

/// A real message of 47–48 MB (two 17.5 MB attachments; the default
/// `max_size_upload` is 50 MB) imported and then read by a fresh bridge
/// (nothing cached) with every body property, as the web opens a big
/// email: the attachments get `bs…` blob ids (streamed), their sizes are
/// exact and the texts come out right. Prints how long each `Email/get`
/// takes and the longest another task of the same runtime (one worker)
/// waited meanwhile: on a single-threaded runtime (everything on one
/// thread, as one busy worker) a parse on the async workers shows there.
#[tokio::test]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn a_large_message_through_email_get() {
    use std::sync::Mutex;
    use std::time::Instant;

    let Some(server) = TestServer::imap() else {
        return;
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-large-test {stamp}");
    let seed = 0x9E37_79B9_7F4A_7C15 ^ u64::try_from(stamp % 1000).unwrap();
    let first = noise(17_500_000, seed);
    let second = noise(17_500_000, seed.rotate_left(29));
    let mut text = String::new();
    for i in 0..2000 {
        let _ = write!(text, "L=C3=ADnea {i}: caf=C3=A9 y m=C3=A1s.\r\n");
    }
    let raw = format!(
        "From: Bob <{bob}>\r\nTo: Alice <{alice}>\r\nSubject: {subject}\r\n\
Message-ID: <large.{stamp}@nebulamail.test>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"big\"\r\n\r\n\
--big\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
--alt\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
{text}\r\n--alt\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Dos adjuntos grandes.</p>\r\n--alt--\r\n\
--big\r\nContent-Type: application/octet-stream; name=\"uno.bin\"\r\n\
Content-Disposition: attachment; filename=\"uno.bin\"\r\nContent-Transfer-Encoding: base64\r\n\r\n\
{}\r\n--big\r\nContent-Type: application/octet-stream; name=\"dos.bin\"\r\n\
Content-Disposition: attachment; filename=\"dos.bin\"\r\nContent-Transfer-Encoding: base64\r\n\r\n\
{}\r\n--big--\r\n",
        base64_lines(&first),
        base64_lines(&second),
        bob = User::Bob.address(),
        alice = User::Alice.address(),
    )
    .into_bytes();
    assert!(
        (45 * 1024 * 1024..50_000_000).contains(&raw.len()),
        "{} bytes",
        raw.len()
    );
    let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
    let expected_text = String::from_utf8(parsed.part(2).unwrap().decoded().into_owned()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let started = Instant::now();
    let uploaded = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(
                raw.chunks(1_000_000)
                    .map(bytes::Bytes::copy_from_slice)
                    .collect(),
                50_000_000,
            ),
        )
        .await
        .unwrap();
    let imported = call(
        &backend,
        &alice,
        json!([["Email/import", {"accountId": a, "emails": {"m": {
            "blobId": uploaded.blob_id.as_str(),
            "mailboxIds": {drafts: true},
            "keywords": {"$seen": true}
        }}}, "0"]]),
    )
    .await;
    let import_time = started.elapsed();
    let email = imported[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not imported: {imported:?}"))
        .to_owned();

    // A fresh bridge: nothing in its cache (the message is too big anyway).
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = Store::open(&dir2.path().join("test.db")).await.unwrap();
    let backend2 = new_backend(&store2, server);
    let alice2 = start(&backend2, &store2, server, User::Alice).await;
    let found = find(&backend2, &alice2, &subject, Duration::from_secs(60)).await;
    assert_eq!(found.len(), 1, "{found:?}");
    let a2 = alice2.jmap_id.as_str();

    let worst = Arc::new(Mutex::new(Duration::ZERO));
    let ticker = tokio::spawn({
        let worst = Arc::clone(&worst);
        async move {
            let mut last = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(2)).await;
                let now = Instant::now();
                let mut worst = worst.lock().unwrap();
                *worst = (*worst).max(now - last);
                last = now;
            }
        }
    });
    let mut runs = Vec::new();
    for _ in 0..3 {
        // The ticker notes a stall when it runs again: let it, before
        // starting and before reading.
        tokio::time::sleep(Duration::from_millis(20)).await;
        *worst.lock().unwrap() = Duration::ZERO;
        let started = Instant::now();
        let got = call(
            &backend2,
            &alice2,
            json!([["Email/get", {"accountId": a2, "ids": [found[0]], "fetchAllBodyValues": true,
                "properties": ["size", "bodyStructure", "textBody", "htmlBody", "attachments", "bodyValues", "headers"]}, "0"]]),
        )
        .await;
        let took = started.elapsed();
        tokio::time::sleep(Duration::from_millis(20)).await;
        runs.push((took, *worst.lock().unwrap()));
        let object = &got[0][1]["list"][0];
        assert_eq!(object["size"], raw.len(), "{got:?}");
        let attachments = object["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 2, "{object}");
        for attachment in attachments {
            assert!(
                attachment["blobId"].as_str().unwrap().starts_with("bs"),
                "{attachment}"
            );
            assert_eq!(attachment["size"], 17_500_000, "{attachment}");
        }
        let text_part = object["textBody"][0]["partId"].as_str().unwrap();
        assert_eq!(
            object["bodyValues"][text_part]["value"].as_str().unwrap(),
            expected_text
        );
    }
    ticker.abort();
    // One attachment, streamed, has the right bytes.
    let blob = {
        let got = call(
            &backend2,
            &alice2,
            json!([["Email/get", {"accountId": a2, "ids": [found[0]], "properties": ["attachments"]}, "0"]]),
        )
        .await;
        got[0][1]["list"][0]["attachments"][1]["blobId"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (streamed, _, bytes) = downloaded(&backend2, &alice2, &blob).await;
    assert!(streamed && bytes == second, "{} bytes", bytes.len());
    eprintln!(
        "large message: {} bytes, imported in {import_time:?}; Email/get with bodies \
         (time, longest stall of another task): {runs:?}",
        raw.len()
    );

    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email]}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend2.account_stopped(alice2.id).await;
    backend.account_stopped(alice.id).await;
}

/// Sending through a submission server that doesn't announce 8BITMIME
/// (Postfix's port `2469`, `smtpd_discard_ehlo_keywords = 8bitmime`), live:
/// a UTF-8 text and a forwarded 8-bit message reach bob intact, the
/// forwarded one turned into a `message/global` in base64 and the whole
/// message 7-bit on the way; a message with 8-bit text the bridge can't
/// turn into 7 bits is refused (`invalidEmail`) and nothing is sent.
/// Only with Postfix, which delivers to Dovecot (nothing to Stalwart).
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn seven_bit_downgrade_reaches_the_recipient_intact() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    if server.kind != ServerKind::Postfix {
        return;
    }
    let seven = TestServer {
        smtp: server
            .smtp_without_8bitmime()
            .expect("Postfix has a port without 8BITMIME"),
        smtp_starttls: None,
        ..server
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, seven);
    let alice = start(&backend, &store, seven, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let sent = mailbox(&backend, &alice, "sent").await;
    let identities = call(
        &backend,
        &alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let identity = identities[0][1]["list"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-7bit-test {stamp}");
    let text = "Hola, ¿qué tal? Ñandú, café, pingüino, 日本語, 😀.\r\nSegunda línea.";
    // A forwarded message in 8 bits (UTF-8 body, ASCII header).
    let forwarded = format!(
        "From: Carol <carol@{}>\r\nSubject: Reenviado {stamp}\r\n\
         Message-ID: <fwd.{stamp}@nebulamail.test>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
         Cañón, pingüino y 日本語 en 8 bits 😀\r\n",
        nebula_tests::DOMAIN
    )
    .into_bytes();
    assert!(!forwarded.is_ascii());
    let uploaded = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(forwarded.clone())], 50_000_000),
        )
        .await
        .unwrap();

    let sending = call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"draft": {
                "mailboxIds": {drafts.clone(): true},
                "keywords": {"$draft": true, "$seen": true},
                "from": [{"email": User::Alice.address(), "name": "Alice"}],
                "to": [{"email": User::Bob.address()}],
                "subject": subject,
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "bodyValues": {"t": {"value": text}},
                "attachments": [{"blobId": uploaded.blob_id.as_str(), "type": "message/rfc822",
                                 "name": "reenviado.eml", "disposition": "attachment"}]
            }}}, "0"],
            ["EmailSubmission/set", {"accountId": a,
              "create": {"send": {"emailId": "#draft", "identityId": identity}},
              "onSuccessUpdateEmail": {"#send": {
                  "keywords/$draft": null,
                  format!("mailboxIds/{drafts}"): null,
                  format!("mailboxIds/{sent}"): true
              }}}, "1"]
        ]),
    )
    .await;
    let email = sending[0][1]["created"]["draft"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {sending:?}"))
        .to_owned();
    assert!(
        sending[1][1]["created"]["send"]["id"].is_string(),
        "not sent: {sending:?}"
    );
    // Alice's own copy keeps the forwarded message as it was, in 8 bits.
    let own = call(
        &backend,
        &alice,
        json!([["Email/get", {"accountId": a, "ids": [email.clone()], "properties": ["blobId", "attachments"]}, "0"]]),
    )
    .await;
    assert_eq!(
        own[0][1]["list"][0]["attachments"][0]["type"], "message/rfc822",
        "{own:?}"
    );
    let (_, _, own_raw) = downloaded(
        &backend,
        &alice,
        own[0][1]["list"][0]["blobId"].as_str().unwrap(),
    )
    .await;
    assert!(!own_raw.is_ascii(), "the stored draft is in 8 bits");

    // Bob's copy, delivered by Postfix into Dovecot.
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, server);
    let bob = start(&bob_backend, &bob_store, server, User::Bob).await;
    let b = bob.jmap_id.as_str();
    let at_bob = find(&bob_backend, &bob, &subject, Duration::from_secs(60)).await;
    assert_eq!(at_bob.len(), 1, "bob didn't get the message");
    let got = call(
        &bob_backend,
        &bob,
        json!([["Email/get", {"accountId": b, "ids": at_bob, "fetchTextBodyValues": true,
            "properties": ["blobId", "subject", "textBody", "attachments", "bodyValues"]}, "0"]]),
    )
    .await;
    let received = &got[0][1]["list"][0];
    assert_eq!(received["subject"], subject.as_str(), "{received}");
    let text_part = received["textBody"][0]["partId"].as_str().unwrap();
    let value = received["bodyValues"][text_part]["value"].as_str().unwrap();
    assert_eq!(value.trim_end(), text, "the UTF-8 text arrives intact");
    let attachment = &received["attachments"][0];
    assert_eq!(attachment["type"], "message/global", "{received}");
    let (_, _, attached) =
        downloaded(&bob_backend, &bob, attachment["blobId"].as_str().unwrap()).await;
    assert!(
        attached == forwarded,
        "the forwarded message arrives byte for byte: {:?}",
        String::from_utf8_lossy(&attached)
    );
    let (_, _, raw) = downloaded(&bob_backend, &bob, received["blobId"].as_str().unwrap()).await;
    assert!(raw.is_ascii(), "sent in 7 bits");
    let raw_text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(
        raw_text.contains("content-type: message/global"),
        "{raw_text}"
    );

    // 8-bit text in a plain part: re-encoded in base64 and sent (RFC 6152
    // §3 allows converting instead of refusing), bob gets the same text.
    let refused_subject = format!("nebula-7bit-text {stamp}");
    let eight_bit = format!(
        "From: Alice <{}>\r\nTo: Bob <{}>\r\nSubject: {refused_subject}\r\n\
         Message-ID: <eight.{stamp}@nebulamail.test>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
         Texto en 8 bits: ñandú.\r\n",
        User::Alice.address(),
        User::Bob.address()
    )
    .into_bytes();
    let eight_bit_upload = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(eight_bit)], 50_000_000),
        )
        .await
        .unwrap();
    let imported = call(
        &backend,
        &alice,
        json!([
            ["Email/import", {"accountId": a, "emails": {"m": {
                "blobId": eight_bit_upload.blob_id.as_str(), "mailboxIds": {drafts.clone(): true},
                "keywords": {"$draft": true, "$seen": true}}}}, "0"],
            ["EmailSubmission/set", {"accountId": a,
              "create": {"send": {"emailId": "#m", "identityId": identity}}}, "1"]
        ]),
    )
    .await;
    let eight_bit_email = imported[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not imported: {imported:?}"))
        .to_owned();
    assert!(
        imported[1][1]["created"]["send"]["id"].is_string(),
        "{imported:?}"
    );
    let text_at_bob = find(
        &bob_backend,
        &bob,
        &refused_subject,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(text_at_bob.len(), 1, "bob didn't get the 8-bit text");
    let got = call(
        &bob_backend,
        &bob,
        json!([["Email/get", {"accountId": b, "ids": text_at_bob.clone(),
            "fetchTextBodyValues": true, "properties": ["textBody", "bodyValues"]}, "0"]]),
    )
    .await;
    let values = &got[0][1]["list"][0]["bodyValues"];
    let part = got[0][1]["list"][0]["textBody"][0]["partId"]
        .as_str()
        .unwrap();
    assert_eq!(
        values[part]["value"].as_str().map(str::trim_end),
        Some("Texto en 8 bits: ñandú."),
        "{got:?}"
    );

    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [email, eight_bit_email]}, "0"]]),
    )
    .await;
    assert_eq!(
        destroyed[0][1]["destroyed"].as_array().map(Vec::len),
        Some(2),
        "{destroyed:?}"
    );
    let at_bob: Vec<String> = at_bob.into_iter().chain(text_at_bob).collect();
    let destroyed = call(
        &bob_backend,
        &bob,
        json!([["Email/set", {"accountId": b, "destroy": at_bob}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    backend.account_stopped(alice.id).await;
    bob_backend.account_stopped(bob.id).await;
}

/// Only with Postfix, the test server whose submission announces DSN: the
/// client gives no ENVID, so the bridge sends its own; Postfix's real
/// reports (a bounce for a recipient it can't deliver to, a `delivered`
/// report for bob, who asked for one with `NOTIFY=SUCCESS`) quote it, and
/// `deliveryStatus` changes because the ENVID matches (a submission sent
/// with an ENVID ignores reports without it).
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn own_envid_matches_real_reports() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    if server.kind != ServerKind::Postfix {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-envid-test {stamp}");
    let nobody = format!("nobody-{stamp}@{}", nebula_tests::BOUNCE_DOMAIN);
    let bob = User::Bob.address();
    let identities = call(
        &backend,
        &alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let identity = identities[0][1]["list"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // No ENVID from the client; bob asks for a success report too.
    let sending = call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"draft": {
                "mailboxIds": {drafts.clone(): true},
                "keywords": {"$seen": true},
                "from": [{"email": "alice@nebulamail.test"}],
                "to": [{"email": bob.clone()}, {"email": nobody.clone()}],
                "subject": subject,
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "bodyValues": {"t": {"value": "Prueba del ENVID propio."}}
            }}}, "0"],
            ["EmailSubmission/set", {"accountId": a, "create": {"send": {
                "emailId": "#draft", "identityId": identity,
                "envelope": {
                    "mailFrom": {"email": "alice@nebulamail.test", "parameters": null},
                    "rcptTo": [
                        {"email": bob.clone(), "parameters": {"NOTIFY": "SUCCESS,FAILURE"}},
                        {"email": nobody.clone(), "parameters": null}
                    ]
                }
            }}}, "1"]
        ]),
    )
    .await;
    let email = sending[0][1]["created"]["draft"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("draft not created: {sending:?}"))
        .to_owned();
    let submission = sending[1][1]["created"]["send"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not sent: {sending:?}"))
        .to_owned();

    // Our ENVID was sent and kept, but the envelope stays the client's.
    let stored = store
        .submissions(alice.id, None, UnixTime::now().unwrap())
        .await
        .unwrap();
    let envid = stored
        .last()
        .and_then(|s| s.sent.envid.clone())
        .expect("an ENVID of our own");
    assert_eq!(envid.len(), 40, "{envid}");
    let got = call(
        &backend,
        &alice,
        json!([["EmailSubmission/get", {"accountId": a, "ids": [submission.clone()]}, "0"]]),
    )
    .await;
    assert_eq!(
        got[0][1]["list"][0]["envelope"]["mailFrom"]["parameters"],
        Value::Null,
        "{got:?}"
    );

    // The state before the reports, for `EmailSubmission/queryChanges`.
    let before = call(
        &backend,
        &alice,
        json!([["EmailSubmission/query", {"accountId": a,
                "filter": {"emailIds": [email.clone()]}, "sort": [{"property": "sentAt"}]}, "0"]]),
    )
    .await;
    assert_eq!(before[0][1]["ids"], json!([submission]), "{before:?}");
    let query_state = before[0][1]["queryState"].clone();

    // Postfix's reports arrive in alice's inbox and are matched.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let object = loop {
        let got = call(
            &backend,
            &alice,
            json!([["EmailSubmission/get", {"accountId": a, "ids": [submission.clone()],
                    "properties": ["deliveryStatus", "dsnBlobIds"]}, "0"]]),
        )
        .await;
        let object = got[0][1]["list"][0].clone();
        let status = &object["deliveryStatus"];
        // Postfix delivers to bob's mailbox itself, so it reports
        // `delivered` (→ `yes`); both are final.
        if status[&nobody]["delivered"] == "no" && status[&bob]["delivered"] == "yes" {
            break object;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no reports matched: {object:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    // Its `Diagnostic-Code` is no SMTP reply (`X-Postfix; delivered via
    // dovecot service`): bob keeps the submission server's.
    assert!(
        object["deliveryStatus"][&bob]["smtpReply"]
            .as_str()
            .is_some_and(|r| r.starts_with("250")),
        "{object:?}"
    );
    let blobs: Vec<String> = object["dsnBlobIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(blobs.len(), 2, "{object:?}");
    let mut reports = Vec::new();
    let mut bounced = false;
    for blob in &blobs {
        let (_, _, bytes) = downloaded(&backend, &alice, blob).await;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert!(
            text.contains(&format!("Original-Envelope-Id: {envid}")),
            "{text}"
        );
        bounced |= text.contains("Action: failed") && text.contains("Status: 5.1.1");
        // `bm{n}` is the whole email `e{n}`.
        reports.push(format!("e{}", blob.strip_prefix("bm").unwrap()));
    }
    assert!(bounced, "no bounce among the reports");

    // The reports updated the submission: `queryChanges` gives it again,
    // in the same place; a thread that isn't its own matches nothing.
    let changes = call(
        &backend,
        &alice,
        json!([
            ["EmailSubmission/queryChanges", {"accountId": a, "sinceQueryState": query_state,
              "filter": {"emailIds": [email.clone()]}, "sort": [{"property": "sentAt"}],
              "calculateTotal": true}, "0"],
            ["EmailSubmission/query", {"accountId": a, "filter": {"threadIds": ["t999999"]}}, "1"]
        ]),
    )
    .await;
    assert_eq!(changes[0][0], "EmailSubmission/queryChanges", "{changes:?}");
    assert_eq!(changes[0][1]["removed"], json!([submission]), "{changes:?}");
    assert_eq!(
        changes[0][1]["added"],
        json!([{"id": submission, "index": 0}]),
        "{changes:?}"
    );
    assert_eq!(changes[0][1]["total"], 1, "{changes:?}");
    assert_eq!(changes[1][1]["ids"], json!([]), "{changes:?}");

    // Clean up: the sent email, the reports and bob's copy.
    let mut destroy = reports;
    destroy.push(email);
    let destroyed = call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": destroy}, "0"]]),
    )
    .await;
    assert_eq!(
        destroyed[0][1]["destroyed"].as_array().map(Vec::len),
        Some(3),
        "{destroyed:?}"
    );
    let delivery = server.delivery();
    let bob_store = Store::open(&dir.path().join("bob.db")).await.unwrap();
    let bob_backend = new_backend(&bob_store, delivery);
    let bob_account = start(&bob_backend, &bob_store, delivery, User::Bob).await;
    let at_bob = find(
        &bob_backend,
        &bob_account,
        &subject,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(at_bob.len(), 1, "bob didn't get the message");
    let b = bob_account.jmap_id.as_str();
    let destroyed = call(
        &bob_backend,
        &bob_account,
        json!([["Email/set", {"accountId": b, "destroy": at_bob}, "0"]]),
    )
    .await;
    assert!(destroyed[0][1]["destroyed"].is_array(), "{destroyed:?}");
    bob_backend.account_stopped(bob_account.id).await;
    backend.account_stopped(alice.id).await;
}
