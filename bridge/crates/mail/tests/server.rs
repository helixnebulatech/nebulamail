// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Integration tests of the mail layer against the servers of
//! `docker/compose.yaml` (Stalwart or Dovecot, see `nebula-tests`).
//!
//! Skipped unless `NEBULA_TEST_IMAP=1`. Test folders get unique
//! names and are deleted at the end; the one message put in INBOX (to wake
//! IDLE up) is destroyed again.

// Test code, also the helpers clippy doesn't see as tests: a panic here is
// a failed test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_imap::proto::command::{ListPattern, StoreOperation, StoreResponse};
use nebula_imap::proto::types::{Flag, MailboxName, UidSet};
use nebula_imap::{
    Authenticated, Client, Connected, Credentials, Limits, SelectOptions, Tls, TlsConnector,
    TlsVerify,
};
use nebula_jmap_proto::Keyword;
use nebula_mail::{
    AppendItem, CertificateCheck, ImapServer, KeywordChange, Mail, MailAccount, MailConfig,
    MailError, MailEvent, OnDestroy, Start, TextCondition, TextField, TlsMode,
};
use nebula_store::{
    AccountId, DataType, EmailFilter, EmailId, EmailQuery, MailboxId, NewAccount, QueryStart, Role,
    Store, UnixTime,
};
use nebula_tests::{ServerKind, TestServer, User};
use net_guard::{Port, PortPolicy, SystemResolver};
use secrecy::SecretString;
use tokio::net::TcpStream;
use tokio::sync::broadcast;

const PREFIX: &str = "nebula-mail-test";

/// The server of the running test; only called once a test knows the IMAP
/// tests are enabled.
fn current() -> TestServer {
    TestServer::imap().expect("IMAP tests enabled")
}

fn password() -> SecretString {
    current().password(User::Alice)
}

fn server() -> ImapServer {
    let port = Port::try_from(current().imap.port).unwrap();
    ImapServer::new(nebula_tests::HOST.parse().unwrap(), port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![port]))
}

/// A plain IMAP connection, playing "another client".
async fn other_client() -> Client<Authenticated> {
    let stream = TcpStream::connect(current().imap.address())
        .await
        .expect("IMAP test server");
    let connector = TlsConnector::new(TlsVerify::InsecureLoopbackOnly).unwrap();
    let connected = Client::connect(
        stream,
        "localhost".try_into().unwrap(),
        Tls::Implicit,
        &connector,
        Limits::default(),
    )
    .await
    .unwrap();
    let Connected::NotAuthenticated(client) = connected else {
        panic!("unexpected PREAUTH")
    };
    client
        .login(&Credentials::new("alice", password()))
        .await
        .unwrap()
}

/// The keywords a client set, without `$hasattachment` and
/// `$hasnoattachment`: Stalwart 1.0 adds those to every message by itself
/// (registered IMAP keywords, from what the message holds).
fn client_keywords(keywords: &BTreeSet<Keyword>) -> BTreeSet<Keyword> {
    keywords
        .iter()
        .filter(|keyword| !matches!(keyword.as_str(), "$hasattachment" | "$hasnoattachment"))
        .cloned()
        .collect()
}

fn unique(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{PREFIX}-{}-{nanos}-{tag}", std::process::id())
}

fn message(subject: &str, id: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@nebulamail.test>\r\nTo: Alice <alice@nebulamail.test>\r\nSubject: {subject}\r\n\
         Message-ID: <{id}@nebula.test>\r\nDate: Thu, 24 Oct 2026 10:00:00 +0000\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nHola, esto es una prueba: {subject}.\r\n"
    )
    .into_bytes()
}

struct Setup {
    store: Store,
    account: AccountId,
    mail: Mail,
    _dir: tempfile::TempDir,
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).await.unwrap();
    let account = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "alice@nebulamail.test",
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let resolver = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store.clone(), MailConfig::new(server()), resolver).unwrap();
    Setup {
        store,
        account,
        mail,
        _dir: dir,
    }
}

