// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Other work interleaved with our operations.
//!
//! - A sync that waits for the command connection while a MOVE or COPY into
//!   a mailbox never synced runs on it.
//! - A search while another client has deleted a mailbox we still list.
//!
//! The order is forced: the test holds the connection, the operation and
//! then the sync queue up behind it (tokio's mutex is fair, so they get it
//! in that order), and only then the test lets go. Before the fix the sync
//! had read the mailbox state (no UIDVALIDITY yet) before queueing, reset
//! the mailbox after the MOVE had stored the moved copy, and imported it
//! again under a new email id.
//!
//! Also, with a real server: a huge BODYSTRUCTURE, a PREVIEW nested too
//! deep, a mailbox deleted between LIST and its sync, mail another client
//! added and we never synced, two mailbox changes at once, and a login
//! with a changed password while the account runs, and a real message of
//! 47–48 MB loaded within the memory budget.
//!
//! Against the servers of `docker/compose.yaml`, like `tests/server.rs`:
//! skipped unless `NEBULA_TEST_IMAP=1`. They are unit tests (they use the
//! crate's insides), so `--test server` doesn't run them:
//!
//! ```sh
//! NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=stalwart \
//!   cargo test -p nebula-mail --lib live_tests -- --test-threads=1
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a failed test"
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_store::{
    AccountId, EmailFilter, EmailId, EmailQuery, MailboxId, NewAccount, QueryStart, Store, UnixTime,
};
use nebula_tests::{TestServer, User};
use net_guard::{Port, PortPolicy, SystemResolver};

use nebula_imap::proto::types::{Capability, MailboxName};

use crate::core::account::MailAccount;
use crate::sync::list;
use crate::{
    CertificateCheck, ImapServer, Mail, MailConfig, MailError, Start, TextCondition, TextField,
    TlsMode,
};

/// Long enough for a few reads of a local SQLite file.
const SETTLE: Duration = Duration::from_millis(300);

fn server(test: TestServer) -> ImapServer {
    let port = Port::try_from(test.imap.port).unwrap();
    ImapServer::new(nebula_tests::HOST.parse().unwrap(), port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![port]))
}

fn unique(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nebula-mail-test-{}-{nanos}-{tag}", std::process::id())
}

async fn emails_in(store: &Store, account: AccountId, mailbox: MailboxId) -> Vec<EmailId> {
    let query = EmailQuery {
        filter: Some(EmailFilter::InMailbox(mailbox)),
        sort: Vec::new(),
        collapse_threads: false,
        start: QueryStart::Position(0),
        limit: None,
    };
    store.query_emails(account, query).await.unwrap().ids
}

/// Runs `operation`, then a sync of `mailbox` queued right behind it on the
/// command connection.
async fn race<F>(account: &Arc<MailAccount>, mailbox: MailboxId, operation: F)
where
    F: Future<Output = Result<nebula_store::Recorded, MailError>> + Send + 'static,
{
    let held = account.slot().await;
    let operation = tokio::spawn(operation);
    tokio::time::sleep(SETTLE).await;
    let syncing = Arc::clone(account);
    let sync = tokio::spawn(async move { syncing.sync_mailbox(mailbox).await });
    tokio::time::sleep(SETTLE).await;
    drop(held);
    operation.await.unwrap().unwrap();
    sync.await.unwrap().unwrap();
}

/// An account without the live task (`Mail::open` starts one): only the
/// syncs of the test run. The `TempDir` keeps the store.
async fn account(test: TestServer) -> (Arc<MailAccount>, Store, AccountId, tempfile::TempDir) {
    account_with(test, nebula_imap::Limits::default()).await
}

async fn account_with(
    test: TestServer,
    limits: nebula_imap::Limits,
) -> (Arc<MailAccount>, Store, AccountId, tempfile::TempDir) {
    let (mail, store, account_id, dir) = service_with(test, limits).await;
    let account = Arc::new(MailAccount::new(
        account_id,
        Arc::clone(&mail.shared),
        nebula_imap::Credentials::new("alice", test.password(User::Alice)),
    ));
    (account, store, account_id, dir)
}

