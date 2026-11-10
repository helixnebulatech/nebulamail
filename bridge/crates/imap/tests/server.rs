// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Integration tests against the servers of `docker/compose.yaml`
//! (Stalwart or Dovecot, see `nebula-tests`).
//!
//! Skipped unless `NEBULA_TEST_IMAP=1`. Test folders get unique names and
//! are deleted at the end.

// This whole file is test code, also its helper functions (which clippy
// does not see as tests): a panic here is a failed test, not a crash.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::num::NonZeroU32;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_imap::proto::command::{
    Access, AppendMessage, FetchAttribute, FetchModifiers, HeaderName, ListPattern, PartPath,
    Partial, QResyncParams, SearchKey, Section, StatusItem, StoreOperation, StoreResponse,
};
use nebula_imap::proto::response::{Preview, SectionKind, Untagged};
use nebula_imap::proto::types::{Capability, Flag, MailboxName, SpecialUse, UidSet};
use nebula_imap::{
    Authenticated, Client, Connected, Credentials, Error, IdleEvent, Limits, ModSeqState,
    NotAuthenticated, ObjectIdSupport, Resync, SelectOptions, Selected, SyncLevel, Tls,
    TlsConnector, TlsVerify,
};
use nebula_tests::{ServerKind, TestServer, User};
use secrecy::SecretString;
use tokio::net::TcpStream;

fn server() -> Option<TestServer> {
    TestServer::imap()
}

/// The server of the running test; only called after [`server`] said yes.
fn current() -> TestServer {
    server().expect("IMAP tests enabled")
}

async fn connect(tls: Tls, address: String) -> Client<NotAuthenticated> {
    let stream = TcpStream::connect(&address)
        .await
        .unwrap_or_else(|e| panic!("IMAP on {address}: {e}"));
    let connector = TlsConnector::new(TlsVerify::InsecureLoopbackOnly).unwrap();
    let connected = Client::connect(
        stream,
        "localhost".try_into().unwrap(),
        tls,
        &connector,
        Limits::default(),
    )
    .await
    .expect("connect");
    let Connected::NotAuthenticated(client) = connected else {
        panic!("unexpected PREAUTH")
    };
    client
}

async fn login() -> Client<Authenticated> {
    let server = current();
    connect(Tls::Implicit, server.imap.address())
        .await
        .login(&Credentials::new(
            User::Alice.login(),
            server.password(User::Alice),
        ))
        .await
        .expect("login")
}

fn unique(prefix: &str) -> MailboxName {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    MailboxName::new(&format!("{prefix}-{}-{nanos}", std::process::id())).unwrap()
}

fn message(subject: &str, id: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@localhost>\r\nTo: Alice <alice@localhost>\r\nSubject: {subject}\r\n\
         Message-ID: <{id}@nebula.test>\r\nDate: Thu, 24 Oct 2026 10:00:00 +0000\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nHola, esto es una prueba: {subject}.\r\n"
    )
    .into_bytes()
}