#[tokio::test]
async fn wrong_password_is_auth_failed() {
    if TestServer::imap().is_none() {
        return;
    }
    let setup = setup().await;
    let error = setup
        .mail
        .open(
            setup.account,
            "alice",
            SecretString::from("definitely-wrong"),
            Start::Login,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, MailError::AuthFailed), "{error:?}");
    // Not reported for later: the caller of `open` deals with it.
    let reported =
        tokio::time::timeout(Duration::from_millis(200), setup.mail.auth_failures()).await;
    assert!(reported.is_err(), "{reported:?}");
    let error = setup
        .mail
        .verify_login("alice", SecretString::from("nope"))
        .await
        .unwrap_err();
    assert!(matches!(error, MailError::AuthFailed), "{error:?}");
    setup.mail.verify_login("alice", password()).await.unwrap();
}

/// Deletes test folders left by earlier runs that crashed.
async fn remove_stale_folders(client: &mut Client<Authenticated>) {
    let ours = format!("-{}-", std::process::id());
    let pattern = ListPattern::new(&format!("{PREFIX}-*")).unwrap();
    for listed in client.list(&pattern, &[]).await.unwrap() {
        if !listed.entry.name.as_str().contains(&ours) {
            let _ = client.delete(&listed.entry.name).await;
        }
    }
}

#[tokio::test]
async fn sync_and_operations() {
    if TestServer::imap().is_none() {
        return;
    }
    let mut other = other_client().await;
    remove_stale_folders(&mut other).await;
    let folder_a = unique("a");
    let folder_b = unique("b");
    other
        .create(&MailboxName::new(&folder_a).unwrap(), None)
        .await
        .unwrap();

    let setup = setup().await;
    let (a, b) = (folder_a.clone(), folder_b.clone());
    let store = setup.store.clone();
    let account_id = setup.account;
    let mail = Arc::new(setup.mail);
    let task_mail = Arc::clone(&mail);
    let outcome = tokio::spawn(async move {
        scenario(&task_mail, &store, account_id, other, &a, &b).await;
    })
    .await;

    // Clean up whatever happened.
    mail.close(account_id).await;
    let mut cleanup = other_client().await;
    for name in [&folder_a, &folder_b] {
        let _ = cleanup.delete(&MailboxName::new(name).unwrap()).await;
    }
    cleanup.logout().await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

async fn mailbox_id(store: &Store, account: AccountId, name: &str) -> MailboxId {
    store
        .mailbox_by_name(account, &MailboxName::new(name).unwrap())
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("mailbox {name} not stored"))
        .id
}

async fn emails_in(store: &Store, account: AccountId, mailbox: MailboxId) -> Vec<EmailId> {
    store
        .query_emails(
            account,
            EmailQuery {
                filter: Some(EmailFilter::InMailbox(mailbox)),
                sort: Vec::new(),
                collapse_threads: false,
                start: QueryStart::Position(0),
                limit: None,
            },
        )
        .await
        .unwrap()
        .ids
}

/// Starts alice's account as after a login.
async fn open(mail: &Mail, account: AccountId) -> Arc<MailAccount> {
    mail.open(account, "alice", password(), Start::Login)
        .await
        .unwrap()
}

