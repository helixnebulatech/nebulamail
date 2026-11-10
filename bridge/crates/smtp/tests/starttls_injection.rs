// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! STARTTLS response injection.
//!
//! A man in the middle answers `STARTTLS` with `220` **and**, in the same
//! write, the replies the client will wait for later. If the client keeps
//! the bytes it already read before the TLS upgrade, it takes them as
//! encrypted replies from the real server (CVE-2011-0411 in Postfix,
//! CVE-2020-14954 in Mutt).
//!
//! lettre 0.11.23 does exactly that: with the unpatched crate
//! `forged_replies_after_starttls_are_refused` *passes the send* and returns
//! the forged `250 ... queued as FORGED` while the real server (over TLS)
//! refused the recipient. The vendored copy in `bridge/vendor/lettre`
//! (`PATCHES.md`) refuses a STARTTLS reply that comes with extra bytes, so
//! now the connection fails before anything is sent.
//!
//! The fake server uses a self-signed certificate for `localhost` made only
//! for these tests (`tests/data`, valid until 2126): it protects nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod common;

use std::time::Duration;

use nebula_smtp::{
    CertificateCheck, Credentials, Envelope, OutgoingMessage, Security, SmtpClient, SmtpError,
    SmtpServer, Stage, Timeouts,
};
use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the attacker puts after the real `220` to STARTTLS: a reply for
/// every command the client sends next (EHLO, AUTH, MAIL, RCPT, DATA and the
/// end of the message).
const FORGED: &str = "220 2.0.0 Ready to start TLS\r\n\
    250-forged.example\r\n250 AUTH PLAIN\r\n\
    235 2.7.0 Authentication successful\r\n\
    250 2.1.0 Ok\r\n\
    250 2.1.5 Ok\r\n\
    354 End data with <CR><LF>.<CR><LF>\r\n\
    250 2.0.0 Ok: queued as FORGED\r\n";

async fn line<S: AsyncRead + Unpin>(read: &mut BufReader<S>) -> String {
    // The client may hang up without a TLS close_notify: that is the end.
    let mut line = String::new();
    if read.read_line(&mut line).await.is_err() {
        line.clear();
    }
    line
}

/// The real server once TLS is up: accepts the login and the sender,
/// refuses the recipient. Returns the lines it received over TLS.
async fn real_server_over_tls<S: AsyncRead + AsyncWrite + Unpin>(stream: S) -> Vec<String> {
    let mut stream = BufReader::new(stream);
    let mut received = Vec::new();
    loop {
        let command = line(&mut stream).await;
        if command.is_empty() {
            return received;
        }
        let upper = command.to_ascii_uppercase();
        let reply: &[u8] = if upper.starts_with("EHLO") {
            b"250-real.example\r\n250 AUTH PLAIN\r\n"
        } else if upper.starts_with("AUTH") {
            b"235 2.7.0 Authentication successful\r\n"
        } else if upper.starts_with("MAIL") {
            b"250 2.1.0 Ok\r\n"
        } else if upper.starts_with("RCPT") {
            b"550 5.1.1 No such user\r\n"
        } else if upper.starts_with("QUIT") {
            b"221 Bye\r\n"
        } else {
            b"554 5.5.1 No valid recipients\r\n"
        };
        received.push(command);
        if stream.get_mut().write_all(reply).await.is_err() {
            return received;
        }
    }
}

/// A plaintext dialogue up to STARTTLS, where the attacker answers with
/// `starttls_reply` in one write, then the real server over TLS.
async fn server(starttls_reply: &'static str) -> (TcpStream, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut plain = BufReader::new(socket);
        plain
            .get_mut()
            .write_all(b"220 real.example ESMTP\r\n")
            .await
            .unwrap();
        assert!(line(&mut plain).await.starts_with("EHLO"));
        plain
            .get_mut()
            .write_all(b"250-real.example\r\n250 STARTTLS\r\n")
            .await
            .unwrap();
        assert_eq!(line(&mut plain).await, "STARTTLS\r\n");
        // One write, so the client reads everything in one go together
        // with the 220.
        plain
            .get_mut()
            .write_all(starttls_reply.as_bytes())
            .await
            .unwrap();
        assert!(plain.buffer().is_empty());
        let Ok(tls) = common::acceptor().accept(plain.into_inner()).await else {
            return Vec::new();
        };
        real_server_over_tls(tls).await
    });
    (TcpStream::connect(address).await.unwrap(), task)
}

fn client() -> SmtpClient {
    let server = SmtpServer::new("localhost".parse().unwrap(), Security::StartTls)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_timeouts(Timeouts {
            command: Duration::from_secs(5),
            data_start: Duration::from_secs(5),
            data_block: Duration::from_secs(5),
            data_end: Duration::from_secs(5),
        });
    SmtpClient::new(server).unwrap()
}

/// connect → AUTH → MAIL/RCPT/DATA, as the bridge does.
async fn send(stream: TcpStream) -> Result<String, SmtpError> {
    let credentials = Credentials::new("alice", SecretString::from("secret")).unwrap();
    let envelope = Envelope::new(
        "alice@example.com".parse().unwrap(),
        vec!["nobody@example.org".parse().unwrap()],
    )
    .unwrap();
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    let mut session = client()
        .connect(stream)
        .await?
        .authenticate(&credentials)
        .await?;
    let reply = session.send(&envelope, &message).await?;
    session.quit().await;
    Ok(reply.text.as_str().to_owned())
}

#[tokio::test]
async fn forged_replies_after_starttls_are_refused() {
    let (stream, server) = server(FORGED).await;
    let result = send(stream).await;
    // Unpatched lettre 0.11.23: `Ok("2.0.0 Ok: queued as FORGED")`, while
    // the real server answered `550 No such user` to RCPT over TLS.
    assert_eq!(
        result,
        Err(SmtpError::Protocol {
            stage: Stage::StartTls
        })
    );
    // Nothing went over TLS: not even EHLO, let alone AUTH.
    assert_eq!(server.await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn clean_starttls_still_works() {
    let (stream, server) = server("220 2.0.0 Ready to start TLS\r\n").await;
    let result = send(stream).await;
    // The real server's refusal reaches us.
    assert!(
        matches!(&result, Err(SmtpError::RecipientsRejected { rejected }) if rejected.len() == 1),
        "{result:?}"
    );
    let received = server.await.unwrap();
    // lettre's EHLO after TLS, and ours (for the full extension list).
    assert!(received[0].starts_with("EHLO"), "{received:?}");
    assert!(received[1].starts_with("EHLO"), "{received:?}");
    assert!(received[2].starts_with("AUTH PLAIN"), "{received:?}");
}
