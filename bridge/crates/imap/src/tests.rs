// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Client tests against a scripted fake server.
//!
//! Command logic runs over an in-memory stream (the `Test` transport, which
//! only exists in tests). Connection setup runs over real local TCP, with a
//! plaintext server, to check the STARTTLS rules.

use std::time::Duration;

use nebula_imap_proto::command::{Access, FetchAttribute, FetchModifiers, QResyncParams};
use nebula_imap_proto::response::{FrameLimits, Untagged, VanishedKind};
use nebula_imap_proto::types::{Capabilities, MailboxName, ModSeq, Uid, UidSet, UidValidity};
use secrecy::SecretString;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::core::client::{Authenticated, Client, NotAuthenticated, Selected, Session, SyncLevel};
use crate::core::connection::{Connection, SecureStream};
use crate::{
    Credentials, Error, IdleEvent, Limits, ModSeqState, Resync, SelectOptions, Tls, TlsConnector,
    TlsVerify,
};

/// One step of the fake server.
enum Step {
    /// Expect exactly these bytes from the client.
    Recv(&'static str),
    /// Send these bytes.
    Send(&'static str),
}

use Step::{Recv, Send};

async fn play(
    mut stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    script: Vec<Step>,
) {
    for step in script {
        match step {
            Recv(expected) => {
                let mut buffer = vec![0; expected.len()];
                stream
                    .read_exact(&mut buffer)
                    .await
                    .expect("client sent less than expected");
                assert_eq!(String::from_utf8_lossy(&buffer), expected);
            }
            Send(bytes) => stream.write_all(bytes.as_bytes()).await.unwrap(),
        }
    }
    // Keep the stream open a little, so the client sees our last bytes.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn client(capabilities: &str, script: Vec<Step>) -> (Client<NotAuthenticated>, JoinHandle<()>) {
    client_with(capabilities, script, FrameLimits::default())
}

/// Lines of at most 1 KiB, to test responses over the limits.
fn small_frames() -> FrameLimits {
    FrameLimits {
        max_line: 1024,
        max_search_line: 1024,
        ..FrameLimits::default()
    }
}

/// A response line of about `size` bytes: `start`, then `item` repeated.
fn long_line(start: &str, item: &str, size: usize) -> &'static str {
    let mut line = start.to_owned();
    while line.len() < size {
        line.push_str(item);
    }
    line.push_str("\r\n");
    line.leak()
}

fn client_with(
    capabilities: &str,
    script: Vec<Step>,
    frame: FrameLimits,
) -> (Client<NotAuthenticated>, JoinHandle<()>) {
    let (ours, theirs): (DuplexStream, DuplexStream) = tokio::io::duplex(1 << 16);
    let server = tokio::spawn(play(theirs, script));
    let limits = Limits {
        frame,
        read_timeout: Duration::from_secs(5),
        ..Limits::default()
    };
    let client = Client {
        connection: Box::new(Connection::new(SecureStream::Test(ours), limits)),
        session: Session::new(
            Capabilities::parse_atoms(capabilities.split(' ')),
            limits.max_events,
        ),
        state: NotAuthenticated,
    };
    (client, server)
}

fn credentials() -> Credentials {
    Credentials::new("alice", SecretString::from("secret"))
}

fn authenticated(capabilities: &str, script: Vec<Step>) -> (Client<Authenticated>, JoinHandle<()>) {
    let (client, server) = client(capabilities, script);
    (client.into_state(Authenticated), server)
}

#[tokio::test]
async fn plain_with_sasl_ir_and_capabilities_in_ok() {
    let (client, server) = client(
        "IMAP4rev1 AUTH=PLAIN SASL-IR",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev1 IDLE MOVE] Logged in\r\n"),
        ],
    );
    let client = client.login(&credentials()).await.unwrap();
    assert!(
        client
            .capabilities()
            .has(&nebula_imap_proto::types::Capability::Move)
    );
    assert_eq!(client.sync_level(), SyncLevel::None);
    server.await.unwrap();
}

#[tokio::test]
async fn plain_without_sasl_ir_then_enable() {
    let (client, server) = client(
        "IMAP4rev1 AUTH=PLAIN",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN\r\n"),
            Send("+ \r\n"),
            Recv("AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK done\r\n"),
            Recv("A0002 CAPABILITY\r\n"),
            Send("* CAPABILITY IMAP4rev1 ENABLE QRESYNC CONDSTORE UTF8=ACCEPT\r\nA0002 OK\r\n"),
            Recv("A0003 ENABLE QRESYNC UTF8=ACCEPT\r\n"),
            Send("* ENABLED QRESYNC UTF8=ACCEPT\r\nA0003 OK\r\n"),
        ],
    );
    let client = client.login(&credentials()).await.unwrap();
    assert_eq!(client.sync_level(), SyncLevel::QResync);
    assert_eq!(
        client.name_encoding(),
        nebula_imap_proto::types::NameEncoding::Utf8
    );
    server.await.unwrap();
}

#[tokio::test]
async fn auth_failed_gives_the_client_back() {
    let (client, server) = client(
        "IMAP4rev1",
        vec![
            Recv("A0001 LOGIN alice secret\r\n"),
            Send("A0001 NO [AUTHENTICATIONFAILED] Invalid credentials\r\n"),
        ],
    );
    let error = client.login(&credentials()).await.unwrap_err();
    assert!(matches!(error.error(), Error::AuthFailed { .. }));
    assert!(error.has_client());
    server.await.unwrap();
}

#[tokio::test]
async fn login_disabled_without_plain_is_refused() {
    let (client, _server) = client("IMAP4rev1 LOGINDISABLED", vec![]);
    let error = client.login(&credentials()).await.unwrap_err();
    assert!(matches!(error.error(), Error::NoAuthMechanism));
}

#[tokio::test]
async fn synchronizing_literal_waits_for_continuation() {
    let (client, server) = client(
        "IMAP4rev1",
        vec![
            Recv("A0001 LOGIN alice {4}\r\n"),
            Send("+ go on\r\n"),
            Recv("a\r\nb\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev1] ok\r\n"),
        ],
    );
    let credentials = Credentials::new("alice", SecretString::from("a\r\nb"));
    client.login(&credentials).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn select_with_qresync_collects_changes() {
    let (mut client, server) = authenticated(
        "IMAP4rev1 QRESYNC CONDSTORE ENABLE",
        vec![
            Recv("A0001 SELECT INBOX (QRESYNC (67890007 20050715194045000 41:211))\r\n"),
            Send(
                "* OK [CLOSED] previous mailbox closed\r\n\
                 * 314 EXISTS\r\n\
                 * FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $Junk)\r\n\
                 * OK [PERMANENTFLAGS (\\Seen \\Deleted \\*)] ok\r\n\
                 * OK [UIDVALIDITY 67890007] UIDs valid\r\n\
                 * OK [UIDNEXT 567] next\r\n\
                 * OK [HIGHESTMODSEQ 20060128194045000] highest\r\n\
                 * VANISHED (EARLIER) 41,43:116,118\r\n\
                 * 49 FETCH (UID 117 FLAGS (\\Seen \\Answered) MODSEQ (20050715194045001))\r\n\
                 * OK [ALERT] Mailbox almost full\r\n\
                 A0001 OK [READ-WRITE] mailbox selected\r\n",
            ),
        ],
    );
    client.session.sync = SyncLevel::QResync;
    let params = QResyncParams {
        uid_validity: UidValidity::new(67_890_007).unwrap(),
        mod_seq: ModSeq::new(20_050_715_194_045_000).unwrap(),
        known_uids: Some(UidSet::range(Uid::new(41).unwrap(), Uid::new(211).unwrap())),
    };
    let options = SelectOptions {
        access: Access::ReadWrite,
        resync: Resync::QResync(params),
    };
    let (mut selected, changes) = client.select(&MailboxName::inbox(), options).await.unwrap();
    let mailbox = selected.mailbox();
    assert_eq!(mailbox.exists, 314);
    assert_eq!(mailbox.uid_next.map(Uid::get), Some(567));
    assert_eq!(
        mailbox.mod_seq,
        ModSeqState::Highest(ModSeq::new(20_060_128_194_045_000).unwrap())
    );
    assert_eq!(mailbox.access, Access::ReadWrite);
    assert_eq!(mailbox.flags.len(), 6);
    assert_eq!(changes.vanished.len(), 1);
    assert_eq!(changes.changed.len(), 1);
    // The alert is not lost: it is an event.
    let events = selected.take_events();
    assert_eq!(events.items.len(), 1, "{events:?}");
    server.await.unwrap();
}

#[tokio::test]
async fn select_no_returns_authenticated_client() {
    let (client, server) = authenticated(
        "IMAP4rev1",
        vec![
            Recv("A0001 SELECT Nope\r\n"),
            Send("A0001 NO [NONEXISTENT] no such mailbox\r\n"),
        ],
    );
    let error = client
        .select(&MailboxName::new("Nope").unwrap(), SelectOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error.error(), Error::Refused(_)));
    assert!(error.has_client());
    server.await.unwrap();
}