#[tokio::test]
async fn wrong_password_is_auth_failed() {
    let Some(server) = server() else { return };
    let client = connect(Tls::Implicit, server.imap.address()).await;
    let error = client
        .login(&Credentials::new(
            User::Alice.login(),
            SecretString::from("definitely-wrong"),
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(error.error(), Error::AuthFailed { .. }),
        "{error:?}"
    );
    assert!(error.has_client());
}

#[tokio::test]
async fn starttls_login() {
    let Some(server) = server() else { return };
    let Some(endpoint) = server.imap_starttls else {
        return;
    };
    let client = connect(Tls::StartTls, endpoint.address()).await;
    let client = client
        .login(&Credentials::new(
            User::Alice.login(),
            server.password(User::Alice),
        ))
        .await
        .expect("login after STARTTLS");
    client.logout().await.unwrap();
}

#[tokio::test]
async fn full_session() {
    if server().is_none() {
        return;
    }
    let mut client = login().await;
    remove_stale_folders(&mut client).await;
    let first = unique("nebula-test-a");
    let second = unique("nebula-test-b");
    client.create(&first, None).await.unwrap();
    client.create(&second, None).await.unwrap();

    // In a task, so that a failed assertion still lets us clean up.
    let (task_first, task_second) = (first.clone(), second.clone());
    let outcome =
        tokio::spawn(async move { scenario(client, &task_first, &task_second).await }).await;

    let mut cleanup = login().await;
    let deleted_first = cleanup.delete(&first).await;
    let deleted_second = cleanup.delete(&second).await;
    cleanup.logout().await.unwrap();
    match outcome {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
    deleted_first.unwrap();
    deleted_second.unwrap();
}

/// Deletes test folders left by earlier runs that crashed (other process
/// ids than ours).
async fn remove_stale_folders(client: &mut Client<Authenticated>) {
    let ours = format!("-{}-", std::process::id());
    let pattern = ListPattern::new("nebula-test-*").unwrap();
    for listed in client.list(&pattern, &[]).await.unwrap() {
        if !listed.entry.name.as_str().contains(&ours) {
            client.delete(&listed.entry.name).await.unwrap();
        }
    }
}

async fn scenario(
    mut client: Client<Authenticated>,
    first: &MailboxName,
    second: &MailboxName,
) -> Result<(), Error> {
    // Capabilities and extensions.
    assert!(client.capabilities().is_imap4());
    assert_eq!(
        client.sync_level(),
        SyncLevel::QResync,
        "both test servers support QRESYNC"
    );

    // LIST with special use and status.
    let mailboxes = client
        .list(
            &ListPattern::all(),
            &[StatusItem::Messages, StatusItem::UidNext],
        )
        .await?;
    assert!(mailboxes.iter().any(|m| m.entry.name.is_inbox()));
    assert!(
        mailboxes
            .iter()
            .any(|m| m.entry.special_use() == Some(SpecialUse::Sent)),
        "special use: {mailboxes:#?}"
    );
    let created = mailboxes
        .iter()
        .find(|m| m.entry.name == *first)
        .expect("test folder listed");
    if client.capabilities().has(&Capability::ListStatus) {
        assert_eq!(created.status.as_ref().and_then(|s| s.messages), Some(0));
    }

    // APPEND two messages, with APPENDUID.
    let appended_one = client
        .append(first, &message("first message", "one"), &[Flag::Seen], None)
        .await?;
    let appended_two = client
        .append(first, &message("second message", "two"), &[], None)
        .await?;
    let (validity, uid_one) = appended_one.uid.expect("APPENDUID");
    let (_, uid_two) = appended_two.uid.expect("APPENDUID");
    assert!(uid_two > uid_one);

    // STATUS.
    let status = client
        .status(first, &[StatusItem::Messages, StatusItem::HighestModSeq])
        .await?;
    assert_eq!(status.messages, Some(2));

    // SELECT with CONDSTORE.
    let (mut selected, _) = client
        .select(first, SelectOptions::default())
        .await
        .map_err(Error::from)?;
    assert_eq!(selected.mailbox().exists, 2);
    assert_eq!(selected.mailbox().uid_validity, validity);
    assert!(matches!(
        selected.mailbox().mod_seq,
        ModSeqState::Highest(_)
    ));

    fetch_and_search(&mut selected, uid_one, uid_two).await?;

    // STORE with the new flags back.
    let stored = selected
        .uid_store(
            &UidSet::single(uid_one),
            StoreOperation::Add,
            &[Flag::Flagged],
            StoreResponse::Fetch,
            None,
        )
        .await?;
    let flags = stored
        .fetched
        .first()
        .and_then(|f| f.flags.clone())
        .unwrap_or_default();
    assert!(flags.contains(&Flag::Flagged));
    let ModSeqState::Highest(before_move) = selected.mailbox().mod_seq else {
        panic!("no HIGHESTMODSEQ")
    };

    // MOVE with COPYUID.
    let moved = selected.uid_move(&UidSet::single(uid_two), second).await?;
    let map = moved.copy_uid.expect("COPYUID");
    assert_eq!(map.pairs.len(), 1);
    assert_eq!(map.pairs[0].0, uid_two);

    // QRESYNC re-select: VANISHED (EARLIER) shows the moved message.
    let client = selected.unselect().await.map_err(Error::from)?;
    let params = QResyncParams {
        uid_validity: validity,
        mod_seq: before_move,
        known_uids: Some(UidSet::range(uid_one, uid_two)),
    };
    let options = SelectOptions {
        access: Access::ReadWrite,
        resync: Resync::QResync(params),
    };
    let (selected, changes) = client.select(first, options).await.map_err(Error::from)?;
    assert_eq!(changes.used, Some(SyncLevel::QResync));
    let vanished: Vec<_> = changes
        .vanished
        .iter()
        .flat_map(|set| set.iter_values().unwrap().collect::<Vec<_>>())
        .collect();
    assert_eq!(vanished, vec![uid_two], "changes: {changes:?}");
    assert_eq!(selected.mailbox().exists, 1);

    idle_wakes_up(selected, first).await
}

async fn fetch_and_search(
    selected: &mut Client<Selected>,
    uid_one: nebula_imap::proto::types::Uid,
    uid_two: nebula_imap::proto::types::Uid,
) -> Result<(), Error> {
    let caps = selected.capabilities().clone();
    let mut attributes = vec![
        FetchAttribute::Uid,
        FetchAttribute::Flags,
        FetchAttribute::InternalDate,
        FetchAttribute::Rfc822Size,
        FetchAttribute::BodyStructure,
        FetchAttribute::ModSeq,
        FetchAttribute::BodyPeek {
            section: Section::HeaderFields(vec![
                HeaderName::new("Subject").unwrap(),
                HeaderName::new("Message-ID").unwrap(),
            ]),
            partial: None,
        },
        FetchAttribute::BodyPeek {
            section: Section::Text,
            partial: Some(Partial {
                offset: 0,
                length: NonZeroU32::new(10).unwrap(),
            }),
        },
    ];
    let object_ids = selected.object_id_support();
    match object_ids {
        ObjectIdSupport::Rfc8474 => {
            attributes.extend([FetchAttribute::EmailId, FetchAttribute::ThreadId]);
        }
        ObjectIdSupport::ObjectIdPlus => attributes.push(FetchAttribute::ObjectIds),
        _ => {}
    }
    if caps.has(&Capability::Preview) {
        attributes.push(FetchAttribute::Preview);
    }
    let fetched = selected
        .uid_fetch(&UidSet::all(), &attributes, FetchModifiers::None)
        .await?;
    assert_eq!(fetched.messages.len(), 2);
    let one = fetched
        .messages
        .iter()
        .find(|m| m.uid == Some(uid_one))
        .expect("first message");
    assert!(one.flags.as_ref().unwrap().contains(&Flag::Seen));
    assert!(one.mod_seq.is_some());
    assert!(one.size.unwrap() > 100);
    assert!(one.internal_date.is_some());
    assert!(!one.body_structure.as_ref().unwrap().has_attachment());
    let header = one
        .sections
        .iter()
        .find(|s| s.section == SectionKind::Header)
        .expect("header section");
    let header = String::from_utf8_lossy(header.data.as_deref().unwrap());
    assert!(header.contains("Subject: first message"), "{header}");
    let text = one
        .sections
        .iter()
        .find(|s| s.section == SectionKind::Text)
        .expect("text section");
    assert_eq!(text.origin, Some(0));
    assert_eq!(text.data.as_ref().map(Vec::len), Some(10));
    if current().kind == ServerKind::Stalwart {
        assert_ne!(object_ids, ObjectIdSupport::None, "Stalwart has stable ids");
        assert!(one.email_id.is_some(), "{one:?}");
        assert!(one.thread_id.is_some(), "{one:?}");
    }
    if caps.has(&Capability::Preview) {
        assert!(
            matches!(&one.preview, Some(Preview::Text(text)) if text.contains("prueba")),
            "{:?}",
            one.preview
        );
    }

    // CHANGEDSINCE with a high mod-sequence: nothing changed.
    let ModSeqState::Highest(highest) = selected.mailbox().mod_seq else {
        panic!()
    };
    let unchanged = selected
        .uid_fetch(
            &UidSet::all(),
            &[FetchAttribute::Flags],
            FetchModifiers::ChangedSince(highest),
        )
        .await?;
    assert!(unchanged.messages.is_empty());
    search(selected, uid_two).await
}

async fn search(
    selected: &mut Client<Selected>,
    uid_two: nebula_imap::proto::types::Uid,
) -> Result<(), Error> {
    // Stalwart and Dovecot's FTS index text in the background, so text
    // searches may need a moment after APPEND.
    let unseen = selected.uid_search(&SearchKey::Unseen).await?;
    assert_eq!(unseen.uids(), vec![uid_two]);
    for criteria in [
        SearchKey::Subject("second".into()),
        SearchKey::Body("prueba: second".into()),
    ] {
        let mut found = Vec::new();
        for _ in 0..20 {
            found = selected.uid_search(&criteria).await?.uids();
            if !found.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert_eq!(found, vec![uid_two], "{criteria:?}");
    }
    Ok(())
}

async fn idle_wakes_up(selected: Client<Selected>, folder: &MailboxName) -> Result<(), Error> {
    // Log the writer in first: after the wrong-password tests Dovecot delays
    // logins from this address, which could eat the whole wait below.
    let mut other = login().await;
    let mut idle = selected.idle().await.map_err(Error::from)?;
    let folder_name = folder.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        other
            .append(&folder_name, &message("pushed", "three"), &[], None)
            .await
            .unwrap();
        other.logout().await.unwrap();
    });
    // The server may push other data first (Stalwart sends STATUS too):
    // wait until EXISTS arrives.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while !seen.iter().any(|c| matches!(c, Untagged::Exists(2))) {
        let event = tokio::time::timeout_at(deadline, idle.wait())
            .await
            .unwrap_or_else(|_| panic!("no EXISTS: {seen:?}"))?;
        let IdleEvent::Changes(changes) = event else {
            panic!("expected changes, got {event:?}")
        };
        seen.extend(changes);
    }
    writer.await.unwrap();
    let selected = idle.done().await?;
    assert_eq!(selected.mailbox().exists, 2);
    let client = selected.unselect().await.map_err(Error::from)?;
    client.logout().await
}