async fn scenario(
    mail: &Mail,
    store: &Store,
    account_id: AccountId,
    mut other: Client<Authenticated>,
    folder_a: &str,
    folder_b: &str,
) {
    let mut events = mail.subscribe();
    let account = open(mail, account_id).await;
    // The live task syncs too; this one waits for the lock and finishes.
    account.sync_all().await.unwrap();
    let inbox = store
        .mailbox_by_role(account_id, Role::Inbox)
        .await
        .unwrap()
        .expect("INBOX stored");
    assert!(
        store
            .mailbox_by_role(account_id, Role::Sent)
            .await
            .unwrap()
            .is_some(),
        "Sent from SPECIAL-USE"
    );
    let a = mailbox_id(store, account_id, folder_a).await;
    assert!(emails_in(store, account_id, a).await.is_empty());

    // Another client appends: the incremental sync sees it.
    let a_name = MailboxName::new(folder_a).unwrap();
    let first_id = unique("m1");
    other
        .append(&a_name, &message("primero", &first_id), &[], None)
        .await
        .unwrap();
    account.sync_mailbox(a).await.unwrap();
    let ids = emails_in(store, account_id, a).await;
    assert_eq!(ids.len(), 1);
    let email = ids[0];
    let stored = store
        .emails(account_id, vec![email])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(stored.meta.subject.as_deref(), Some("primero"));
    if current().kind == ServerKind::Stalwart {
        assert!(stored.server_id.is_some(), "Stalwart has OBJECTID+");
    }
    assert!(client_keywords(&stored.keywords).is_empty(), "{stored:?}");
    let mailbox = store.mailbox(account_id, a).await.unwrap();
    assert!(
        mailbox.sync.highest_mod_seq.is_some(),
        "CONDSTORE state saved"
    );

    // A flag set by the other client: the QRESYNC sync picks it up.
    let (mut selected, _) = other
        .select(&a_name, SelectOptions::default())
        .await
        .unwrap();
    selected
        .uid_store(
            &UidSet::all(),
            StoreOperation::Add,
            &[Flag::Flagged],
            StoreResponse::Silent,
            None,
        )
        .await
        .unwrap();
    let other = selected.unselect().await.unwrap();
    account.sync_mailbox(a).await.unwrap();
    let stored = store
        .emails(account_id, vec![email])
        .await
        .unwrap()
        .remove(0);
    assert!(stored.keywords.contains(&Keyword::flagged()), "{stored:?}");

    // Our own keyword change goes to the server.
    let before = store.state(account_id, DataType::Email).await.unwrap();
    account
        .update_keywords(
            email,
            KeywordChange::Patch {
                add: BTreeSet::from([Keyword::seen()]),
                remove: BTreeSet::from([Keyword::flagged()]),
            },
        )
        .await
        .unwrap();
    let stored = store
        .emails(account_id, vec![email])
        .await
        .unwrap()
        .remove(0);
    let keywords = client_keywords(&stored.keywords);
    assert_eq!(keywords, BTreeSet::from([Keyword::seen()]));
    assert!(store.state(account_id, DataType::Email).await.unwrap() > before);
    // A sync afterwards changes nothing (the server agrees).
    let resync = account.sync_mailbox(a).await.unwrap();
    assert!(resync.state(DataType::Email).is_none(), "{resync:?}");

    second_half(
        &account,
        store,
        account_id,
        other,
        &mut events,
        (inbox.id, a),
        email,
        folder_b,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "the state of the scenario so far"
)]
async fn second_half(
    account: &MailAccount,
    store: &Store,
    account_id: AccountId,
    other: Client<Authenticated>,
    events: &mut broadcast::Receiver<MailEvent>,
    (inbox, a): (MailboxId, MailboxId),
    email: EmailId,
    folder_b: &str,
) {
    // Move to a folder we create: same email id.
    let b = account.create_mailbox(folder_b, None).await.unwrap();
    account
        .set_mailboxes(email, BTreeSet::from([b]))
        .await
        .unwrap();
    let stored = store
        .emails(account_id, vec![email])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(stored.mailboxes, BTreeSet::from([b]));
    assert!(emails_in(store, account_id, a).await.is_empty());
    account.sync_mailbox(a).await.unwrap();
    account.sync_mailbox(b).await.unwrap();
    assert_eq!(emails_in(store, account_id, b).await, vec![email]);

    // Raw message and preview.
    let mut lease = account.lease();
    let raw = account
        .load_message(email, &mut lease, nebula_mail::Copies::One)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("Subject: primero"));
    let stored = store
        .emails(account_id, vec![email])
        .await
        .unwrap()
        .remove(0);
    assert!(
        stored
            .meta
            .preview
            .as_deref()
            .is_some_and(|p| p.contains("prueba")),
        "{:?}",
        stored.meta.preview
    );

    // Append through the bridge.
    let second = account
        .append(
            b,
            &message("segundo", &unique("m2")),
            &BTreeSet::from([Keyword::seen()]),
            None,
        )
        .await
        .unwrap()
        .email;
    let stored = store
        .emails(account_id, vec![second])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(stored.meta.subject.as_deref(), Some("segundo"));
    assert!(stored.keywords.contains(&Keyword::seen()));

    // Text search on the server (both index in the background).
    let condition = TextCondition {
        field: TextField::Subject,
        value: "segundo".to_owned(),
    };
    let mut found = BTreeSet::new();
    for _ in 0..40 {
        found = account
            .search(Some(b), std::slice::from_ref(&condition))
            .await
            .unwrap();
        if !found.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(found, BTreeSet::from([second]));

    // IDLE wakes up on a new INBOX message.
    idle_sees_new_mail(account, store, inbox, other, events).await;

    // Destroy and delete the folders through the bridge.
    account.destroy(second).await.unwrap();
    assert!(
        store
            .emails(account_id, vec![second])
            .await
            .unwrap()
            .is_empty()
    );
    account
        .delete_mailbox(b, OnDestroy::RemoveEmails)
        .await
        .unwrap();
    assert!(
        store
            .emails(account_id, vec![email])
            .await
            .unwrap()
            .is_empty()
    );
    account
        .delete_mailbox(a, OnDestroy::RemoveEmails)
        .await
        .unwrap();
    assert!(store.mailbox(account_id, a).await.is_err());
}