fn selected(capabilities: &str, script: Vec<Step>) -> (Client<Selected>, JoinHandle<()>) {
    selected_with(capabilities, script, FrameLimits::default())
}

fn selected_with(
    capabilities: &str,
    script: Vec<Step>,
    frame: FrameLimits,
) -> (Client<Selected>, JoinHandle<()>) {
    let (client, server) = client_with(capabilities, script, frame);
    let mailbox = crate::core::client::MailboxState {
        name: MailboxName::inbox(),
        access: Access::ReadWrite,
        uid_validity: UidValidity::new(1).unwrap(),
        uid_next: None,
        exists: 3,
        mod_seq: ModSeqState::Unknown,
        flags: Vec::new(),
        permanent_flags: crate::PermanentFlags::NotAnnounced,
    };
    (client.into_state(Selected { mailbox }), server)
}

#[tokio::test]
async fn unsolicited_responses_become_events() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 NOOP\r\n"),
            Send(
                "* 2 EXPUNGE\r\n* 3 EXISTS\r\n* 1 FETCH (FLAGS (\\Seen))\r\n* 99 BOGUS stuff\r\nA0001 OK\r\n",
            ),
        ],
    );
    client.noop().await.unwrap();
    let events = client.take_events();
    // The unknown response is only logged.
    assert_eq!(events.items.len(), 3, "{events:?}");
    assert!(!events.overflowed);
    assert_eq!(client.mailbox().exists, 3);
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_rejects_missing_extensions() {
    let (mut client, _server) = selected("IMAP4rev1", vec![]);
    let all = UidSet::all();
    let modseq = ModSeq::new(5).unwrap();
    let error = client
        .uid_fetch(
            &all,
            &[FetchAttribute::Uid],
            FetchModifiers::ChangedSince(modseq),
        )
        .await;
    assert!(matches!(error, Err(Error::Unsupported("CONDSTORE"))));
    let error = client
        .uid_fetch(&all, &[FetchAttribute::EmailId], FetchModifiers::None)
        .await;
    assert!(matches!(error, Err(Error::Unsupported("OBJECTID"))));
    // Nothing was sent, so the connection still works.
    assert!(!client.is_broken());
}

#[tokio::test]
async fn fetch_streams_and_collects_vanished() {
    let (mut client, server) = selected(
        "IMAP4rev1 CONDSTORE QRESYNC",
        vec![
            Recv("A0001 UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE 7 VANISHED)\r\n"),
            Send(
                "* VANISHED (EARLIER) 5:6\r\n* 1 FETCH (UID 2 FLAGS ())\r\n* 2 FETCH (UID 3 FLAGS (\\Seen))\r\nA0001 OK\r\n",
            ),
        ],
    );
    client.session.sync = SyncLevel::QResync;
    let result = client
        .uid_fetch(
            &UidSet::all(),
            &[FetchAttribute::Uid, FetchAttribute::Flags],
            FetchModifiers::ChangedSinceWithVanished(ModSeq::new(7).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.vanished.len(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn move_falls_back_to_copy_store_expunge() {
    let (mut client, server) = selected(
        "IMAP4rev1 UIDPLUS",
        vec![
            Recv("A0001 UID COPY 4:5 Trash\r\n"),
            Send("A0001 OK [COPYUID 9 4:5 10:11] copied\r\n"),
            Recv("A0002 UID STORE 4:5 +FLAGS.SILENT (\\Deleted)\r\n"),
            Send("A0002 OK\r\n"),
            Recv("A0003 UID EXPUNGE 4:5\r\n"),
            Send("* 3 EXPUNGE\r\n* 3 EXPUNGE\r\nA0003 OK\r\n"),
        ],
    );
    let uids = UidSet::range(Uid::new(4).unwrap(), Uid::new(5).unwrap());
    let result = client
        .uid_move(&uids, &MailboxName::new("Trash").unwrap())
        .await
        .unwrap();
    let map = result.copy_uid.unwrap();
    assert_eq!(
        map.pairs,
        vec![
            (Uid::new(4).unwrap(), Uid::new(10).unwrap()),
            (Uid::new(5).unwrap(), Uid::new(11).unwrap())
        ]
    );
    assert_eq!(client.mailbox().exists, 1);
    server.await.unwrap();
}

/// RFC 6851 as Dovecot and Stalwart answer: COPYUID in an untagged OK
/// before the expunges, the tagged OK without it.
#[tokio::test]
async fn move_takes_copyuid_from_the_untagged_ok() {
    let answers = [
        "* OK [COPYUID 1761800000 5 12] Moved UIDs.\r\n* 2 EXPUNGE\r\n\
         A0001 OK Move completed (0.002 + 0.000 secs).\r\n",
        "* OK [COPYUID 1761800000 5 12] Moved UIDs.\r\n* VANISHED 5\r\n\
         A0001 OK Move completed (0.002 + 0.000 secs).\r\n",
    ];
    for answer in answers {
        let (mut client, server) = selected(
            "IMAP4rev1 UIDPLUS MOVE",
            vec![Recv("A0001 UID MOVE 5 Archive\r\n"), Send(answer)],
        );
        let moved = client
            .uid_move(
                &UidSet::single(Uid::new(5).unwrap()),
                &MailboxName::new("Archive").unwrap(),
            )
            .await
            .unwrap();
        let map = moved.copy_uid.unwrap();
        assert_eq!(map.uid_validity, UidValidity::new(1_761_800_000).unwrap());
        assert_eq!(
            map.pairs,
            vec![(Uid::new(5).unwrap(), Uid::new(12).unwrap())]
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn move_without_move_or_uidplus_is_refused() {
    let (mut client, _server) = selected("IMAP4rev1", vec![]);
    let result = client
        .uid_move(&UidSet::all(), &MailboxName::new("Trash").unwrap())
        .await;
    assert!(matches!(result, Err(Error::Unsupported(_))));
}

#[tokio::test]
async fn idle_wakes_up_and_ends() {
    let (client, server) = selected(
        "IMAP4rev1 IDLE",
        vec![
            Recv("A0001 IDLE\r\n"),
            Send("+ idling\r\n* OK still here\r\n* 4 EXISTS\r\n* VANISHED 7\r\n"),
            Recv("DONE\r\n"),
            Send("* 1 FETCH (FLAGS (\\Seen))\r\nA0001 OK IDLE terminated\r\n"),
        ],
    );
    let mut idle = client.idle().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), idle.wait())
        .await
        .unwrap()
        .unwrap();
    let IdleEvent::Changes(changes) = event else {
        panic!("{event:?}")
    };
    assert!(changes.iter().any(|c| matches!(c, Untagged::Exists(4))));
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Untagged::Vanished(v) if v.kind == VanishedKind::Now))
    );
    assert!(
        !changes.iter().any(|c| matches!(c, Untagged::Status(_))),
        "keepalive not reported"
    );
    let mut client = idle.done().await.unwrap();
    assert_eq!(client.mailbox().exists, 3);
    assert_eq!(client.take_events().items.len(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn idle_refreshes_in_time() {
    let (mut client, server) = selected(
        "IMAP4rev1 IDLE",
        vec![
            Recv("A0001 IDLE\r\n"),
            Send("+ idling\r\n"),
            Recv("DONE\r\n"),
            Send("A0001 OK\r\n"),
        ],
    );
    client.connection.limits.idle_refresh = Duration::from_millis(100);
    let mut idle = client.idle().await.unwrap();
    assert_eq!(idle.wait().await.unwrap(), IdleEvent::Refresh);
    idle.done().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn bye_during_command_is_reported() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![Recv("A0001 NOOP\r\n"), Send("* BYE shutting down\r\n")],
    );
    let error = client.noop().await.unwrap_err();
    server.await.unwrap();
    assert!(
        matches!(&error, Error::Bye(text) if text == "shutting down"),
        "{error:?}"
    );
    assert!(client.is_broken());
    assert!(matches!(client.noop().await, Err(Error::Broken)));
}

#[tokio::test]
async fn oversized_literal_is_dropped_until_the_connection_ends() {
    // Nothing is buffered: the literal is dropped as it comes, and only the
    // timeouts (or the server closing) end it.
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 NOOP\r\n"),
            Send("* 1 FETCH (BODY[] {999999999999}\r\n"),
            Send("A0001 OK this is still inside the literal\r\n"),
        ],
    );
    let error = client.noop().await.unwrap_err();
    assert!(matches!(error, Error::ConnectionClosed), "{error:?}");
    assert_eq!(client.connection.decoder().buffered(), 0);
    assert!(client.is_broken());
    server.await.unwrap();
}

/// A plaintext fake server on local TCP, for the connection setup rules.
async fn tcp_server(script: Vec<Step>) -> (TcpStream, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        play(stream, script).await;
    });
    (TcpStream::connect(address).await.unwrap(), server)
}