/// The service with alice's account stored, not started.
async fn service_with(
    test: TestServer,
    limits: nebula_imap::Limits,
) -> (Mail, Store, AccountId, tempfile::TempDir) {
    service_with_config(test, limits, |_| {}).await
}

/// [`service_with`], with the config changed by `configure`.
async fn service_with_config(
    test: TestServer,
    limits: nebula_imap::Limits,
    configure: impl FnOnce(&mut MailConfig),
) -> (Mail, Store, AccountId, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).await.unwrap();
    let account_id = store
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
    let mut config = MailConfig::new(server(test).with_limits(limits));
    configure(&mut config);
    let mail = Mail::new(store.clone(), config, resolver).unwrap();
    (mail, store, account_id, dir)
}

async fn imap_name(store: &Store, account: AccountId, mailbox: MailboxId) -> MailboxName {
    store.mailbox(account, mailbox).await.unwrap().imap_name
}

/// Another client, logged in as alice.
async fn other_client(account: &MailAccount) -> nebula_imap::Client<nebula_imap::Authenticated> {
    account
        .connector()
        .login(&account.credentials(), account.alert_sink())
        .await
        .unwrap()
}

fn message(subject: &str) -> Vec<u8> {
    let id = unique("m");
    format!(
        "From: Bob <bob@nebulamail.test>\r\nTo: Alice <alice@nebulamail.test>\r\n\
         Subject: {subject}\r\nMessage-ID: <{id}@nebula.test>\r\n\
         Date: Thu, 30 Oct 2026 10:00:00 +0000\r\n\r\nHola.\r\n"
    )
    .into_bytes()
}

#[tokio::test]
async fn sync_queued_behind_move_or_copy_keeps_the_email_id() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;

    let names = [unique("race-a"), unique("race-b"), unique("race-c")];
    let mut created = Vec::new();
    for name in &names {
        created.push(account.create_mailbox(name, None).await.unwrap());
    }
    let outcome = tokio::spawn(scenario(
        Arc::clone(&account),
        store.clone(),
        account_id,
        [created[0], created[1], created[2]],
    ))
    .await;
    for id in created {
        let _ = account
            .delete_mailbox(id, crate::OnDestroy::RemoveEmails)
            .await;
    }
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

async fn scenario(
    account: Arc<MailAccount>,
    store: Store,
    account_id: AccountId,
    [a, b, c]: [MailboxId; 3],
) {
    let email = account
        .append(a, &message("carrera"), &BTreeSet::new(), None)
        .await
        .unwrap()
        .email;

    // MOVE into `b`, never synced.
    let moving = Arc::clone(&account);
    race(&account, b, async move {
        moving.set_mailboxes(email, BTreeSet::from([b])).await
    })
    .await;
    account.sync_mailbox(a).await.unwrap();
    account.sync_mailbox(b).await.unwrap();
    assert!(emails_in(&store, account_id, a).await.is_empty());
    assert_eq!(emails_in(&store, account_id, b).await, vec![email]);

    // COPY into `c`, never synced.
    let copying = Arc::clone(&account);
    race(&account, c, async move {
        copying.set_mailboxes(email, BTreeSet::from([b, c])).await
    })
    .await;
    account.sync_mailbox(b).await.unwrap();
    account.sync_mailbox(c).await.unwrap();
    assert_eq!(emails_in(&store, account_id, b).await, vec![email]);
    assert_eq!(emails_in(&store, account_id, c).await, vec![email]);
    let mailboxes: BTreeSet<MailboxId> = store
        .locations(account_id, email)
        .await
        .unwrap()
        .iter()
        .map(|l| l.mailbox)
        .collect();
    assert_eq!(mailboxes, BTreeSet::from([b, c]));
}