async fn idle_sees_new_mail(
    account: &MailAccount,
    store: &Store,
    inbox: MailboxId,
    mut other: Client<Authenticated>,
    events: &mut broadcast::Receiver<MailEvent>,
) {
    // Give the live task time to start IDLE after its first sync.
    tokio::time::sleep(Duration::from_secs(2)).await;
    while events.try_recv().is_ok() {}
    let id = unique("idle");
    other
        .append(&MailboxName::inbox(), &message("despierta", &id), &[], None)
        .await
        .unwrap();
    other.logout().await.unwrap();
    let message_id = format!("{id}@nebula.test");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let found = loop {
        let found = store
            .emails_by_message_id(account.id(), inbox, message_id.clone())
            .await
            .unwrap();
        if let Some(email) = found.first() {
            break *email;
        }
        // Nobody but the live task syncs now: an event means IDLE worked.
        let wait = tokio::time::timeout_at(deadline, events.recv()).await;
        assert!(wait.is_ok(), "IDLE did not bring the new message in time");
    };
    // Don't leave test mail in the inbox.
    account.destroy(found).await.unwrap();
}

/// Subscriptions, search of mail newer than the last sync, NOTIFY (when
/// the server has it), MULTIAPPEND, and a `Mail` dropped without `close`.
#[tokio::test]
async fn extensions_and_live() {
    if TestServer::imap().is_none() {
        return;
    }
    let folder = unique("ext");
    let mut other = other_client().await;
    let name = MailboxName::new(&folder).unwrap();
    other.create(&name, None).await.unwrap();
    other.subscribe(&name).await.unwrap();

    let setup = setup().await;
    let store = setup.store.clone();
    let account_id = setup.account;
    let mail = Arc::new(setup.mail);
    let task_mail = Arc::clone(&mail);
    let task_folder = folder.clone();
    let outcome = tokio::spawn(async move {
        extensions_scenario(&task_mail, &store, account_id, other, &task_folder).await;
    })
    .await;

    mail.close(account_id).await;
    let mut cleanup = other_client().await;
    let _ = cleanup.delete(&name).await;
    cleanup.logout().await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
    dropped_mail_stops().await;
}

