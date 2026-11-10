// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sends through the servers of `docker/compose.yaml` (Stalwart, Dovecot
//! relaying to Stalwart, or Postfix delivering to Dovecot; see
//! `nebula-tests`). Every test server uses a self-signed certificate.
//!
//! Skipped unless `NEBULA_TEST_SMTP=1`:
//!
//! ```sh
//! NEBULA_TEST_SMTP=1 cargo test -p nebula-smtp --test server
//! ```
//!
//! It leaves one message in bob's inbox (in Stalwart, or in Dovecot with
//! Postfix, two) per submission port, with a subject starting
//! `nebula-smtp bridge test`. Only Postfix announces DSN, so only there
//! do the client's and our own ENVID go out.

// The helpers below are test code too: a failed unwrap is a failed test.
// Clippy only knows that inside `#[test]` functions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "integration test helpers"
)]

use std::time::{SystemTime, UNIX_EPOCH};

use nebula_smtp::{
    CertificateCheck, Credentials, Envelope, EnvelopeId, MailDsn, OutgoingMessage, RcptDsn, RcptTo,
    Security, SmtpClient, SmtpError, SmtpServer, Stage,
};
use nebula_tests::{Endpoint, ServerKind, TestServer, TlsKind, User};
use secrecy::SecretString;
use tokio::net::TcpStream;

fn dev_client(security: Security, certificates: CertificateCheck) -> SmtpClient {
    let server =
        SmtpServer::new("localhost".parse().unwrap(), security).with_certificates(certificates);
    SmtpClient::new(server).unwrap()
}

fn security(endpoint: Endpoint) -> Security {
    match endpoint.tls {
        TlsKind::Implicit => Security::ImplicitTls,
        TlsKind::StartTls => Security::StartTls,
    }
}

fn alice(server: TestServer) -> Credentials {
    Credentials::new(User::Alice.login(), server.password(User::Alice)).unwrap()
}

#[tokio::test]
async fn alice_sends_to_bob() {
    let Some(server) = TestServer::smtp() else {
        return;
    };
    send_to_bob(server, server.smtp).await;
}

#[tokio::test]
async fn alice_sends_to_bob_with_starttls() {
    let Some(server) = TestServer::smtp() else {
        return;
    };
    if let Some(endpoint) = server.smtp_starttls {
        send_to_bob(server, endpoint).await;
    }
}

async fn send_to_bob(server: TestServer, endpoint: Endpoint) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let subject = format!("nebula-smtp bridge test {stamp}");
    let raw = format!(
        "From: alice@nebulamail.test\r\nTo: bob@nebulamail.test\r\nBcc: hidden@nebulamail.test\r\n\
         Subject: {subject}\r\nMessage-ID: <{stamp}@bridge-test.localhost>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Sent by the nebula-smtp integration test. Safe to delete.\r\n"
    );
    let message = OutgoingMessage::new(raw.as_bytes()).unwrap();
    assert!(!String::from_utf8_lossy(message.as_bytes()).contains("Bcc"));
    let envelope = Envelope::new(
        User::Alice.address().parse().unwrap(),
        vec![User::Bob.address().parse().unwrap()],
    )
    .unwrap();

    let stream = TcpStream::connect(endpoint.address()).await.unwrap();
    let mut session = dev_client(security(endpoint), CertificateCheck::InsecureLoopbackOnly)
        .connect(stream)
        .await
        .unwrap()
        .authenticate(&alice(server))
        .await
        .unwrap();
    // With DSN parameters if the server takes them (RFC 3461).
    let has_dsn = session.extensions().has("DSN");
    let client_envid: EnvelopeId = format!("bridge-test-{stamp}").parse().unwrap();
    let plain = envelope.clone();
    let envelope = if has_dsn {
        let bob: RcptTo = User::Bob.address().parse().unwrap();
        envelope
            .with_mail_dsn(MailDsn {
                ret: Some("HDRS".parse().unwrap()),
                envid: Some(client_envid.clone()),
            })
            .with_recipient_dsn(
                &bob,
                RcptDsn {
                    notify: Some("FAILURE,DELAY".parse().unwrap()),
                    orcpt: Some(format!("rfc822;{}", User::Bob.address()).parse().unwrap()),
                },
            )
            .unwrap()
    } else {
        envelope
    };
    // Our own ENVID never replaces the client's, and without DSN it is not
    // sent (and the message still goes out).
    let own = EnvelopeId::random().unwrap();
    let sent = session
        .send_tracked(&envelope, &message, Some(own.clone()))
        .await
        .unwrap();
    assert_eq!(sent.reply.code.get(), 250);
    assert_eq!(sent.envid, has_dsn.then_some(client_envid));
    println!("DSN offered by {}: {has_dsn}", endpoint.address());
    // Postfix is the test server with DSN (Stalwart 0.16 and Dovecot's
    // submission have none).
    assert_eq!(has_dsn, server.kind == ServerKind::Postfix);

    // The session can be reused: a refused recipient cancels only that
    // transaction.
    let unknown = Envelope::new(
        User::Alice.address().parse().unwrap(),
        vec!["nobody-here@nebulamail.test".parse().unwrap()],
    )
    .unwrap();
    match session.send(&unknown, &message).await {
        Err(SmtpError::RecipientsRejected { rejected }) => {
            assert_eq!(rejected.len(), 1);
            assert_eq!(
                rejected[0].recipient.as_str(),
                "nobody-here@nebulamail.test"
            );
        }
        other => panic!("expected a rejected recipient, got {other:?}"),
    }
    // With DSN and no ENVID from the client, ours goes out (only Postfix:
    // no second message through Stalwart, which limits alice's sends).
    if has_dsn {
        let sent = session
            .send_tracked(&plain, &message, Some(own.clone()))
            .await
            .unwrap();
        assert_eq!(sent.envid, Some(own));
    }
    session.quit().await;
    println!("sent through {}: {subject}", endpoint.address());
}

#[tokio::test]
async fn wrong_password_is_an_auth_error() {
    let Some(server) = TestServer::smtp() else {
        return;
    };
    let stream = TcpStream::connect(server.smtp.address()).await.unwrap();
    let credentials =
        Credentials::new(User::Alice.login(), SecretString::from("wrong password")).unwrap();
    let error = dev_client(
        security(server.smtp),
        CertificateCheck::InsecureLoopbackOnly,
    )
    .connect(stream)
    .await
    .unwrap()
    .authenticate(&credentials)
    .await
    .unwrap_err();
    assert!(matches!(error, SmtpError::Auth { .. }), "{error:?}");
}

#[tokio::test]
async fn self_signed_certificate_is_refused_by_default() {
    let Some(server) = TestServer::smtp() else {
        return;
    };
    let stream = TcpStream::connect(server.smtp.address()).await.unwrap();
    let error = dev_client(security(server.smtp), CertificateCheck::Platform)
        .connect(stream)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SmtpError::Tls {
            stage: Stage::TlsHandshake
        }
    );
}