#[tokio::test]
async fn search_skips_a_mailbox_deleted_by_another_client() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    let names = [unique("search-kept"), unique("search-gone")];
    let kept = account.create_mailbox(&names[0], None).await.unwrap();
    let gone = account.create_mailbox(&names[1], None).await.unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        async move {
            let subject = unique("buscar");
            let email = account
                .append(kept, &message(&subject), &BTreeSet::new(), None)
                .await
                .unwrap()
                .email;
            // Another client deletes `gone`; our store still lists it.
            let gone_name = store.mailbox(account_id, gone).await.unwrap().imap_name;
            let mut other = account
                .connector()
                .login(&account.credentials(), account.alert_sink())
                .await
                .unwrap();
            other.delete(&gone_name).await.unwrap();
            let _ = other.logout().await;
            let condition = TextCondition {
                field: TextField::Subject,
                value: subject,
            };
            let found = account.search(None, &[condition]).await.unwrap();
            assert_eq!(found, BTreeSet::from([email]));
        }
    })
    .await;
    for id in [kept, gone] {
        let _ = account
            .delete_mailbox(id, crate::OnDestroy::RemoveEmails)
            .await;
    }
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

/// A multipart message with `parts` empty parts: its BODYSTRUCTURE is far
/// longer than the message.
fn many_parts(parts: usize) -> Vec<u8> {
    let id = unique("parts");
    let mut raw = format!(
        "From: Bob <[EMAIL]>\r\nTo: Alice <[EMAIL]>\r\n\
         Subject: muchas partes\r\nMessage-ID: <{id}@nebula.test>\r\n\
         Date: Thu, 30 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=b\r\n\r\n"
    );
    for _ in 0..parts {
        raw.push_str("--b\r\nContent-Type: text/plain\r\n\r\n\r\n");
    }
    raw.push_str("--b--\r\n");
    raw.into_bytes()
}

#[tokio::test]
async fn a_huge_bodystructure_does_not_block_the_sync() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    // 4 KiB lines: the BODYSTRUCTURE of 3000 parts is dropped, while the
    // metadata without it fits.
    let limits = nebula_imap::Limits {
        frame: nebula_imap::proto::response::FrameLimits {
            max_line: 4096,
            ..nebula_imap::proto::response::FrameLimits::default()
        },
        ..nebula_imap::Limits::default()
    };
    let (account, store, account_id, _dir) = account_with(test, limits).await;
    let name = unique("parts");
    let folder = account.create_mailbox(&name, None).await.unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            let imap_name = store.mailbox(account_id, folder).await.unwrap().imap_name;
            // Another client adds them: our store has never seen them.
            let mut other = account
                .connector()
                .login(&account.credentials(), account.alert_sink())
                .await
                .unwrap();
            other
                .append(&imap_name, &many_parts(3000), &[], None)
                .await
                .unwrap();
            other
                .append(&imap_name, &message("normal"), &[], None)
                .await
                .unwrap();
            let _ = other.logout().await;
            account.sync_mailbox(folder).await.unwrap();
            let ids = emails_in(&store, account_id, folder).await;
            assert_eq!(ids.len(), 2);
            // Asked again without BODYSTRUCTURE: no attachments known.
            let emails = store.emails(account_id, ids).await.unwrap();
            let subjects: BTreeSet<_> = emails
                .iter()
                .map(|e| e.meta.subject.clone().unwrap_or_default())
                .collect();
            assert_eq!(
                subjects,
                BTreeSet::from(["muchas partes".to_owned(), "normal".to_owned()])
            );
            for email in &emails {
                assert!(!email.meta.has_attachment, "{:?}", email.meta.subject);
            }
        }
    })
    .await;
    let _ = account
        .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
        .await;
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

