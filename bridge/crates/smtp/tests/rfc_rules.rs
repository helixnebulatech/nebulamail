// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What the standards ask of the client, against a fake server over
//! implicit TLS that records every command: `SIZE` (RFC 1870), `SMTPUTF8`
//! for UTF-8 header fields (RFC 6531 §3.4), a `552` to `RCPT` taken as
//! `452` (RFC 5321 §4.5.3.1.10) and `QUIT` before closing (§4.1.1.10).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod common;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nebula_smtp::{
    CertificateCheck, Credentials, Envelope, OutgoingMessage, Security, SmtpClient, SmtpError,
    SmtpServer, Timeouts,
};
use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// How the fake server behaves.
#[derive(Clone, Copy, Default)]
struct Fake {
    /// `SIZE` with this maximum in `EHLO`.
    size: Option<usize>,
    /// Refuse `AUTH` with 535.
    refuse_auth: bool,
    /// Answer `552` to every `RCPT`.
    rcpt_552: bool,
}

/// Starts the fake server; the commands it gets (one per line, `DATA`
/// content left out) end up in the returned list.
async fn server(fake: Fake) -> (TcpStream, Arc<Mutex<Vec<String>>>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let tls = common::acceptor().accept(socket).await.unwrap();
        let mut stream = BufReader::new(tls);
        let _ = stream.get_mut().write_all(b"220 fake ESMTP\r\n").await;
        let mut in_data = false;
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    let _ = stream.get_mut().write_all(b"250 queued\r\n").await;
                }
                continue;
            }
            log.lock().unwrap().push(line.trim_end().to_owned());
            let upper = line.to_ascii_uppercase();
            let reply: Vec<u8> = if upper.starts_with("EHLO") {
                let mut reply = "250-fake\r\n250-8BITMIME\r\n".to_owned();
                if let Some(size) = fake.size {
                    let _ = write!(reply, "250-SIZE {size}\r\n");
                }
                reply.push_str("250 AUTH PLAIN\r\n");
                reply.into_bytes()
            } else if upper.starts_with("AUTH") {
                if fake.refuse_auth {
                    b"535 5.7.8 bad credentials\r\n".to_vec()
                } else {
                    b"235 ok\r\n".to_vec()
                }
            } else if upper.starts_with("RCPT") && fake.rcpt_552 {
                b"552 too many recipients\r\n".to_vec()
            } else if upper.starts_with("DATA") {
                in_data = true;
                b"354 go\r\n".to_vec()
            } else if upper.starts_with("QUIT") {
                let _ = stream.get_mut().write_all(b"221 bye\r\n").await;
                return;
            } else {
                b"250 ok\r\n".to_vec()
            };
            if stream.get_mut().write_all(&reply).await.is_err() {
                return;
            }
        }
    });
    (TcpStream::connect(address).await.unwrap(), seen, task)
}

fn client() -> SmtpClient {
    let server = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_timeouts(Timeouts {
            command: Duration::from_secs(5),
            data_start: Duration::from_secs(5),
            data_block: Duration::from_secs(5),
            data_end: Duration::from_secs(5),
        });
    SmtpClient::new(server).unwrap()
}

fn envelope() -> Envelope {
    Envelope::new(
        "alice@example.com".parse().unwrap(),
        vec!["bob@example.com".parse().unwrap()],
    )
    .unwrap()
}

async fn send(stream: TcpStream, message: &OutgoingMessage) -> Result<(), SmtpError> {
    let credentials = Credentials::new("alice", SecretString::from("secret")).unwrap();
    let mut session = client()
        .connect(stream)
        .await?
        .authenticate(&credentials)
        .await?;
    let result = session.send(&envelope(), message).await.map(|_| ());
    session.quit().await;
    result
}

fn commands(seen: &Mutex<Vec<String>>) -> Vec<String> {
    seen.lock().unwrap().clone()
}

#[tokio::test]
async fn size_is_announced_and_a_bigger_message_is_never_uploaded() {
    let (stream, seen, task) = server(Fake {
        size: Some(1000),
        ..Fake::default()
    })
    .await;
    let small = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    send(stream, &small).await.unwrap();
    task.await.unwrap();
    let mail = commands(&seen)
        .into_iter()
        .find(|line| line.starts_with("MAIL FROM"))
        .unwrap();
    assert!(
        mail.contains(&format!("SIZE={}", small.as_bytes().len())),
        "{mail}"
    );

    let (stream, seen, task) = server(Fake {
        size: Some(1000),
        ..Fake::default()
    })
    .await;
    let big =
        OutgoingMessage::new(format!("Subject: big\r\n\r\n{}\r\n", "x".repeat(2000)).as_bytes())
            .unwrap();
    let error = send(stream, &big).await.unwrap_err();
    assert_eq!(error, SmtpError::TooLargeForServer { maximum: 1000 });
    assert!(error.is_too_large());
    task.await.unwrap();
    let seen = commands(&seen);
    assert!(
        !seen
            .iter()
            .any(|line| line.starts_with("MAIL") || line == "DATA"),
        "{seen:?}"
    );
    assert_eq!(seen.last().map(String::as_str), Some("QUIT"));
}

#[tokio::test]
async fn utf8_header_fields_need_smtputf8() {
    let (stream, seen, task) = server(Fake::default()).await;
    let message = OutgoingMessage::new("Subject: Ñandú\r\n\r\nhola\r\n".as_bytes()).unwrap();
    let error = send(stream, &message).await.unwrap_err();
    assert_eq!(error, SmtpError::SmtpUtf8HeadersUnsupported);
    task.await.unwrap();
    // Refused before anything of the transaction went out.
    assert!(!commands(&seen).iter().any(|line| line.starts_with("MAIL")));
}

#[tokio::test]
async fn a_552_to_rcpt_is_the_temporary_452() {
    let (stream, _seen, task) = server(Fake {
        rcpt_552: true,
        ..Fake::default()
    })
    .await;
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    let error = send(stream, &message).await.unwrap_err();
    let SmtpError::RecipientsRejected { rejected } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(rejected[0].code.get(), 452);
    assert!(error.is_transient());
    task.await.unwrap();
}

#[tokio::test]
async fn a_refused_login_still_says_quit() {
    let (stream, seen, task) = server(Fake {
        refuse_auth: true,
        ..Fake::default()
    })
    .await;
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    let error = send(stream, &message).await.unwrap_err();
    assert!(matches!(error, SmtpError::Auth { .. }), "{error:?}");
    task.await.unwrap();
    assert_eq!(commands(&seen).last().map(String::as_str), Some("QUIT"));
}

#[tokio::test]
async fn a_session_opened_only_for_its_extensions_says_quit() {
    let (stream, seen, task) = server(Fake::default()).await;
    let session = client().connect(stream).await.unwrap();
    session.quit().await;
    task.await.unwrap();
    assert_eq!(commands(&seen).last().map(String::as_str), Some("QUIT"));
}