async fn connect(stream: TcpStream, tls: Tls) -> Result<crate::Connected, Error> {
    let connector = TlsConnector::new(TlsVerify::InsecureLoopbackOnly).unwrap();
    let limits = Limits {
        greeting_timeout: Duration::from_secs(5),
        ..Limits::default()
    };
    Client::connect(
        stream,
        "localhost".try_into().unwrap(),
        tls,
        &connector,
        limits,
    )
    .await
}

#[tokio::test]
async fn starttls_not_offered_is_an_error() {
    let (stream, server) = tcp_server(vec![Send(
        "* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] ready\r\n",
    )])
    .await;
    assert!(matches!(
        connect(stream, Tls::StartTls).await,
        Err(Error::StartTlsUnavailable)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn starttls_capabilities_are_asked_when_missing() {
    let (stream, server) = tcp_server(vec![
        Send("* OK ready\r\n"),
        Recv("A0001 CAPABILITY\r\n"),
        Send("* CAPABILITY IMAP4rev1 LOGINDISABLED\r\nA0001 OK\r\n"),
    ])
    .await;
    assert!(matches!(
        connect(stream, Tls::StartTls).await,
        Err(Error::StartTlsUnavailable)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn preauth_on_plaintext_is_refused() {
    let (stream, server) = tcp_server(vec![Send(
        "* PREAUTH [CAPABILITY IMAP4rev1 STARTTLS] welcome\r\n",
    )])
    .await;
    assert!(matches!(
        connect(stream, Tls::StartTls).await,
        Err(Error::PreAuthOnPlaintext)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn bye_greeting_is_an_error() {
    let (stream, server) = tcp_server(vec![Send("* BYE too many connections\r\n")]).await;
    assert!(matches!(
        connect(stream, Tls::StartTls).await,
        Err(Error::Bye(_))
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn data_injected_before_tls_is_refused() {
    let (stream, server) = tcp_server(vec![
        Send("* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n"),
        Recv("A0001 STARTTLS\r\n"),
        Send("A0001 OK Begin TLS\r\n* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] injected\r\n"),
    ])
    .await;
    // Give the server time to send both lines in one go.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let result = connect(stream, Tls::StartTls).await;
    assert!(
        matches!(result, Err(Error::InjectedBeforeTls)),
        "{result:?}"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn oversized_literal_before_tls() {
    let (stream, server) = tcp_server(vec![
        Send("* OK ready\r\n"),
        Recv("A0001 CAPABILITY\r\n"),
        Send("* CAPABILITY {99999999999}\r\n"),
    ])
    .await;
    let result = connect(stream, Tls::StartTls).await;
    assert!(matches!(result, Err(Error::ConnectionClosed)), "{result:?}");
    server.await.unwrap();
}

#[tokio::test]
async fn implicit_tls_to_a_plaintext_server_fails() {
    let (stream, server) = tcp_server(vec![Send("* OK plaintext\r\n")]).await;
    assert!(matches!(
        connect(stream, Tls::Implicit).await,
        Err(Error::Io(_))
    ));
    server.abort();
}

#[tokio::test]
async fn search_uses_esearch_when_offered() {
    let (mut client, server) = selected(
        "IMAP4rev1 ESEARCH",
        vec![
            Recv("A0001 UID SEARCH RETURN (ALL) UNSEEN\r\n"),
            Send("* ESEARCH (TAG \"A0001\") UID ALL 2:4,9\r\nA0001 OK\r\n"),
            Recv("A0002 UID SEARCH RETURN (ALL) SEEN\r\n"),
            Send("* ESEARCH (TAG \"A0002\") UID\r\nA0002 OK\r\n"),
        ],
    );
    let found = client
        .uid_search(&nebula_imap_proto::command::SearchKey::Unseen)
        .await
        .unwrap();
    let uids: Vec<u32> = found.uids().into_iter().map(Uid::get).collect();
    assert_eq!(uids, vec![2, 3, 4, 9]);
    let none = client
        .uid_search(&nebula_imap_proto::command::SearchKey::Seen)
        .await
        .unwrap();
    assert!(none.numbers.is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn esearch_too_large_is_refused() {
    let (mut client, server) = selected(
        "IMAP4rev1 ESEARCH",
        vec![
            Recv("A0001 UID SEARCH RETURN (ALL) ALL\r\n"),
            Send("* ESEARCH (TAG \"A0001\") UID ALL 1:4000000000\r\nA0001 OK\r\n"),
        ],
    );
    let result = client
        .uid_search(&nebula_imap_proto::command::SearchKey::All)
        .await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    // The command finished: the connection is still usable.
    assert!(!client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn search_without_esearch_is_plain() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send("* SEARCH 3 5\r\nA0001 OK\r\n"),
        ],
    );
    let found = client
        .uid_search(&nebula_imap_proto::command::SearchKey::All)
        .await
        .unwrap();
    assert_eq!(found.numbers.len(), 2);
    server.await.unwrap();
}

#[tokio::test]
async fn list_returns_subscriptions() {
    let (mut client, server) = authenticated(
        "IMAP4rev1 LIST-EXTENDED SPECIAL-USE",
        vec![
            Recv("A0001 LIST \"\" \"*\" RETURN (SUBSCRIBED SPECIAL-USE)\r\n"),
            Send("* LIST (\\Subscribed) \"/\" INBOX\r\n* LIST (\\Sent) \"/\" Sent\r\nA0001 OK\r\n"),
        ],
    );
    let listed = client
        .list(&nebula_imap_proto::command::ListPattern::all(), &[])
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|m| m.subscribed).collect::<Vec<_>>(),
        vec![true, false]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn list_without_extended_uses_lsub() {
    let (mut client, server) = authenticated(
        "IMAP4rev1",
        vec![
            Recv("A0001 LIST \"\" \"*\"\r\n"),
            Send("* LIST () \"/\" INBOX\r\n* LIST () \"/\" Old\r\nA0001 OK\r\n"),
            Recv("A0002 LSUB \"\" \"*\"\r\n"),
            Send("* LSUB () \"/\" INBOX\r\n* LSUB (\\Noselect) \"/\" Gone\r\nA0002 OK\r\n"),
        ],
    );
    let listed = client
        .list(&nebula_imap_proto::command::ListPattern::all(), &[])
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|m| m.subscribed).collect::<Vec<_>>(),
        vec![true, false]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn long_uid_sets_are_split() {
    let (mut client, server) = selected(
        "IMAP4rev1 UIDPLUS MOVE",
        vec![
            Recv("A0001 UID FETCH 1,3 (FLAGS)\r\n"),
            Send("* 1 FETCH (UID 1 FLAGS ())\r\nA0001 OK\r\n"),
            Recv("A0002 UID FETCH 5,7 (FLAGS)\r\n"),
            Send("* 2 FETCH (UID 5 FLAGS ())\r\nA0002 OK\r\n"),
            Recv("A0003 UID FETCH 9 (FLAGS)\r\n"),
            Send("A0003 OK\r\n"),
            Recv("A0004 UID MOVE 1,3 Trash\r\n"),
            Send("* OK [COPYUID 7 1,3 20:21] moved\r\nA0004 OK\r\n"),
            Recv("A0005 UID MOVE 5,7 Trash\r\n"),
            Send("* OK [COPYUID 7 5,7 22:23] moved\r\nA0005 OK\r\n"),
            Recv("A0006 UID MOVE 9 Trash\r\n"),
            Send("* OK [COPYUID 7 9 24] moved\r\nA0006 OK\r\n"),
        ],
    );
    client.connection.limits.max_uid_set_len = 4;
    let uids = UidSet::from_values([1, 3, 5, 7, 9].map(|n| Uid::new(n).unwrap())).unwrap();
    let fetched = client
        .uid_fetch(&uids, &[FetchAttribute::Flags], FetchModifiers::None)
        .await
        .unwrap();
    assert_eq!(fetched.messages.len(), 2);
    let moved = client
        .uid_move(&uids, &MailboxName::new("Trash").unwrap())
        .await
        .unwrap();
    let map = moved.copy_uid.unwrap();
    assert_eq!(map.pairs.len(), 5);
    assert_eq!(map.pairs[4], (Uid::new(9).unwrap(), Uid::new(24).unwrap()));
    server.await.unwrap();
}

#[tokio::test]
async fn multiappend_and_literal8() {
    use nebula_imap_proto::command::AppendMessage;
    use nebula_imap_proto::types::Flag;
    let (mut client, server) = authenticated(
        "IMAP4rev1 MULTIAPPEND BINARY UIDPLUS",
        vec![
            Recv("A0001 APPEND Drafts (\\Draft) {3}\r\n"),
            Send("+ go\r\n"),
            Recv("one ~{3}\r\n"),
            Send("+ go\r\n"),
            Recv("t\0o\r\n"),
            Send("A0001 OK [APPENDUID 5 8:9] done\r\n"),
        ],
    );
    let messages = [
        AppendMessage {
            flags: &[Flag::Draft],
            internal_date: None,
            message: b"one",
        },
        AppendMessage {
            flags: &[],
            internal_date: None,
            message: b"t\0o",
        },
    ];
    let result = client
        .append_many(&MailboxName::new("Drafts").unwrap(), &messages)
        .await
        .unwrap();
    let (validity, uids) = result.uids.unwrap();
    assert_eq!(validity.get(), 5);
    assert_eq!(uids, vec![Uid::new(8).unwrap(), Uid::new(9).unwrap()]);
    server.await.unwrap();
}

#[tokio::test]
async fn multiappend_needs_the_capability() {
    let (mut client, _server) = authenticated("IMAP4rev1", vec![]);
    let result = client.append_many(&MailboxName::inbox(), &[]).await;
    assert!(matches!(result, Err(Error::Unsupported("MULTIAPPEND"))));
    // Without BINARY a NUL byte can't be sent at all.
    let result = client
        .append(&MailboxName::inbox(), b"a\0b", &[], None)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn binary_fetch() {
    let (mut client, server) = selected(
        "IMAP4rev1 BINARY",
        vec![
            Recv("A0001 UID FETCH 4 (UID BINARY.PEEK[2])\r\n"),
            Send("* 1 FETCH (UID 4 BINARY[2] ~{2}\r\n\0\x01)\r\nA0001 OK\r\n"),
        ],
    );
    let part =
        nebula_imap_proto::command::PartPath::new(vec![std::num::NonZeroU32::new(2).unwrap()])
            .unwrap();
    let result = client
        .uid_fetch(
            &UidSet::single(Uid::new(4).unwrap()),
            &[
                FetchAttribute::Uid,
                FetchAttribute::BinaryPeek {
                    part: Some(part),
                    partial: None,
                },
            ],
            FetchModifiers::None,
        )
        .await
        .unwrap();
    assert_eq!(
        result.messages[0].binary[0].data.as_deref(),
        Some(&[0, 1][..])
    );
    server.await.unwrap();
}

#[tokio::test]
async fn notify_during_idle() {
    use nebula_imap_proto::command::{NotifyEvent, NotifyFilter, NotifyGroup};
    let (mut client, server) = selected(
        "IMAP4rev1 NOTIFY IDLE",
        vec![
            Recv("A0001 NOTIFY SET (PERSONAL (MessageNew MessageExpunge))\r\n"),
            Send("A0001 OK\r\n"),
            Recv("A0002 IDLE\r\n"),
            Send("+ idling\r\n* STATUS Work (MESSAGES 3 UIDNEXT 9)\r\n"),
            Recv("DONE\r\n"),
            Send("A0002 OK\r\n"),
        ],
    );
    client
        .notify(&[NotifyGroup {
            filter: NotifyFilter::Personal,
            events: &[NotifyEvent::MessageNew, NotifyEvent::MessageExpunge],
        }])
        .await
        .unwrap();
    let mut idle = client.idle().await.unwrap();
    let IdleEvent::Changes(changes) = idle.wait().await.unwrap() else {
        panic!("no changes")
    };
    assert!(
        matches!(&changes[0], Untagged::MailboxStatus(status) if status.mailbox.as_str() == "Work")
    );
    idle.done().await.unwrap();
    server.await.unwrap();
}

/// A `&'static str` for the scripted server, built at run time.
fn leak(text: String) -> &'static str {
    text.leak()
}

fn search(
    client: &mut Client<Selected>,
) -> impl Future<Output = Result<nebula_imap_proto::response::SearchResult, Error>> + '_ {
    client.uid_search(&nebula_imap_proto::command::SearchKey::All)
}

#[tokio::test]
async fn command_budget_breaks_on_flood() {
    let flood = leak("* 1 EXISTS\r\n".repeat(101));
    let (mut client, server) = selected("IMAP4rev1", vec![Recv("A0001 NOOP\r\n"), Send(flood)]);
    client.connection.limits.max_command_responses = 100;
    let result = client.noop().await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    assert!(client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn command_budget_bytes() {
    let body = "x".repeat(2000);
    let answer = leak(format!(
        "* 1 FETCH (UID 1 BODY[] {{{}}}\r\n{body})\r\nA0001 OK\r\n",
        body.len()
    ));
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID FETCH 1 (UID BODY.PEEK[])\r\n"),
            Send(answer),
        ],
    );
    client.connection.limits.max_command_bytes = 1000;
    let result = client
        .uid_fetch(
            &UidSet::single(Uid::new(1).unwrap()),
            &[
                FetchAttribute::Uid,
                FetchAttribute::BodyPeek {
                    section: nebula_imap_proto::command::Section::Full,
                    partial: None,
                },
            ],
            FetchModifiers::None,
        )
        .await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    assert!(client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn budget_resets_per_command() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 NOOP\r\n"),
            Send("* 1 EXISTS\r\n* 2 EXISTS\r\n* 3 EXISTS\r\nA0001 OK\r\n"),
            Recv("A0002 NOOP\r\n"),
            Send("* 4 EXISTS\r\n* 5 EXISTS\r\n* 6 EXISTS\r\nA0002 OK\r\n"),
        ],
    );
    client.connection.limits.max_command_responses = 4;
    client.noop().await.unwrap();
    client.noop().await.unwrap();
    assert!(!client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn events_overflow_keeps_command_ok() {
    let answer = leak(format!(
        "{}A0001 OK moved\r\n",
        "* 1 EXPUNGE\r\n".repeat(20)
    ));
    let (mut client, server) = selected(
        "IMAP4rev1 MOVE",
        vec![Recv("A0001 UID MOVE 1:20 Archive\r\n"), Send(answer)],
    );
    client.session.max_events = 5;
    client.state.mailbox.exists = 30;
    client
        .uid_move(
            &UidSet::range(Uid::new(1).unwrap(), Uid::new(20).unwrap()),
            &MailboxName::new("Archive").unwrap(),
        )
        .await
        .unwrap();
    assert!(!client.is_broken());
    // The counters still saw every EXPUNGE.
    assert_eq!(client.mailbox().exists, 10);
    let events = client.take_events();
    assert!(events.overflowed);
    assert!(events.items.is_empty());
    // Taking them resets the flag.
    assert!(!client.take_events().overflowed);
    server.await.unwrap();
}

#[tokio::test]
async fn store_keeps_only_the_uids_asked() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID STORE 4 +FLAGS (\\Seen)\r\n"),
            Send("* 1 FETCH (UID 4 FLAGS (\\Seen))\r\n* 2 FETCH (UID 9 FLAGS ())\r\nA0001 OK\r\n"),
        ],
    );
    let result = client
        .uid_store(
            &UidSet::single(Uid::new(4).unwrap()),
            nebula_imap_proto::command::StoreOperation::Add,
            &[nebula_imap_proto::types::Flag::Seen],
            nebula_imap_proto::command::StoreResponse::Fetch,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.fetched.len(), 1);
    assert_eq!(result.fetched[0].uid.map(Uid::get), Some(4));
    assert_eq!(client.take_events().items.len(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn select_without_qresync_keeps_fetch_as_event() {
    let (client, server) = authenticated(
        "IMAP4rev1 CONDSTORE",
        vec![
            Recv("A0001 SELECT INBOX (CONDSTORE)\r\n"),
            Send(
                "* 3 EXISTS\r\n\
                 * OK [UIDVALIDITY 7] ok\r\n\
                 * 1 FETCH (UID 2 FLAGS (\\Seen))\r\n\
                 * VANISHED (EARLIER) 1\r\n\
                 A0001 OK [READ-WRITE] done\r\n",
            ),
        ],
    );
    let (mut selected, result) = client
        .select(&MailboxName::inbox(), SelectOptions::default())
        .await
        .unwrap();
    assert!(result.changed.is_empty());
    assert!(result.vanished.is_empty());
    assert_eq!(selected.take_events().items.len(), 2);
    server.await.unwrap();
}

#[tokio::test]
async fn select_without_exists_is_fatal() {
    let (client, server) = authenticated(
        "IMAP4rev1",
        vec![
            Recv("A0001 SELECT INBOX\r\n"),
            Send("* OK [UIDVALIDITY 7] ok\r\nA0001 OK [READ-WRITE] done\r\n"),
        ],
    );
    let options = SelectOptions {
        access: Access::ReadWrite,
        resync: Resync::Plain,
    };
    let error = client
        .select(&MailboxName::inbox(), options)
        .await
        .unwrap_err();
    assert!(
        matches!(error.error(), Error::Protocol(_)),
        "{:?}",
        error.error()
    );
    assert!(!error.has_client());
    server.await.unwrap();
}

#[tokio::test]
async fn list_matches_status_by_name() {
    let (mut client, server) = authenticated(
        "IMAP4rev1 LIST-EXTENDED LIST-STATUS",
        vec![
            Recv("A0001 LIST \"\" \"*\" RETURN (SUBSCRIBED STATUS (MESSAGES))\r\n"),
            Send(
                "* STATUS Work (MESSAGES 3)\r\n\
                 * LIST () \"/\" INBOX\r\n\
                 * LIST () \"/\" Work\r\n\
                 * STATUS INBOX (MESSAGES 9)\r\n\
                 * STATUS Ghost (MESSAGES 1)\r\n\
                 A0001 OK\r\n",
            ),
        ],
    );
    let listed = client
        .list(
            &nebula_imap_proto::command::ListPattern::all(),
            &[nebula_imap_proto::command::StatusItem::Messages],
        )
        .await
        .unwrap();
    let messages: Vec<_> = listed
        .iter()
        .map(|m| m.status.as_ref().and_then(|status| status.messages))
        .collect();
    assert_eq!(messages, vec![Some(9), Some(3)]);
    server.await.unwrap();
}

#[tokio::test]
async fn plain_search_too_large_is_refused() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send("* SEARCH 1 2\r\n* SEARCH 3 4\r\nA0001 OK\r\n"),
        ],
    );
    client.connection.limits.max_search_results = 3;
    let result = search(&mut client).await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    assert!(!client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn search_ok_without_data_is_an_error() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![Recv("A0001 UID SEARCH ALL\r\n"), Send("A0001 OK\r\n")],
    );
    let result = search(&mut client).await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    assert!(!client.is_broken());
    server.await.unwrap();
}

#[tokio::test]
async fn search_empty_plain_is_ok() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send("* SEARCH\r\nA0001 OK\r\n"),
            Recv("A0002 UID SEARCH ALL\r\n"),
            Send("* SEARCH \r\nA0002 OK\r\n"),
            Recv("A0003 UID SEARCH ALL\r\n"),
            Send("* SEARCH 1  2 \r\nA0003 OK\r\n"),
        ],
    );
    assert!(search(&mut client).await.unwrap().numbers.is_empty());
    assert!(search(&mut client).await.unwrap().numbers.is_empty());
    assert_eq!(search(&mut client).await.unwrap().numbers.len(), 2);
    server.await.unwrap();
}

#[tokio::test]
async fn imap4rev2_only_uses_return_all() {
    let (mut client, server) = selected(
        "IMAP4rev2",
        vec![
            Recv("A0001 UID SEARCH RETURN (ALL) ALL\r\n"),
            Send("* ESEARCH (TAG \"A0001\") UID ALL 1:3\r\nA0001 OK\r\n"),
        ],
    );
    assert_eq!(search(&mut client).await.unwrap().numbers.len(), 3);
    server.await.unwrap();
    // With both revisions (no ENABLE IMAP4rev2) it stays plain, but an
    // ESEARCH answer is still read.
    let (mut client, server) = selected(
        "IMAP4rev1 IMAP4rev2",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send("* ESEARCH (TAG \"A0001\") UID ALL 4\r\nA0001 OK\r\n"),
        ],
    );
    assert_eq!(search(&mut client).await.unwrap().numbers.len(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn esearch_without_uid_is_refused() {
    let (mut client, server) = selected(
        "IMAP4rev1 ESEARCH",
        vec![
            Recv("A0001 UID SEARCH RETURN (ALL) ALL\r\n"),
            Send("* ESEARCH (TAG \"A0001\") ALL 1:3\r\nA0001 OK\r\n"),
        ],
    );
    let result = search(&mut client).await;
    assert!(matches!(result, Err(Error::Protocol(_))), "{result:?}");
    server.await.unwrap();
}

#[tokio::test]
async fn search_with_unknown_response_is_an_error() {
    for answer in [
        "* ESEARCH (TAG \"A0001\") UID ALL 1:*\r\nA0001 OK\r\n",
        "* ESEARCH (TAG \"A0001\") UID ALL 0\r\nA0001 OK\r\n",
        "* SEARCH 1 x\r\nA0001 OK\r\n",
    ] {
        let (mut client, server) = selected(
            "IMAP4rev1 ESEARCH",
            vec![Recv("A0001 UID SEARCH RETURN (ALL) ALL\r\n"), Send(answer)],
        );
        let result = search(&mut client).await;
        assert!(
            matches!(result, Err(Error::Protocol(_))),
            "{answer}: {result:?}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn search_ignores_unrelated_unknown() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send(
                "* METADATA \"INBOX\" (/shared/comment \"x\")\r\n* 99 BOGUS\r\n* SEARCH 1\r\nA0001 OK\r\n",
            ),
        ],
    );
    let found = search(&mut client).await.unwrap();
    assert_eq!(found.numbers.len(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_counts_unreadable() {
    let (mut client, server) = selected(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID FETCH 1:* (UID FLAGS)\r\n"),
            Send(
                "* 1 FETCH (UID 2 FLAGS ())\r\n* 2 FETCH (UID 0 FLAGS ())\r\n* 99 BOGUS\r\nA0001 OK\r\n",
            ),
        ],
    );
    let result = client
        .uid_fetch(
            &UidSet::all(),
            &[FetchAttribute::Uid, FetchAttribute::Flags],
            FetchModifiers::None,
        )
        .await
        .unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.unreadable, 1);
    server.await.unwrap();
}

#[tokio::test]
async fn dropped_append_breaks_the_connection() {
    // The server never reads: the 200 KB literal fills the in-memory pipe
    // and the write blocks until the future is dropped.
    let (mut client, server) = authenticated("IMAP4rev1 LITERAL+", vec![]);
    let message = vec![b'x'; 200_000];
    let inbox = MailboxName::inbox();
    let append = client.append(&inbox, &message, &[], None);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), append)
            .await
            .is_err()
    );
    assert!(client.is_broken());
    assert!(matches!(client.noop().await, Err(Error::Broken)));
    server.await.unwrap();
}

#[tokio::test]
async fn dropped_while_waiting_tagged() {
    let (mut client, server) = selected("IMAP4rev1", vec![Recv("A0001 NOOP\r\n")]);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), client.noop())
            .await
            .is_err()
    );
    assert!(client.is_broken());
    assert!(matches!(client.noop().await, Err(Error::Broken)));
    server.await.unwrap();
}

#[tokio::test]
async fn idle_done_clears_in_flight() {
    let (client, server) = selected(
        "IMAP4rev1 IDLE",
        vec![
            Recv("A0001 IDLE\r\n"),
            Send("+ idling\r\n"),
            Recv("DONE\r\n"),
            Send("A0001 OK\r\n"),
            Recv("A0002 NOOP\r\n"),
            Send("A0002 OK\r\n"),
        ],
    );
    let idle = client.idle().await.unwrap();
    let mut client = idle.done().await.unwrap();
    assert!(!client.is_broken());
    client.noop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn idle_reports_data_sent_before_continuation() {
    let (client, server) = selected(
        "IMAP4rev1 IDLE",
        vec![
            Recv("A0001 IDLE\r\n"),
            Send("* 5 EXISTS\r\n* OK still here\r\n+ idling\r\n"),
            Recv("DONE\r\n"),
            Send("A0001 OK\r\n"),
        ],
    );
    let mut idle = client.idle().await.unwrap();
    // Returned at once, without waiting for more data.
    let event = tokio::time::timeout(Duration::from_millis(500), idle.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event, IdleEvent::Changes(vec![Untagged::Exists(5)]));
    let client = idle.done().await.unwrap();
    assert_eq!(client.mailbox().exists, 5);
    server.await.unwrap();
}

async fn login_answer(answer: &'static str) -> Error {
    let (client, server) = client(
        "IMAP4rev1",
        vec![Recv("A0001 LOGIN alice secret\r\n"), Send(answer)],
    );
    let error = client.login(&credentials()).await.unwrap_err();
    server.await.unwrap();
    error.into_parts().0
}

#[tokio::test]
async fn login_no_with_alert_is_auth_failed() {
    for answer in [
        "A0001 NO [ALERT] Your account is locked\r\n",
        "A0001 NO [CONTACTADMIN] ask\r\n",
        "A0001 NO [PRIVACYREQUIRED] tls\r\n",
    ] {
        let error = login_answer(answer).await;
        assert!(
            matches!(error, Error::AuthFailed { .. }),
            "{answer}: {error:?}"
        );
    }
}

#[tokio::test]
async fn login_no_unavailable_is_refused() {
    for answer in [
        "A0001 NO [UNAVAILABLE] try later\r\n",
        "A0001 NO [SERVERBUG] oops\r\n",
        "A0001 NO [INUSE] busy\r\n",
        "A0001 NO [LIMIT] too many\r\n",
        "A0001 BAD what\r\n",
    ] {
        let error = login_answer(answer).await;
        assert!(matches!(error, Error::Refused(_)), "{answer}: {error:?}");
    }
}

#[tokio::test]
async fn login_no_vendor_code_is_auth_failed() {
    let error = login_answer("A0001 NO [XYZZY-LOCKED] vendor\r\n").await;
    assert!(matches!(error, Error::AuthFailed { .. }), "{error:?}");
}

#[tokio::test]
async fn a_refused_literal_leaves_the_connection_usable() {
    let (mut client, server) = authenticated(
        "IMAP4rev1",
        vec![
            Recv("A0001 APPEND INBOX {3}\r\n"),
            Send("A0001 NO [TOOBIG] too big\r\n"),
            Recv("A0002 NOOP\r\n"),
            Send("A0002 OK\r\n"),
        ],
    );
    let result = client
        .append(&MailboxName::inbox(), b"one", &[], None)
        .await;
    assert!(matches!(result, Err(Error::Refused(_))), "{result:?}");
    assert!(!client.is_broken());
    client.noop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_skips_a_response_over_the_limits() {
    let mut huge = String::from("* 1 FETCH (UID 7 BODYSTRUCTURE (");
    while huge.len() < 200 * 1024 {
        huge.push_str("(\"text\" \"plain\" NIL NIL NIL \"7bit\" 0 0)");
    }
    huge.push_str(" \"mixed\"))\r\n* 2 FETCH (UID 8 FLAGS ())\r\nA0001 OK\r\n");
    let (mut client, server) = selected_with(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID FETCH 7:8 (UID FLAGS)\r\n"),
            Send(huge.leak()),
            Recv("A0002 NOOP\r\n"),
            Send("A0002 OK\r\n"),
        ],
        small_frames(),
    );
    let result = client
        .uid_fetch(
            &UidSet::range(Uid::new(7).unwrap(), Uid::new(8).unwrap()),
            &[FetchAttribute::Uid, FetchAttribute::Flags],
            FetchModifiers::None,
        )
        .await
        .unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].uid, Some(Uid::new(8).unwrap()));
    client.noop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn search_over_the_limits_fails_without_breaking() {
    let (mut client, server) = selected_with(
        "IMAP4rev1",
        vec![
            Recv("A0001 UID SEARCH ALL\r\n"),
            Send(long_line("* SEARCH", " 4000000000", 4096)),
            Send("A0001 OK\r\n"),
            Recv("A0002 NOOP\r\n"),
            Send("A0002 OK\r\n"),
        ],
        small_frames(),
    );
    let result = client
        .uid_search(&nebula_imap_proto::command::SearchKey::All)
        .await;
    let Err(error) = result else {
        panic!("{result:?}")
    };
    assert!(
        matches!(error, Error::ResponseTooLarge("UID SEARCH")),
        "{error:?}"
    );
    assert!(!error.is_fatal());
    assert!(!client.is_broken());
    client.noop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn select_with_dropped_data_drops_the_connection() {
    let (client, server) = client_with(
        "IMAP4rev1",
        vec![
            Recv("A0001 SELECT INBOX\r\n"),
            Send(long_line("* FLAGS (", " $Keyword", 4096)),
            Send("* OK [UIDVALIDITY 1] ok\r\nA0001 OK [READ-WRITE] done\r\n"),
        ],
        small_frames(),
    );
    let error = client
        .into_state(Authenticated)
        .select(&MailboxName::inbox(), SelectOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error.error(), Error::ResponseTooLarge(_)));
    // The server has INBOX selected: going back to Authenticated would
    // leave both sides in different states.
    assert!(!error.has_client());
    server.await.unwrap();
}

#[tokio::test]
async fn login_with_dropped_data_drops_the_connection() {
    let (client, server) = client_with(
        "IMAP4rev1",
        vec![
            Recv("A0001 LOGIN alice secret\r\n"),
            Send(long_line("* CAPABILITY IMAP4rev1", " X-FILLER", 4096)),
            Send("A0001 OK done\r\n"),
        ],
        small_frames(),
    );
    let error = client.login(&credentials()).await.unwrap_err();
    assert!(matches!(error.error(), Error::ResponseTooLarge(_)));
    assert!(!error.has_client());
    server.await.unwrap();
}

#[test]
fn permanent_flags_can_store() {
    use crate::PermanentFlags;
    use nebula_imap_proto::response::NewKeywords;
    use nebula_imap_proto::types::Flag;
    let flag = |text| Flag::parse(text).unwrap();
    let listed = |flags, new_keywords| PermanentFlags::Listed {
        flags,
        new_keywords,
    };
    // `()`: nothing can be stored.
    let none = listed(Vec::new(), NewKeywords::NotAllowed);
    assert!(!none.can_store(&flag("$Forwarded")));
    assert!(!none.can_store(&Flag::Seen));
    // No PERMANENTFLAGS: anything can.
    assert!(PermanentFlags::NotAnnounced.can_store(&flag("$Forwarded")));
    assert!(PermanentFlags::NotAnnounced.can_store(&Flag::Deleted));
    // Keywords compare without case.
    let lower = listed(vec![flag("$forwarded")], NewKeywords::NotAllowed);
    assert!(lower.can_store(&flag("$Forwarded")));
    assert!(!lower.can_store(&flag("$Junk")));
    let some = listed(vec![Flag::Seen, Flag::Flagged], NewKeywords::NotAllowed);
    assert!(some.can_store(&Flag::Seen));
    assert!(!some.can_store(&Flag::Draft));
    assert!(!some.can_store(&flag("work")));
    let open = listed(vec![Flag::Seen], NewKeywords::Allowed);
    assert!(open.can_store(&flag("work")));
    assert!(!open.can_store(&Flag::Draft));
}

#[tokio::test]
async fn select_tells_empty_permanentflags_from_none() {
    let (client, server) = authenticated(
        "IMAP4rev1",
        vec![
            Recv("A0001 SELECT INBOX\r\n"),
            Send(
                "* 0 EXISTS\r\n* OK [PERMANENTFLAGS ()] nothing\r\n* OK [UIDVALIDITY 1] ok\r\nA0001 OK done\r\n",
            ),
            Recv("A0002 SELECT Other\r\n"),
            Send("* 0 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\nA0002 OK done\r\n"),
        ],
    );
    let (client, _) = client
        .select(&MailboxName::inbox(), SelectOptions::default())
        .await
        .unwrap();
    assert_eq!(
        client.mailbox().permanent_flags,
        crate::PermanentFlags::Listed {
            flags: Vec::new(),
            new_keywords: nebula_imap_proto::response::NewKeywords::NotAllowed,
        }
    );
    let (client, _) = client
        .select(
            &MailboxName::new("Other").unwrap(),
            SelectOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        client.mailbox().permanent_flags,
        crate::PermanentFlags::NotAnnounced
    );
    server.await.unwrap();
}

/// RFC 9051 §6.1.1, §7.2.2, Appendix E: a server that only announces
/// `IMAP4rev2` has its base extensions (SASL-IR, ENABLE, LIST-EXTENDED,
/// ESEARCH, MOVE, UIDPLUS, UNSELECT, IDLE…) without listing them, and
/// mailbox names are UTF-8 (§5.1).
#[tokio::test]
async fn rev2_only_login_uses_sasl_ir_and_utf8_names() {
    let (client, server) = client(
        "IMAP4rev2 AUTH=PLAIN",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev2 AUTH=PLAIN CONDSTORE] Logged in\r\n"),
            Recv("A0002 ENABLE CONDSTORE\r\n"),
            Send("* ENABLED CONDSTORE\r\nA0002 OK\r\n"),
        ],
    );
    let client = client.login(&credentials()).await.unwrap();
    assert_eq!(
        client.name_encoding(),
        nebula_imap_proto::types::NameEncoding::Utf8
    );
    server.await.unwrap();
}

#[tokio::test]
async fn rev2_only_list_is_extended_with_utf8_names_and_no_lsub() {
    let (client, server) = client(
        "IMAP4rev2 AUTH=PLAIN",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev2 AUTH=PLAIN] Logged in\r\n"),
            Recv("A0002 LIST \"\" \"*\" RETURN (SUBSCRIBED STATUS (MESSAGES))\r\n"),
            Send(
                "* LIST (\\Subscribed) \"/\" INBOX\r\n\
                 * LIST (\\Sent) \"/\" \"Enviados é\"\r\n\
                 * STATUS \"Enviados é\" (MESSAGES 4)\r\nA0002 OK\r\n",
            ),
            Recv("A0003 CREATE \"Año/Café\"\r\n"),
            Send("A0003 OK\r\n"),
        ],
    );
    let mut client = client.login(&credentials()).await.unwrap();
    let listed = client
        .list(
            &nebula_imap_proto::command::ListPattern::all(),
            &[nebula_imap_proto::command::StatusItem::Messages],
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].entry.name.as_str(), "Enviados é");
    assert!(listed[1].status.is_some());
    assert_eq!(
        listed.iter().map(|m| m.subscribed).collect::<Vec<_>>(),
        vec![true, false]
    );
    client
        .create(&MailboxName::new("Año/Café").unwrap(), None)
        .await
        .unwrap();
    // RFC 9051 has no LSUB (Appendix E item 19).
    let lsub = client
        .lsub(&nebula_imap_proto::command::ListPattern::all())
        .await;
    assert!(matches!(lsub, Err(Error::Unsupported("LSUB"))), "{lsub:?}");
    server.await.unwrap();
}