#[tokio::test]
async fn concurrent_mailbox_changes_keep_the_email() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    let names = [unique("member-a"), unique("member-b")];
    let a = account.create_mailbox(&names[0], None).await.unwrap();
    let b = account.create_mailbox(&names[1], None).await.unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            let email = account
                .append(a, &message("dos a la vez"), &BTreeSet::new(), None)
                .await
                .unwrap()
                .email;
            account
                .set_mailboxes(email, BTreeSet::from([a, b]))
                .await
                .unwrap();
            // Each one alone keeps one copy; together, without the lock,
            // each could expunge the copy the other keeps.
            let (first, second) = tokio::join!(
                account.set_mailboxes(email, BTreeSet::from([a])),
                account.set_mailboxes(email, BTreeSet::from([b])),
            );
            first.unwrap();
            second.unwrap();
            account.sync_mailbox(a).await.unwrap();
            account.sync_mailbox(b).await.unwrap();
            let in_a = emails_in(&store, account_id, a).await;
            let in_b = emails_in(&store, account_id, b).await;
            assert_eq!(in_a.len() + in_b.len(), 1, "{in_a:?} {in_b:?}");
            assert_eq!(store.locations(account_id, email).await.unwrap().len(), 1);
        }
    })
    .await;
    for id in [a, b] {
        let _ = account
            .delete_mailbox(id, crate::OnDestroy::RemoveEmails)
            .await;
    }
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

#[tokio::test]
async fn a_mailbox_with_unsynced_mail_is_not_deleted() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    let name = unique("keep");
    let folder = account.create_mailbox(&name, None).await.unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            let imap_name = store.mailbox(account_id, folder).await.unwrap().imap_name;
            // Another client adds a message; we never sync it.
            let mut other = account
                .connector()
                .login(&account.credentials(), account.alert_sink())
                .await
                .unwrap();
            other
                .append(&imap_name, &message("sin sincronizar"), &[], None)
                .await
                .unwrap();
            let _ = other.logout().await;
            assert!(emails_in(&store, account_id, folder).await.is_empty());
            let kept = account
                .delete_mailbox(folder, crate::OnDestroy::KeepEmails)
                .await;
            assert!(matches!(kept, Err(MailError::MailboxHasEmail)), "{kept:?}");
            assert!(store.mailbox(account_id, folder).await.is_ok());
            account
                .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
                .await
                .unwrap();
            assert!(store.mailbox(account_id, folder).await.is_err());
        }
    })
    .await;
    let _ = account
        .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
        .await;
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

/// A plain-text message whose text starts with `depth` `(`: a PREVIEW
/// sent as a quoted string has them outside any literal.
fn parentheses(depth: usize) -> Vec<u8> {
    let id = unique("paren");
    let text = "(".repeat(depth);
    format!(
        "From: Bob <bob@{domain}>\r\nTo: Alice <alice@{domain}>\r\n\
         Subject: parentesis\r\nMessage-ID: <{id}@nebula.test>\r\n\
         Date: Thu, 30 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{text} hola\r\n",
        domain = nebula_tests::DOMAIN,
    )
    .into_bytes()
}

/// A PREVIEW with more `(` than the parser nests (`FrameLimits::max_depth`)
/// fails the whole FETCH answer of that message; it is asked again without
/// PREVIEW (and without BODYSTRUCTURE) and stored, not left out.
///
/// Dovecot 2.4 sends PREVIEW as a quoted string, so the `(` count there:
/// that is where this test tells the fix from no fix. Stalwart 1.0 sends it
/// as a literal, which the depth guard skips: there it only checks that
/// nothing breaks.
#[tokio::test]
async fn fetch_metadata_retries_without_preview() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    let folder = account
        .create_mailbox(&unique("preview"), None)
        .await
        .unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            let name = imap_name(&store, account_id, folder).await;
            let mut other = other_client(&account).await;
            if !other.capabilities().has(&Capability::Preview) {
                eprintln!("{}: no PREVIEW, nothing to test", test.kind);
                let _ = other.logout().await;
                return;
            }
            let depth = nebula_imap::proto::response::FrameLimits::default().max_depth * 2;
            other
                .append(&name, &parentheses(depth), &[], None)
                .await
                .unwrap();
            other
                .append(&name, &message("normal"), &[], None)
                .await
                .unwrap();
            let _ = other.logout().await;
            account.sync_mailbox(folder).await.unwrap();
            let ids = emails_in(&store, account_id, folder).await;
            assert_eq!(
                ids.len(),
                2,
                "the message with the deep PREVIEW was left out"
            );
            let emails = store.emails(account_id, ids).await.unwrap();
            let deep = emails
                .iter()
                .find(|e| e.meta.subject.as_deref() == Some("parentesis"))
                .unwrap();
            assert!(!deep.meta.has_attachment);
        }
    })
    .await;
    let _ = account
        .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
        .await;
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