/// A QRESYNC SELECT made while another mailbox is selected still reports
/// what changed. Stalwart 0.16 sends the changes before `[CLOSED]`, which
/// the client used to drop with the old mailbox's data:
/// the mail crate lost flag changes and expunges when the live task had
/// left INBOX selected.
#[tokio::test]
async fn qresync_select_from_another_mailbox() {
    if server().is_none() {
        return;
    }
    let folder = unique("nebula-qresync");
    let mut client = login().await;
    client.create(&folder, None).await.unwrap();
    let task_folder = folder.clone();
    let outcome =
        tokio::spawn(async move { qresync_from_other_scenario(client, &task_folder).await }).await;
    let mut cleanup = login().await;
    let deleted = cleanup.delete(&folder).await;
    cleanup.logout().await.unwrap();
    match outcome {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
    deleted.unwrap();
}

async fn qresync_from_other_scenario(
    mut client: Client<Authenticated>,
    folder: &MailboxName,
) -> Result<(), Error> {
    if client.sync_level() < SyncLevel::QResync {
        return Ok(());
    }
    let one = client
        .append(folder, &message("uno", "qresync-1"), &[], None)
        .await?
        .uid
        .expect("APPENDUID")
        .1;
    let two = client
        .append(folder, &message("dos", "qresync-2"), &[], None)
        .await?
        .uid
        .expect("APPENDUID")
        .1;
    let (selected, _) = client
        .select(folder, SelectOptions::default())
        .await
        .map_err(Error::from)?;
    let state = selected.mailbox().clone();
    let ModSeqState::Highest(known) = state.mod_seq else {
        panic!("no HIGHESTMODSEQ")
    };
    // Another mailbox is selected when the changes happen.
    let (selected, _) = selected
        .select(&MailboxName::inbox(), SelectOptions::default())
        .await
        .map_err(Error::from)?;

    let other = login().await;
    let (mut other, _) = other
        .select(folder, SelectOptions::default())
        .await
        .map_err(Error::from)?;
    other
        .uid_store(
            &UidSet::single(one),
            StoreOperation::Add,
            &[Flag::Flagged],
            StoreResponse::Silent,
            None,
        )
        .await?;
    other
        .uid_store(
            &UidSet::single(two),
            StoreOperation::Add,
            &[Flag::Deleted],
            StoreResponse::Silent,
            None,
        )
        .await?;
    other.uid_expunge(&UidSet::single(two)).await?;
    other
        .unselect()
        .await
        .map_err(Error::from)?
        .logout()
        .await?;

    // Straight from INBOX to the folder, with QRESYNC.
    let options = SelectOptions {
        resync: Resync::QResync(QResyncParams {
            uid_validity: state.uid_validity,
            mod_seq: known,
            known_uids: None,
        }),
        ..SelectOptions::default()
    };
    let (selected, changes) = selected
        .select(folder, options)
        .await
        .map_err(Error::from)?;
    assert_eq!(changes.used, Some(SyncLevel::QResync));
    let flagged: Vec<_> = changes
        .changed
        .iter()
        .filter(|data| {
            data.uid == Some(one)
                && data
                    .flags
                    .as_ref()
                    .is_some_and(|flags| flags.contains(&Flag::Flagged))
        })
        .collect();
    assert_eq!(flagged.len(), 1, "changes: {changes:?}");
    let vanished: Vec<_> = changes
        .vanished
        .iter()
        .flat_map(|set| set.iter_values().unwrap().collect::<Vec<_>>())
        .collect();
    assert_eq!(vanished, vec![two], "changes: {changes:?}");
    selected
        .unselect()
        .await
        .map_err(Error::from)?
        .logout()
        .await
}

/// ESEARCH, BINARY, MULTIAPPEND, LIST subscriptions and NOTIFY, used when
/// the server has them (Dovecot has all; Stalwart has no NOTIFY).
#[tokio::test]
async fn extensions() {
    if server().is_none() {
        return;
    }
    let folder = unique("nebula-ext");
    let mut client = login().await;
    client.create(&folder, None).await.unwrap();
    let task_folder = folder.clone();
    let outcome =
        tokio::spawn(async move { extensions_scenario(client, &task_folder).await }).await;
    let mut cleanup = login().await;
    let deleted = cleanup.delete(&folder).await;
    cleanup.logout().await.unwrap();
    match outcome {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
    deleted.unwrap();
}

async fn extensions_scenario(
    mut client: Client<Authenticated>,
    folder: &MailboxName,
) -> Result<(), Error> {
    let caps = client.capabilities().clone();
    // What each test server has (checked 2026-10-26).
    let expected = match current().kind {
        ServerKind::Dovecot | ServerKind::Postfix | ServerKind::DovecotPrefix => {
            [true, true, true, true]
        }
        ServerKind::Stalwart => [true, true, true, false],
    };
    let has = [
        Capability::ESearch,
        Capability::Binary,
        Capability::MultiAppend,
        Capability::Notify,
    ]
    .map(|capability| caps.has(&capability));
    assert_eq!(has, expected, "ESEARCH BINARY MULTIAPPEND NOTIFY: {caps:?}");

    // MULTIAPPEND: two messages in one command.
    let one = message("multi one", "ext-one");
    let two = message("multi two", "ext-two");
    let mut count = 0;
    if caps.has(&Capability::MultiAppend) {
        let messages = [
            AppendMessage {
                flags: &[Flag::Seen],
                internal_date: None,
                message: &one,
            },
            AppendMessage {
                flags: &[],
                internal_date: None,
                message: &two,
            },
        ];
        let result = client.append_many(folder, &messages).await?;
        let (_, uids) = result.uids.expect("APPENDUID for both");
        assert_eq!(uids.len(), 2);
        assert!(uids[1] > uids[0]);
        count += 2;
    }

    // RFC 6855 §4: after ENABLE UTF8=ACCEPT, UTF-8 header fields go as
    // `UTF8 (~{n})`; the server must take them.
    if client.name_encoding() == nebula_imap::proto::types::NameEncoding::Utf8 {
        let utf8 = "From: Bob <bob@localhost>\r\nTo: Álice <alice@localhost>\r\n\
            Subject: Año nuevo, café\r\nMessage-ID: <[EMAIL]>\r\n\
            MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhola\r\n";
        client.append(folder, utf8.as_bytes(), &[], None).await?;
        count += 1;
    }

    // BINARY: a message with a NUL byte in a base64 part goes as literal8.
    let binary_body = "From: Bob <bob@localhost>\r\nTo: Alice <alice@localhost>\r\n\
        Subject: binary\r\nMessage-ID: <ext-bin@nebula.test>\r\nMIME-Version: 1.0\r\n\
        Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\n\
        AAECAw==\r\n";
    client
        .append(folder, binary_body.as_bytes(), &[], None)
        .await?;
    count += 1;

    // LIST: subscriptions come back (RETURN (SUBSCRIBED) or LSUB).
    let pattern = ListPattern::new(folder.as_str()).unwrap();
    client.subscribe(folder).await?;
    let listed = client.list(&pattern, &[]).await?;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].subscribed, "{listed:?}");
    client.unsubscribe(folder).await?;
    let listed = client.list(&pattern, &[]).await?;
    assert!(!listed[0].subscribed, "{listed:?}");

    // ESEARCH (or plain SEARCH): every message.
    let (mut selected, _) = client
        .select(folder, SelectOptions::default())
        .await
        .map_err(Error::from)?;
    let all = selected.uid_search(&SearchKey::All).await?.uids();
    assert_eq!(all.len(), count);

    // BINARY.PEEK: the server decodes the base64 part.
    if caps.has(&Capability::Binary) {
        let last = *all.last().unwrap();
        let fetched = selected
            .uid_fetch(
                &UidSet::single(last),
                &[
                    FetchAttribute::Uid,
                    FetchAttribute::BinarySize {
                        part: Some(PartPath::new(vec![NonZeroU32::new(1).unwrap()]).unwrap()),
                    },
                    FetchAttribute::BinaryPeek {
                        part: Some(PartPath::new(vec![NonZeroU32::new(1).unwrap()]).unwrap()),
                        partial: None,
                    },
                ],
                FetchModifiers::None,
            )
            .await?;
        let data = fetched
            .messages
            .iter()
            .find(|m| m.uid == Some(last))
            .expect("fetched");
        assert_eq!(
            data.binary.first().and_then(|b| b.data.as_deref()),
            Some(&[0, 1, 2, 3][..]),
            "{data:?}"
        );
        assert_eq!(data.binary_sizes.first().map(|s| s.1), Some(4));
    }
    let client = selected.unselect().await.map_err(Error::from)?;
    notify_reports_other_mailboxes(client, folder).await
}