#[tokio::test]
async fn rev2_only_selected_uses_move_esearch_idle_and_unselect() {
    let (mut client, server) = selected(
        "IMAP4rev2",
        vec![
            Recv("A0001 UID MOVE 5 \"Archivé\"\r\n"),
            Send("* OK [COPYUID 7 5 12] Moved\r\n* 2 EXPUNGE\r\nA0001 OK\r\n"),
            Recv("A0002 UID SEARCH RETURN (ALL) UNSEEN\r\n"),
            Send("* ESEARCH (TAG \"A0002\") UID ALL 2:3\r\nA0002 OK\r\n"),
            Recv("A0003 IDLE\r\n"),
            Send("+ idling\r\n* 3 EXISTS\r\n"),
            Recv("DONE\r\n"),
            Send("A0003 OK\r\n"),
            Recv("A0004 UNSELECT\r\n"),
            Send("A0004 OK\r\n"),
        ],
    );
    client.session.names = nebula_imap_proto::types::NameEncoding::Utf8;
    let moved = client
        .uid_move(
            &UidSet::single(Uid::new(5).unwrap()),
            &MailboxName::new("Archivé").unwrap(),
        )
        .await
        .unwrap();
    assert!(moved.copy_uid.is_some());
    let found = client
        .uid_search(&nebula_imap_proto::command::SearchKey::Unseen)
        .await
        .unwrap();
    assert_eq!(found.numbers.len(), 2);
    let mut idle = client.idle().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), idle.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(event, IdleEvent::Changes(_)), "{event:?}");
    let client = idle.done().await.unwrap();
    client.unselect().await.unwrap();
    server.await.unwrap();
}