/// Another client deletes a mailbox between our LIST and its SELECT (or
/// STATUS): that mailbox is skipped and the next ones are still synced,
/// in `sync_all` and in `check_mailboxes`.
#[tokio::test]
async fn a_mailbox_deleted_after_list_does_not_stop_the_sync() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    // Created in this order, `gone` gets the smaller id, so it is synced
    // before `kept` (`sync_order`): otherwise the test proves nothing.
    let gone = account.create_mailbox(&unique("gone"), None).await.unwrap();
    let kept = account.create_mailbox(&unique("kept"), None).await.unwrap();
    let gone2 = account
        .create_mailbox(&unique("gone2"), None)
        .await
        .unwrap();
    let kept2 = account
        .create_mailbox(&unique("kept2"), None)
        .await
        .unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            // `sync_all`, with the server changed after its LIST.
            {
                let _one_sync = account.sync_lock.lock().await;
                list::sync_list(&account).await.unwrap();
                let mut other = other_client(&account).await;
                let kept_name = imap_name(&store, account_id, kept).await;
                other
                    .append(&kept_name, &message("tras LIST"), &[], None)
                    .await
                    .unwrap();
                other
                    .delete(&imap_name(&store, account_id, gone).await)
                    .await
                    .unwrap();
                let _ = other.logout().await;
                account.sync_stored_mailboxes().await.unwrap();
            }
            assert_eq!(emails_in(&store, account_id, kept).await.len(), 1);

            // `check_mailboxes`: both changed since that sync (a message
            // each), and the first one goes away after LIST.
            let gone_name = imap_name(&store, account_id, gone2).await;
            let kept_name = imap_name(&store, account_id, kept2).await;
            let mut other = other_client(&account).await;
            for name in [&gone_name, &kept_name] {
                other
                    .append(name, &message("antes de LIST"), &[], None)
                    .await
                    .unwrap();
            }
            let _one_sync = account.sync_lock.lock().await;
            let listed = list::sync_list(&account).await.unwrap();
            // Which branch fails for `gone2`: SELECT after LIST-STATUS, or
            // its own STATUS. Every server of docker/compose.yaml has
            // LIST-STATUS (Stalwart 1.0, Dovecot 2.4): the STATUS branch is
            // only covered by the unit tests of `tracked`.
            let via_list_status = listed.statuses.contains_key(gone_name.as_str());
            eprintln!(
                "{}: the deleted mailbox fails at {}",
                test.kind,
                if via_list_status {
                    "SELECT (LIST-STATUS)"
                } else {
                    "STATUS"
                }
            );
            assert!(via_list_status, "{}: no LIST-STATUS?", test.kind);
            other.delete(&gone_name).await.unwrap();
            let _ = other.logout().await;
            account.check_listed(listed).await.unwrap();
            assert_eq!(emails_in(&store, account_id, kept2).await.len(), 1);
        }
    })
    .await;
    for id in [gone, kept, gone2, kept2] {
        let _ = account
            .delete_mailbox(id, crate::OnDestroy::RemoveEmails)
            .await;
    }
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

/// A login while the account runs, with a password that changed on the
/// server: the running account is kept and its next connections use the
/// new password (no `AuthFailed`, nothing reported).
///
/// The account can't really change alice's password (docker/.env has it),
/// so it is given an old one that no longer works, as after the change,
/// right before the new login. Nothing logs in with the old one here:
/// Stalwart bans after repeated failures.
#[tokio::test]
async fn relogin_with_a_new_password_reconnects_with_it() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (mail, _store, account_id, _dir) = service_with(test, nebula_imap::Limits::default()).await;
    let mail = Arc::new(mail);
    let outcome = tokio::spawn({
        let mail = Arc::clone(&mail);
        async move {
            let password = || test.password(User::Alice);
            let running = mail
                .open(account_id, "alice", password(), Start::Login)
                .await
                .unwrap();
            running.set_credentials(nebula_imap::Credentials::new(
                "alice",
                secrecy::SecretString::from("old-password-no-longer-valid"),
            ));
            let again = mail
                .open(account_id, "alice", password(), Start::Login)
                .await
                .unwrap();
            assert!(
                Arc::ptr_eq(&running, &again),
                "{}",
                if running.auth_failed() {
                    "the live task reconnected with the old password between \
                     set_credentials and open (the account is auth_failed): a race \
                     of this test, not the fix failing; run it again"
                } else {
                    "open started the account again instead of keeping it"
                }
            );
            // The next connection logs in with what `open` was given.
            running.slot().await.close().await;
            running.sync_all().await.unwrap();
            assert!(!running.auth_failed());
            let reported =
                tokio::time::timeout(Duration::from_millis(200), mail.auth_failures()).await;
            assert!(reported.is_err(), "{reported:?}");
        }
    })
    .await;
    mail.close(account_id).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}