async fn extensions_scenario(
    mail: &Mail,
    store: &Store,
    account_id: AccountId,
    mut other: Client<Authenticated>,
    folder: &str,
) {
    let mut events = mail.subscribe();
    let account = mail
        .open(account_id, "alice", password(), Start::Login)
        .await
        .unwrap();
    account.sync_all().await.unwrap();
    let id = mailbox_id(store, account_id, folder).await;
    let name = MailboxName::new(folder).unwrap();
    assert!(store.mailbox(account_id, id).await.unwrap().subscribed);
    other.unsubscribe(&name).await.unwrap();
    account.check_mailboxes().await.unwrap();
    assert!(!store.mailbox(account_id, id).await.unwrap().subscribed);

    // A message the bridge hasn't synced yet is still found by search.
    let fresh = unique("fresh");
    other
        .append(&name, &message("recienllegado", &fresh), &[], None)
        .await
        .unwrap();
    let condition = TextCondition {
        field: TextField::Subject,
        value: "recienllegado".to_owned(),
    };
    let mut found = BTreeSet::new();
    for _ in 0..40 {
        found = account
            .search(Some(id), std::slice::from_ref(&condition))
            .await
            .unwrap();
        if !found.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(found.len(), 1, "the new message is synced and found");

    // NOTIFY: a message in another folder arrives without asking.
    if matches!(
        current().kind,
        ServerKind::Dovecot | ServerKind::Postfix | ServerKind::DovecotPrefix
    ) {
        tokio::time::sleep(Duration::from_secs(2)).await;
        while events.try_recv().is_ok() {}
        let notified = unique("notified");
        other
            .append(&name, &message("avisado", &notified), &[], None)
            .await
            .unwrap();
        let message_id = format!("{notified}@nebula.test");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let found = store
                .emails_by_message_id(account_id, id, message_id.clone())
                .await
                .unwrap();
            if !found.is_empty() {
                break;
            }
            let wait = tokio::time::timeout_at(deadline, events.recv()).await;
            assert!(wait.is_ok(), "NOTIFY did not bring the message in time");
        }
    }
    other.logout().await.unwrap();

    // MULTIAPPEND (or one by one): one email per item.
    let (one, two) = (
        message("lote uno", &unique("b1")),
        message("lote dos", &unique("b2")),
    );
    let keywords = BTreeSet::from([Keyword::seen()]);
    let none = BTreeSet::new();
    let results = account
        .append_many(
            id,
            &[
                AppendItem {
                    raw: &one,
                    keywords: &keywords,
                    received_at: None,
                },
                AppendItem {
                    raw: &two,
                    keywords: &none,
                    received_at: None,
                },
            ],
        )
        .await
        .unwrap();
    let ids: Vec<EmailId> = results
        .into_iter()
        .map(|result| result.unwrap().email)
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    let stored = store.emails(account_id, ids.clone()).await.unwrap();
    assert_eq!(stored[0].meta.subject.as_deref(), Some("lote uno"));
    assert!(stored[0].keywords.contains(&Keyword::seen()));
    assert_eq!(stored[1].meta.subject.as_deref(), Some("lote dos"));
}

/// Dropping `Mail` without `close` stops the live task, which lets go of
/// the account.
async fn dropped_mail_stops() {
    let setup = setup().await;
    let account = setup
        .mail
        .open(setup.account, "alice", password(), Start::Login)
        .await
        .unwrap();
    let weak = Arc::downgrade(&account);
    drop(account);
    drop(setup.mail);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while weak.upgrade().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the live task still holds the account"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn sent(message_id: &str) -> nebula_store::NewSubmission {
    let now = UnixTime::now().unwrap();
    nebula_store::NewSubmission {
        identity: nebula_store::IdentityId::from_raw(1).unwrap(),
        email: EmailId::from_raw(1).unwrap(),
        thread: nebula_store::ThreadId::from_raw(1).unwrap(),
        envelope: serde_json::from_value(serde_json::json!({
            "mailFrom": {"email": "alice@nebulamail.test"},
            "rcptTo": [{"email": "bob@nebulamail.test"}, {"email": "carol@nebulamail.test"}]
        }))
        .unwrap(),
        send_at: now,
        smtp_reply: "250 2.0.0 queued".into(),
        expires_at: now.saturating_add(Duration::from_secs(3600)),
        message_id: Some(message_id.to_owned()),
        envid: None,
    }
}