/// A server with both revisions behaves as `IMAP4rev1` (RFC 9051 §6.1.1):
/// only what it lists counts, and names stay modified UTF-7.
#[tokio::test]
async fn rev1_and_rev2_stays_rev1() {
    let (client, server) = client(
        "IMAP4rev1 IMAP4rev2 AUTH=PLAIN",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN\r\n"),
            Send("+ \r\n"),
            Recv("AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev1 IMAP4rev2 AUTH=PLAIN] Logged in\r\n"),
            Recv("A0002 LIST \"\" \"*\"\r\n"),
            Send("* LIST () \"/\" \"Enviados &AOk-\"\r\nA0002 OK\r\n"),
            Recv("A0003 LSUB \"\" \"*\"\r\n"),
            Send("* LSUB () \"/\" \"Enviados &AOk-\"\r\nA0003 OK\r\n"),
        ],
    );
    let mut client = client.login(&credentials()).await.unwrap();
    assert_eq!(
        client.name_encoding(),
        nebula_imap_proto::types::NameEncoding::ModifiedUtf7
    );
    let listed = client
        .list(&nebula_imap_proto::command::ListPattern::all(), &[])
        .await
        .unwrap();
    assert_eq!(listed[0].entry.name.as_str(), "Enviados é");
    assert!(listed[0].subscribed);
    server.await.unwrap();
}