async fn notify_reports_other_mailboxes(
    mut client: Client<Authenticated>,
    folder: &MailboxName,
) -> Result<(), Error> {
    use nebula_imap::proto::command::{NotifyEvent, NotifyFilter, NotifyGroup};
    if !client.capabilities().has(&Capability::Notify) {
        return client.logout().await;
    }
    client
        .notify(&[
            NotifyGroup {
                filter: NotifyFilter::Selected,
                events: &[
                    NotifyEvent::MessageNew,
                    NotifyEvent::MessageExpunge,
                    NotifyEvent::FlagChange,
                ],
            },
            NotifyGroup {
                filter: NotifyFilter::Personal,
                events: &[
                    NotifyEvent::MessageNew,
                    NotifyEvent::MessageExpunge,
                    NotifyEvent::MailboxName,
                ],
            },
        ])
        .await?;
    let options = SelectOptions {
        access: Access::ReadOnly,
        ..SelectOptions::default()
    };
    let (selected, _) = client
        .select(&MailboxName::inbox(), options)
        .await
        .map_err(Error::from)?;
    let mut other = login().await;
    let mut idle = selected.idle().await.map_err(Error::from)?;
    let folder_name = folder.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        other
            .append(&folder_name, &message("notified", "ext-notify"), &[], None)
            .await
            .unwrap();
        other.logout().await.unwrap();
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while !seen
        .iter()
        .any(|c| matches!(c, Untagged::MailboxStatus(s) if s.mailbox == *folder))
    {
        let event = tokio::time::timeout_at(deadline, idle.wait())
            .await
            .unwrap_or_else(|_| panic!("no STATUS for {folder:?}: {seen:?}"))?;
        if let IdleEvent::Changes(changes) = event {
            seen.extend(changes);
        }
    }
    writer.await.unwrap();
    let selected = idle.done().await?;
    let client = selected.unselect().await.map_err(Error::from)?;
    client.logout().await
}