fn dsn(original: &str) -> Vec<u8> {
    format!(
        "From: Mail Delivery System <MAILER-DAEMON@nebulamail.test>\r\nTo: alice@nebulamail.test\r\n\
Subject: Undelivered Mail Returned to Sender\r\nMessage-ID: <dsn-{original}>\r\n\
Date: Tue, 27 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/report; report-type=delivery-status; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/plain\r\n\r\nYour message could not be delivered.\r\n\
--R\r\nContent-Type: message/delivery-status\r\n\r\n\
Reporting-MTA: dns; mx.nebulamail.test\r\n\r\n\
Final-Recipient: rfc822; bob@nebulamail.test\r\nAction: failed\r\nStatus: 5.1.1\r\n\
Diagnostic-Code: smtp; 550 5.1.1 mailbox unavailable\r\n\r\n\
--R\r\nContent-Type: text/rfc822-headers\r\n\r\n\
From: alice@nebulamail.test\r\nTo: bob@nebulamail.test\r\nMessage-ID: <{original}>\r\n\
Subject: hola\r\n\r\n--R--\r\n"
    )
    .into_bytes()
}

fn mdn(original: &str) -> Vec<u8> {
    format!(
        "From: carol@nebulamail.test\r\nTo: alice@nebulamail.test\r\nSubject: Read: hola\r\n\
Message-ID: <mdn-{original}>\r\nDate: Tue, 27 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/report; report-type=disposition-notification; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/plain\r\n\r\nDisplayed.\r\n\
--R\r\nContent-Type: message/disposition-notification\r\n\r\n\
Reporting-UA: x; y\r\nFinal-Recipient: rfc822; carol@nebulamail.test\r\n\
Original-Message-ID: <{original}>\r\n\
Disposition: manual-action/MDN-sent-manually; displayed\r\n\r\n--R--\r\n"
    )
    .into_bytes()
}