/// RFC 7628 §3.2.3: the OAUTHBEARER error challenge is answered with
/// `%x01` (`AQ==`), and the server's NO counts as refused credentials.
#[tokio::test]
async fn oauthbearer_error_challenge_is_acknowledged_with_x01() {
    let (client, server) = client(
        "IMAP4rev1 AUTH=OAUTHBEARER SASL-IR",
        vec![
            Recv("A0001 AUTHENTICATE OAUTHBEARER bixhPWFsaWNlLAFhdXRoPUJlYXJlciB0b2sBAQ==\r\n"),
            Send("+ eyJzdGF0dXMiOiJpbnZhbGlkX3Rva2VuIn0=\r\n"),
            Recv("AQ==\r\n"),
            Send("A0001 NO [AUTHENTICATIONFAILED] Invalid token\r\n"),
        ],
    );
    let error = client
        .login_oauth(
            "alice",
            &SecretString::from("tok"),
            crate::OAuthMechanism::OAuthBearer,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error.error(), Error::AuthFailed { .. }),
        "{:?}",
        error.error()
    );
    server.await.unwrap();
}

/// XOAUTH2 has no RFC: its error challenge is cancelled with `*`
/// (RFC 3501 §6.2.2).
#[tokio::test]
async fn xoauth2_error_challenge_is_cancelled() {
    let (client, server) = client(
        "IMAP4rev1 AUTH=XOAUTH2 SASL-IR",
        vec![
            Recv("A0001 AUTHENTICATE XOAUTH2 dXNlcj1hbGljZQFhdXRoPUJlYXJlciB0b2sBAQ==\r\n"),
            Send("+ eyJzdGF0dXMiOiI0MDEifQ==\r\n"),
            Recv("*\r\n"),
            Send("A0001 NO [AUTHENTICATIONFAILED] Invalid token\r\n"),
        ],
    );
    let error = client
        .login_oauth(
            "alice",
            &SecretString::from("tok"),
            crate::OAuthMechanism::XOAuth2,
        )
        .await
        .unwrap_err();
    assert!(matches!(error.error(), Error::AuthFailed { .. }));
    server.await.unwrap();
}

