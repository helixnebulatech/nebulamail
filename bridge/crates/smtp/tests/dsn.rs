// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! DSN parameters (RFC 3461) against a fake server over implicit TLS: they
//! go out (xtext-encoded) only if the server announces DSN, and the
//! bridge's own ENVID only if the `EHLO` of that connection has DSN.

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
    CertificateCheck, Credentials, DsnNotify, Envelope, EnvelopeId, MailDsn, OutgoingMessage,
    RcptDsn, RcptTo, Security, SmtpClient, SmtpError, SmtpServer, Timeouts,
};
use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A server that accepts everything; returns the command lines.
async fn server(ehlo: &'static str) -> (TcpStream, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let tls = common::acceptor().accept(socket).await.unwrap();
        let mut stream = BufReader::new(tls);
        stream
            .get_mut()
            .write_all(b"220 fake ESMTP\r\n")
            .await
            .unwrap();
        let mut received = Vec::new();
        let mut in_data = false;
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                return received;
            }
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    stream.get_mut().write_all(b"250 queued\r\n").await.unwrap();
                }
                continue;
            }
            let upper = line.to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") {
                ehlo.as_bytes()
            } else if upper.starts_with("AUTH") {
                b"235 ok\r\n"
            } else if upper.starts_with("DATA") {
                in_data = true;
                b"354 go\r\n"
            } else if upper.starts_with("QUIT") {
                b"221 bye\r\n"
            } else {
                b"250 ok\r\n"
            };
            received.push(line.trim_end().to_owned());
            if stream.get_mut().write_all(reply).await.is_err() {
                return received;
            }
        }
    });
    (TcpStream::connect(address).await.unwrap(), task)
}

async fn send(stream: TcpStream, envelope: &Envelope) -> Result<(), SmtpError> {
    let server = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_timeouts(Timeouts {
            command: Duration::from_secs(5),
            data_start: Duration::from_secs(5),
            data_block: Duration::from_secs(5),
            data_end: Duration::from_secs(5),
        });
    let credentials = Credentials::new("alice", SecretString::from("secret")).unwrap();
    let mut session = SmtpClient::new(server)
        .unwrap()
        .connect(stream)
        .await?
        .authenticate(&credentials)
        .await?;
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    let result = session.send(envelope, &message).await.map(|_| ());
    session.quit().await;
    result
}