/// A DSN and an MDN that arrive (here: appended by another client) fill
/// in the `deliveryStatus` of the submission they are about, with a state
/// change for push; another account's submission with the same
/// `Message-ID` is not touched. No message is sent.
#[tokio::test]
async fn delivery_reports_update_submissions() {
    if TestServer::imap().is_none() {
        return;
    }
    let mut other = other_client().await;
    remove_stale_folders(&mut other).await;
    let folder = unique("dsn");
    let name = MailboxName::new(&folder).unwrap();
    other.create(&name, None).await.unwrap();
    let setup = setup().await;
    let store = setup.store.clone();
    let account_id = setup.account;
    let mail = Arc::new(setup.mail);
    let task_mail = Arc::clone(&mail);
    let task_folder = folder.clone();
    let outcome = tokio::spawn(async move {
        reports_scenario(&task_mail, &store, account_id, &mut other, &task_folder).await;
        other.logout().await.unwrap();
    })
    .await;
    mail.close(account_id).await;
    let mut cleanup = other_client().await;
    let _ = cleanup.delete(&name).await;
    cleanup.logout().await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

async fn reports_scenario(
    mail: &Mail,
    store: &Store,
    account_id: AccountId,
    other: &mut Client<Authenticated>,
    folder: &str,
) {
    let original = format!("{}@nebula.test", unique("sent"));
    let (submission, _) = store
        .add_submission(account_id, sent(&original))
        .await
        .unwrap();
    let mallory = store
        .get_or_create_account(
            NewAccount {
                login: "mallory",
                email: "mallory@nebulamail.test",
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let (theirs, _) = store
        .add_submission(mallory, sent(&original))
        .await
        .unwrap();

    let account = mail
        .open(account_id, "alice", password(), Start::Login)
        .await
        .unwrap();
    account.sync_all().await.unwrap();
    let mut events = mail.subscribe();
    let name = MailboxName::new(folder).unwrap();
    other
        .append(&name, &dsn(&original), &[], None)
        .await
        .unwrap();
    other
        .append(&name, &mdn(&original), &[], None)
        .await
        .unwrap();
    let mailbox = mailbox_id(store, account_id, folder).await;
    account.sync_mailbox(mailbox).await.unwrap();

    let now = UnixTime::now().unwrap();
    let list = store
        .submissions(account_id, Some(vec![submission]), now)
        .await
        .unwrap();
    let status = &list[0].status;
    assert_eq!(
        status["bob@nebulamail.test"].delivered,
        nebula_store::Delivered::No,
        "{status:?}"
    );
    assert_eq!(
        status["bob@nebulamail.test"].smtp_reply,
        "550 5.1.1 mailbox unavailable"
    );
    assert!(!status["bob@nebulamail.test"].displayed);
    assert_eq!(
        status["carol@nebulamail.test"].delivered,
        nebula_store::Delivered::Unknown
    );
    assert!(status["carol@nebulamail.test"].displayed);
    assert_eq!(list[0].dsn_emails.len(), 1);
    assert_eq!(list[0].mdn_emails.len(), 1);
    let pushed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(MailEvent::StateChanged(changed))
                    if changed.data_types.contains(&DataType::EmailSubmission) =>
                {
                    return;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await;
    assert!(pushed.is_ok(), "no EmailSubmission state change");
    let untouched = store
        .submissions(mallory, Some(vec![theirs]), now)
        .await
        .unwrap();
    assert_eq!(
        untouched[0].status["bob@nebulamail.test"].delivered,
        nebula_store::Delivered::Unknown
    );
    assert!(untouched[0].dsn_emails.is_empty());
}

/// The flags of every message in `name`, as another client sees them.
async fn server_flags(
    client: Client<Authenticated>,
    name: &MailboxName,
) -> (Vec<Vec<Flag>>, Client<Authenticated>) {
    let (mut selected, _) = client.select(name, SelectOptions::default()).await.unwrap();
    let fetched = selected
        .uid_fetch(
            &UidSet::all(),
            &[nebula_imap::proto::command::FetchAttribute::Flags],
            nebula_imap::proto::command::FetchModifiers::None,
        )
        .await
        .unwrap();
    let flags = fetched
        .messages
        .into_iter()
        .map(|m| m.flags.unwrap_or_default())
        .collect();
    (flags, selected.unselect().await.unwrap())
}

/// Checks that `name` can keep neither `\Flagged` nor new keywords.
async fn assert_keywords_refused(
    client: Client<Authenticated>,
    name: &MailboxName,
) -> Client<Authenticated> {
    let (selected, _) = client.select(name, SelectOptions::default()).await.unwrap();
    let permanent = selected.mailbox().permanent_flags.clone();
    let flagged_storable = permanent.can_store(&Flag::Flagged);
    let keywords_allowed = matches!(
        &permanent,
        nebula_imap::PermanentFlags::Listed {
            new_keywords: nebula_imap::proto::response::NewKeywords::Allowed,
            ..
        } | nebula_imap::PermanentFlags::NotAnnounced
    );
    assert!(
        !flagged_storable && !keywords_allowed,
        "{name:?} has PERMANENTFLAGS {permanent:?}: is docker/dovecot/acl.conf \
         mounted? (docker compose -f docker/compose.yaml --profile bridge up -d \
         --force-recreate dovecot)"
    );
    selected.unselect().await.unwrap()
}

/// A keyword change on an email with copies in two mailboxes, one of which
/// can't keep it (PERMANENTFLAGS without `\*` nor `\Flagged`): it fails
/// before any copy is written, so the other mailbox doesn't keep half of
/// it. Only Dovecot: `docker/dovecot/acl.conf` takes the
/// `w` right away in the folders ending in `-noflags`; Stalwart and a
/// plain Dovecot announce `\*` everywhere.
///
/// Stalwart can't have this variant (tried by hand on 1.0.1, 2026-11-04):
/// the owner of a folder always keeps `lrswipkxtena` (a `SETACL` for the
/// owner answers OK and changes nothing), and a share with `s` always gets
/// `w` too (`lrsi` is stored as `lrswi`, with `\*` in PERMANENTFLAGS). A
/// folder of another user shared with `lri` does give `PERMANENTFLAGS ()`,
/// but the bridge leaves other users' namespaces out of its mailboxes
/// (`sync::list`), so it can't hold a copy there.
#[tokio::test]
async fn keyword_change_checks_every_copy_first() {
    if TestServer::imap().is_none() {
        return;
    }
    if current().kind != ServerKind::Dovecot {
        eprintln!(
            "{}: no mailbox of the user's own without `\\*` here (see the docs), skipped",
            current().kind
        );
        return;
    }
    let setup = setup().await;
    let account = open(&setup.mail, setup.account).await;
    // `rw` first: it gets the smaller id, so it is the first copy and the
    // one a change without the check would write before failing.
    let rw = account.create_mailbox(&unique("rw"), None).await.unwrap();
    let noflags = account
        .create_mailbox(&unique("noflags"), None)
        .await
        .unwrap();
    let store = setup.store.clone();
    let account_id = setup.account;
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        async move {
            let rw_name = store.mailbox(account_id, rw).await.unwrap().imap_name;
            let noflags_name = store.mailbox(account_id, noflags).await.unwrap().imap_name;
            // The ACL is there: otherwise this test would pass for nothing.
            let other = assert_keywords_refused(other_client().await, &noflags_name).await;

            let email = account
                .append(
                    rw,
                    &message("dos copias", &unique("m")),
                    &BTreeSet::new(),
                    None,
                )
                .await
                .unwrap()
                .email;
            account
                .set_mailboxes(email, BTreeSet::from([rw, noflags]))
                .await
                .unwrap();
            let locations = store.locations(account_id, email).await.unwrap();
            assert_eq!(locations.len(), 2, "{locations:?}");
            assert_eq!(locations[0].mailbox, rw, "the writable copy goes first");

            let add = |keyword: Keyword| KeywordChange::Patch {
                add: BTreeSet::from([keyword]),
                remove: BTreeSet::new(),
            };
            let before = store.emails(account_id, vec![email]).await.unwrap()[0]
                .keywords
                .clone();
            let refused = account
                .update_keywords(email, add(Keyword::flagged()))
                .await;
            assert!(
                matches!(refused, Err(MailError::Unsupported(_))),
                "{refused:?}"
            );
            let (flags, other) = server_flags(other, &rw_name).await;
            assert_eq!(flags.len(), 1);
            assert!(!flags[0].contains(&Flag::Flagged), "{flags:?}");
            let after = store.emails(account_id, vec![email]).await.unwrap()[0]
                .keywords
                .clone();
            assert_eq!(before, after);

            // Both can keep \Seen (the `s` right).
            account
                .update_keywords(email, add(Keyword::seen()))
                .await
                .unwrap();
            let (flags, other) = server_flags(other, &rw_name).await;
            assert!(flags[0].contains(&Flag::Seen), "{flags:?}");
            let (flags, other) = server_flags(other, &noflags_name).await;
            assert_eq!(flags.len(), 1);
            assert!(flags[0].contains(&Flag::Seen), "{flags:?}");

            // One copy only: the check in the loop refuses it too.
            account
                .set_mailboxes(email, BTreeSet::from([noflags]))
                .await
                .unwrap();
            assert_eq!(emails_in(&store, account_id, noflags).await, vec![email]);
            let refused = account
                .update_keywords(email, add(Keyword::flagged()))
                .await;
            assert!(
                matches!(refused, Err(MailError::Unsupported(_))),
                "{refused:?}"
            );
            let _ = other.logout().await;
        }
    })
    .await;
    setup.mail.close(account_id).await;
    let mut cleanup = other_client().await;
    for id in [rw, noflags] {
        if let Ok(mailbox) = setup.store.mailbox(account_id, id).await {
            let _ = cleanup.delete(&mailbox.imap_name).await;
        }
    }
    let _ = cleanup.logout().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}