/// RFC 6855 §4: after `ENABLE UTF8=ACCEPT`, a message with UTF-8 header
/// fields is appended with the `UTF8 (~{n})` data extension.
#[tokio::test]
async fn append_utf8_headers_after_enable() {
    let (client, server) = client(
        "IMAP4rev1 AUTH=PLAIN SASL-IR ENABLE UTF8=ACCEPT",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [CAPABILITY IMAP4rev1 ENABLE UTF8=ACCEPT] Logged in\r\n"),
            Recv("A0002 ENABLE UTF8=ACCEPT\r\n"),
            Send("* ENABLED UTF8=ACCEPT\r\nA0002 OK\r\n"),
            Recv("A0003 APPEND Enviados UTF8 (~{19}\r\n"),
            Send("+ go\r\n"),
            Recv("Subject: é\r\n\r\nhola)\r\n"),
            Send("A0003 OK [APPENDUID 3 7] done\r\n"),
        ],
    );
    let mut client = client.login(&credentials()).await.unwrap();
    let result = client
        .append(
            &MailboxName::new("Enviados").unwrap(),
            "Subject: é\r\n\r\nhola".as_bytes(),
            &[],
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.uid.map(|(_, uid)| uid.get()), Some(7));
    server.await.unwrap();
}

#[derive(Default)]
struct CollectAlerts(std::sync::Mutex<Vec<String>>);