fn envelope() -> Envelope {
    let bob: RcptTo = "bob@example.org".parse().unwrap();
    let carol: RcptTo = "carol@example.org".parse().unwrap();
    Envelope::new(
        "alice@example.com".parse().unwrap(),
        vec![bob.clone(), carol],
    )
    .unwrap()
    .with_mail_dsn(MailDsn {
        ret: Some("hdrs".parse().unwrap()),
        envid: Some("QQ314159+x".parse().unwrap()),
    })
    .with_recipient_dsn(
        &bob,
        RcptDsn {
            notify: Some("failure,delay".parse::<DsnNotify>().unwrap()),
            orcpt: Some("rfc822;bob@example.org".parse().unwrap()),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn dsn_parameters_are_sent() {
    let (stream, server) = server("250-fake\r\n250-DSN\r\n250 AUTH PLAIN\r\n").await;
    send(stream, &envelope()).await.unwrap();
    let received = server.await.unwrap();
    let line = |prefix: &str| {
        received
            .iter()
            .find(|l| l.starts_with(prefix))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        line("MAIL FROM"),
        "MAIL FROM:<alice@example.com> RET=HDRS ENVID=QQ314159+2Bx"
    );
    assert_eq!(
        line("RCPT TO:<bob"),
        "RCPT TO:<bob@example.org> NOTIFY=FAILURE,DELAY ORCPT=rfc822;bob@example.org"
    );
    assert_eq!(line("RCPT TO:<carol"), "RCPT TO:<carol@example.org>");
}

#[tokio::test]
async fn dsn_needs_the_extension() {
    let (stream, server) = server("250-fake\r\n250 AUTH PLAIN\r\n").await;
    assert_eq!(
        send(stream, &envelope()).await,
        Err(SmtpError::DsnUnsupported)
    );
    let received = server.await.unwrap();
    assert!(
        !received.iter().any(|l| l.starts_with("MAIL")),
        "{received:?}"
    );
}

/// Sends `envelope` with `own` as the fallback ENVID; returns the ENVID
/// `send_tracked` says it sent and the `MAIL FROM` line the server got.
async fn send_tracked(
    ehlo: &'static str,
    envelope: &Envelope,
    own: &EnvelopeId,
) -> (Option<EnvelopeId>, String) {
    let (stream, server) = server(ehlo).await;
    let client = SmtpClient::new(
        SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
            .with_certificates(CertificateCheck::InsecureLoopbackOnly),
    )
    .unwrap();
    let credentials = Credentials::new("alice", SecretString::from("secret")).unwrap();
    let mut session = client
        .connect(stream)
        .await
        .unwrap()
        .authenticate(&credentials)
        .await
        .unwrap();
    let message = OutgoingMessage::new(b"Subject: hi\r\n\r\nHi\r\n").unwrap();
    let sent = session
        .send_tracked(envelope, &message, Some(own.clone()))
        .await
        .unwrap();
    session.quit().await;
    let received = server.await.unwrap();
    let mail = received
        .iter()
        .find(|l| l.starts_with("MAIL FROM"))
        .cloned()
        .unwrap();
    (sent.envid, mail)
}

#[tokio::test]
async fn own_envid_only_if_this_ehlo_has_dsn() {
    let own = EnvelopeId::random().unwrap();
    let plain = Envelope::new(
        "alice@example.com".parse().unwrap(),
        vec!["bob@example.org".parse().unwrap()],
    )
    .unwrap();
    let with_dsn = "250-fake\r\n250-DSN\r\n250 AUTH PLAIN\r\n";

    // DSN: ours goes out.
    let (sent, mail) = send_tracked(with_dsn, &plain, &own).await;
    assert_eq!(sent.as_ref(), Some(&own));
    assert_eq!(
        mail,
        format!("MAIL FROM:<alice@example.com> ENVID={}", own.as_str())
    );

    // No DSN: the message still goes out, without ENVID.
    let (sent, mail) = send_tracked("250-fake\r\n250 AUTH PLAIN\r\n", &plain, &own).await;
    assert_eq!(sent, None);
    assert_eq!(mail, "MAIL FROM:<alice@example.com>");

    // The client's ENVID wins over ours.
    let (sent, mail) = send_tracked(with_dsn, &envelope(), &own).await;
    assert_eq!(sent.as_ref().map(EnvelopeId::as_str), Some("QQ314159+x"));
    assert_eq!(
        mail,
        "MAIL FROM:<alice@example.com> RET=HDRS ENVID=QQ314159+2Bx"
    );
}

#[tokio::test]
async fn utf8_orcpt_goes_out_as_ascii() {
    let bob: RcptTo = "bob@example.net".parse().unwrap();
    let envelope = Envelope::new("alice@example.com".parse().unwrap(), vec![bob.clone()])
        .unwrap()
        .with_recipient_dsn(
            &bob,
            RcptDsn {
                notify: None,
                orcpt: Some("utf-8;bø+1@example.net".parse().unwrap()),
            },
        )
        .unwrap();
    let (stream, server) = server("250-fake\r\n250-DSN\r\n250 AUTH PLAIN\r\n").await;
    send(stream, &envelope).await.unwrap();
    let received = server.await.unwrap();
    let rcpt = received
        .iter()
        .find(|l| l.starts_with("RCPT"))
        .cloned()
        .unwrap();
    assert!(rcpt.is_ascii(), "{rcpt:?}");
    assert_eq!(
        rcpt,
        "RCPT TO:<bob@example.net> ORCPT=utf-8;b\\x{F8}\\x{2B}1@example.net"
    );
}