/// Mail another client adds to the mailbox our connection has selected is
/// found by the periodic check. Dovecot 2.4 answers LIST-STATUS (and
/// STATUS) for the selected mailbox with what our session last saw, so
/// trusting it would miss that mail until something else changed there.
#[tokio::test]
async fn check_mailboxes_sees_new_mail_in_the_selected_mailbox() {
    let Some(test) = TestServer::imap() else {
        return;
    };
    let (account, store, account_id, _dir) = account(test).await;
    let folder = account
        .create_mailbox(&unique("selected"), None)
        .await
        .unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        async move {
            // Leaves `folder` selected on our connection.
            account.sync_mailbox(folder).await.unwrap();
            let name = imap_name(&store, account_id, folder).await;
            assert_eq!(
                account
                    .slot()
                    .await
                    .selected()
                    .map(|c| c.mailbox().name.clone()),
                Some(name.clone())
            );
            let mut other = other_client(&account).await;
            other
                .append(&name, &message("en la seleccionada"), &[], None)
                .await
                .unwrap();
            let _ = other.logout().await;
            account.check_mailboxes().await.unwrap();
            assert_eq!(emails_in(&store, account_id, folder).await.len(), 1);
        }
    })
    .await;
    let _ = account
        .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
        .await;
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
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

/// Base64 in lines of 76 characters.
fn base64_lines(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() / 3 * 4 + data.len() / 57 * 2 + 8);
    for line in data.chunks(57) {
        for chunk in line.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4_u32 {
                if usize::try_from(i).unwrap() <= chunk.len() {
                    let index = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap();
                    out.push(char::from(ALPHABET[index]));
                } else {
                    out.push('=');
                }
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// A real message near the limits: 47–48 MB, under the 50 MB
/// `max_size_upload` of the default config, with a text part, an HTML part
/// and two attachments of 17.5 MB (base64). Returns it and the two
/// attachments as they decode.
fn large_message(subject: &str, seed: u64) -> (Vec<u8>, [Vec<u8>; 2]) {
    let first = noise(17_500_000, seed);
    let second = noise(17_500_000, seed.rotate_left(17) ^ 0x0005_DEEC_E66D);
    let id = unique("large");
    let mut text = String::new();
    for i in 0..2000 {
        let _ = write!(text, "L=C3=ADnea {i}: caf=C3=A9 y m=C3=A1s texto.\r\n");
    }
    let raw = format!(
        "From: Bob <{bob}>\r\nTo: Alice <{alice}>\r\n\
         Subject: {subject}\r\nMessage-ID: <{id}@nebula.test>\r\n\
         Date: Thu, 30 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"big\"\r\n\r\n\
         --big\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
         --alt\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Transfer-Encoding: quoted-printable\r\n\r\n{text}\r\n\
         --alt\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <p>Dos adjuntos grandes.</p>\r\n--alt--\r\n\
         --big\r\nContent-Type: application/octet-stream; name=\"uno.bin\"\r\n\
         Content-Disposition: attachment; filename=\"uno.bin\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\
         --big\r\nContent-Type: application/octet-stream; name=\"dos.bin\"\r\n\
         Content-Disposition: attachment; filename=\"dos.bin\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\
         --big--\r\n",
        base64_lines(&first),
        base64_lines(&second),
        bob = User::Bob.address(),
        alice = User::Alice.address(),
    )
    .into_bytes();
    (raw, [first, second])
}

/// All the pieces of a reader.
async fn read_all(mut reader: crate::ChunkReader) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(piece) = reader.next().await.unwrap() {
        out.extend_from_slice(&piece);
    }
    out
}

/// Peak resident memory of this process (`VmHWM`), in MiB; 0 if unknown.
fn peak_rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|value| {
                    value
                        .trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
        })
        .map_or(0, |kib| kib / 1024)
}