impl crate::AlertSink for CollectAlerts {
    fn alert(&self, alert: crate::ServerAlert) {
        self.0.lock().unwrap().push(alert.as_str().to_owned());
    }
}

/// RFC 3501 §7.1 / RFC 9051 §7.1: `[ALERT]` text MUST reach the user. It
/// goes to the sink from tagged and untagged responses, during commands
/// and IDLE, cleaned.
#[tokio::test]
async fn alerts_go_to_the_sink() {
    let (mut client, server) = client(
        "IMAP4rev1 AUTH=PLAIN SASL-IR IDLE",
        vec![
            Recv("A0001 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"),
            Send("A0001 OK [ALERT] Password expires\x1b[31m in 3 days\r\n"),
            Recv("A0002 CAPABILITY\r\n"),
            Send(
                "* CAPABILITY IMAP4rev1 IDLE\r\n* OK [ALERT] Maintenance at 22:00\r\nA0002 OK\r\n",
            ),
            Recv("A0003 SELECT INBOX\r\n"),
            Send(
                "* 1 EXISTS\r\n* OK [UIDVALIDITY 1] x\r\n* OK [ALERT] Mailbox 95% full\r\n\
                 A0003 OK [READ-WRITE] done\r\n",
            ),
            Recv("A0004 IDLE\r\n"),
            Send("+ idling\r\n* OK [ALERT] Going down now\r\n"),
            Recv("DONE\r\n"),
            Send("A0004 OK\r\n"),
        ],
    );
    let sink = std::sync::Arc::new(CollectAlerts::default());
    client.set_alert_sink(sink.clone());
    let client = client.login(&credentials()).await.unwrap();
    let (client, _) = client
        .select(&MailboxName::inbox(), SelectOptions::default())
        .await
        .unwrap();
    let mut idle = client.idle().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), idle.wait())
        .await
        .unwrap()
        .unwrap();
    idle.done().await.unwrap();
    assert_eq!(
        *sink.0.lock().unwrap(),
        [
            "Password expires [31m in 3 days",
            "Maintenance at 22:00",
            "Mailbox 95% full",
            "Going down now",
        ]
    );
    server.await.unwrap();
}

/// A `NO [ALERT]` to LOGIN is still a refused login, and its text reaches
/// the sink too.
#[tokio::test]
async fn login_no_alert_reaches_the_sink() {
    let (mut client, server) = client(
        "IMAP4rev1",
        vec![
            Recv("A0001 LOGIN alice secret\r\n"),
            Send("A0001 NO [ALERT] Account locked: call the help desk\r\n"),
        ],
    );
    let sink = std::sync::Arc::new(CollectAlerts::default());
    client.set_alert_sink(sink.clone());
    let error = client.login(&credentials()).await.unwrap_err();
    assert!(matches!(error.error(), Error::AuthFailed { .. }));
    assert_eq!(
        *sink.0.lock().unwrap(),
        ["Account locked: call the help desk"]
    );
    server.await.unwrap();
}