/// A real 47–48 MB message (two 17.5 MB attachments) that another client
/// appended, loaded through `load_message` (memory for
/// whole messages): the bytes are the server's, it is counted exactly in
/// the lease (two copies), it takes the account's turn and is not cached;
/// a second load of the account waits for the turn and goes on once the
/// first gives it back; a third one that doesn't fit in the budget is
/// "busy" after the wait and keeps nothing; a load cancelled half way
/// gives everything back. Its parts and the whole message also stream in
/// pieces with the same bytes. The budget is all free again at the end.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn a_large_real_message_loads_within_the_budget() {
    use crate::core::memory::MIB;
    use crate::{Copies, Unavailable};

    let Some(test) = TestServer::imap() else {
        return;
    };
    // 160 MiB: room for one load of two copies (92 MiB) and one of one
    // (46 MiB), not for a third; a short wait so that "busy" comes soon.
    let (mail, store, account_id, _dir) =
        service_with_config(test, nebula_imap::Limits::default(), |config| {
            config.whole_memory = 160 * MIB;
            config.whole_wait = Duration::from_secs(3);
        })
        .await;
    let account = Arc::new(MailAccount::new(
        account_id,
        Arc::clone(&mail.shared),
        nebula_imap::Credentials::new("alice", test.password(User::Alice)),
    ));
    let name = unique("large");
    let folder = account.create_mailbox(&name, None).await.unwrap();
    let outcome = tokio::spawn({
        let account = Arc::clone(&account);
        let store = store.clone();
        let mail_shared = Arc::clone(&mail.shared);
        async move {
            let seed = u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
                    % 1_000_000,
            )
            .unwrap()
                | 1;
            let (raw, attachments) = large_message("mensaje grande", seed);
            let len = u64::try_from(raw.len()).unwrap();
            assert!((45 * MIB..50_000_000).contains(&len), "{len} bytes");
            let rss_before = peak_rss_mib();

            // Another client appends it: a real APPEND of 47+ MB.
            let imap_name = store.mailbox(account_id, folder).await.unwrap().imap_name;
            let mut other = other_client(&account).await;
            let started = std::time::Instant::now();
            other.append(&imap_name, &raw, &[], None).await.unwrap();
            let append_time = started.elapsed();
            let _ = other.logout().await;
            account.sync_mailbox(folder).await.unwrap();
            let ids = emails_in(&store, account_id, folder).await;
            assert_eq!(ids.len(), 1, "{ids:?}");
            let email = ids[0];
            let stored = store.emails(account_id, vec![email]).await.unwrap();
            assert_eq!(stored[0].meta.size, len, "RFC822.SIZE is the real size");
            assert!(stored[0].meta.has_attachment);

            let memory = &mail_shared.memory;
            let total = memory.available();
            assert_eq!(total, 160 * MIB);
            let permits = |bytes: u64| bytes.div_ceil(MIB) * MIB;

            // The first load: two copies, the turn, not cached.
            let mut first = account.lease();
            let started = std::time::Instant::now();
            let loaded = account
                .load_message(email, &mut first, Copies::Two)
                .await
                .unwrap();
            let load_time = started.elapsed();
            assert!(*loaded == *raw, "the server's bytes");
            assert_eq!(first.bytes(), 2 * len);
            assert!(first.has_turn());
            assert_eq!(memory.available(), total - permits(2 * len));
            assert!(
                mail_shared.cache.get(account_id, email).is_none(),
                "bigger than the cache's largest item"
            );
            assert_eq!(account.cached_message(email).await.unwrap(), None);

            // A second load of the account waits for the turn.
            let second = tokio::spawn({
                let account = Arc::clone(&account);
                async move {
                    let mut lease = account.lease();
                    let raw = account.load_message(email, &mut lease, Copies::One).await;
                    (raw.map(|raw| raw.len()), lease)
                }
            });
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(!second.is_finished(), "waits for the account's turn");
            assert_eq!(memory.available(), total - permits(2 * len));
            first.drop_turn();
            let (second_len, mut second) = second.await.unwrap();
            assert_eq!(second_len.unwrap(), raw.len());
            assert_eq!(second.bytes(), len);
            assert!(second.has_turn());
            second.drop_turn();
            let left = total - permits(2 * len) - permits(len);
            assert_eq!(memory.available(), left);

            // A third doesn't fit: busy after the wait, nothing kept.
            let mut third = account.lease();
            let started = std::time::Instant::now();
            let refused = account.load_message(email, &mut third, Copies::One).await;
            assert!(
                matches!(refused, Err(MailError::Unavailable(Unavailable::Busy))),
                "{refused:?}"
            );
            assert!(started.elapsed() >= Duration::from_secs(3));
            assert_eq!(third.bytes(), 0);
            assert_eq!(memory.available(), left);
            drop(third);

            // Cancelled half way (while the server sends it): nothing leaks.
            drop(second);
            drop(first);
            drop(loaded);
            assert_eq!(memory.available(), total);
            let cancelled = tokio::spawn({
                let account = Arc::clone(&account);
                async move {
                    let mut lease = account.lease();
                    let _ = account.load_message(email, &mut lease, Copies::Two).await;
                    // Never here: aborted first.
                    std::future::pending::<()>().await;
                }
            });
            // Its memory is reserved before the FETCH.
            let mut reserved = false;
            for _ in 0..200 {
                if memory.available() < total {
                    reserved = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(reserved, "the load reserves before it fetches");
            cancelled.abort();
            assert!(cancelled.await.unwrap_err().is_cancelled());
            assert_eq!(memory.available(), total, "an aborted load gives all back");
            // The connection it was using is still good for the next one.
            let mut again = account.lease();
            let reloaded = account
                .load_message(email, &mut again, Copies::One)
                .await
                .unwrap();
            assert_eq!(reloaded.len(), raw.len());
            drop(again);
            drop(reloaded);
            assert_eq!(memory.available(), total);

            // Streamed: the attachments and the whole message in pieces.
            let parsed = crate::ParsedMessage::parse(&raw).unwrap();
            let structure = account.body_structure(email).await.unwrap().unwrap();
            let sections = crate::streamable_parts(&parsed, &structure);
            let started = std::time::Instant::now();
            for (index, expected) in [(4_u32, &attachments[0]), (5, &attachments[1])] {
                let section = sections
                    .get(&index)
                    .unwrap_or_else(|| panic!("part {index} streamable: {:?}", sections.keys()));
                let download = account.part_reader(email, section).await.unwrap().unwrap();
                assert_eq!(download.media_type, "application/octet-stream");
                let bytes = read_all(download.reader).await;
                assert!(
                    bytes == *expected,
                    "part {index}: {} bytes, expected {}",
                    bytes.len(),
                    expected.len()
                );
            }
            let reader = account.message_reader(email).await.unwrap().unwrap();
            assert_eq!(reader.size(), len);
            assert!(read_all(reader).await == raw, "the whole message streamed");
            let stream_time = started.elapsed();
            assert_eq!(memory.available(), total, "streaming takes no budget");
            eprintln!(
                "large message: {len} bytes; APPEND {append_time:?}, load_message (2 copies) \
                 {load_time:?}, streaming both attachments and the message {stream_time:?}; \
                 peak RSS {rss_before} MiB before, {} MiB after",
                peak_rss_mib()
            );
        }
    })
    .await;
    let _ = account
        .delete_mailbox(folder, crate::OnDestroy::RemoveEmails)
        .await;
    account.slot().await.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic.into_panic());
    }
}
