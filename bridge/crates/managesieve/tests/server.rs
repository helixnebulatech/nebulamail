// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Against the ManageSieve of Stalwart or Dovecot (Pigeonhole) in
//! `docker/compose.yaml`. Skipped unless
//! `NEBULA_TEST_SIEVE=1`; `NEBULA_TEST_SERVER` picks the server
//! (`nebula-tests`):
//!
//! ```sh
//! NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=dovecot cargo test -p nebula-managesieve --test server
//! ```
//!
//! Every test uses its own script names and leaves the account's active
//! script as it found it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::sync::Arc;

use nebula_managesieve::proto::{ResponseCode, ScriptName};
use nebula_managesieve::{
    Authenticated, CertificateCheck, CheckOutcome, Client, Connector, Credentials, Error,
    SieveServer, Tls,
};
use nebula_tests::{ServerKind, TestServer, User};
use net_guard::{Port, SystemResolver};
use secrecy::SecretString;

fn connector(server: TestServer) -> Connector {
    let sieve = SieveServer::new(
        nebula_tests::HOST.parse().unwrap(),
        Port::try_from(server.sieve.port).unwrap(),
        Tls::StartTls,
    )
    .with_certificates(CertificateCheck::InsecureLoopbackOnly)
    .with_private_network(true);
    Connector::new(sieve, Arc::new(SystemResolver::new().unwrap())).unwrap()
}

async fn login(server: TestServer, user: User) -> Client<Authenticated> {
    connector(server)
        .login(&Credentials::new(user.login(), server.password(user)))
        .await
        .unwrap()
}

fn unique(prefix: &str) -> ScriptName {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    ScriptName::new(format!("{prefix}-{nanos}")).unwrap()
}

#[tokio::test]
async fn scripts_round_trip() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    let mut client = login(server, User::Alice).await;
    let caps = client.capabilities().clone();
    assert!(
        caps.has_extension("vacation") && caps.has_extension("include"),
        "{caps:?}"
    );
    // SASL is usually empty once logged in (RFC 5804 §1.7).
    assert!(caps.is_rfc5804());
    match server.kind {
        ServerKind::Stalwart => assert!(caps.has_extension("mailboxid")),
        ServerKind::Dovecot | ServerKind::Postfix | ServerKind::DovecotPrefix => {
            assert!(!caps.has_extension("mailboxid"));
        }
    }
    let before = client.list_scripts().await.unwrap();
    let was_active = before.iter().find(|s| s.active).map(|s| s.name.clone());

    let name = unique("nebula-test");
    // UTF-8, CRLF, a quote, a backslash and a line starting with a dot:
    // all opaque bytes to the protocol.
    let content = "require [\"fileinto\"];\r\n# Ünïcode \"quoted\" \\ back\r\n/*\r\n.dot\r\n*/\r\nif header :contains \"subject\" \"x\" { fileinto \"INBOX\"; }\r\n";
    assert!(client.have_space(&name, 1024).await.unwrap());
    client.put_script(&name, content.as_bytes()).await.unwrap();
    assert_eq!(client.get_script(&name).await.unwrap(), content.as_bytes());
    let listed = client.list_scripts().await.unwrap();
    assert!(listed.iter().any(|s| s.name == name && !s.active));

    assert!(matches!(
        client.check_script(content.as_bytes()).await.unwrap(),
        CheckOutcome::Valid { .. }
    ));
    let CheckOutcome::Invalid(message) = client
        .check_script(b"require [\"fileinto\"];\r\nnot_a_command;\r\n")
        .await
        .unwrap()
    else {
        panic!("an invalid script was accepted");
    };
    // Dovecot names line 2; Stalwart 0.16's ManageSieve says "at line 0,
    // column 0" whatever the line (its JMAP gives the right one).
    assert!(message.contains("not_a_command"), "{message}");
    // PUTSCRIPT of an invalid script is refused and stores nothing.
    let bad = unique("nebula-test-bad");
    assert!(matches!(
        client.put_script(&bad, b"nope nope;\r\n").await,
        Err(Error::Refused(_))
    ));

    let renamed = unique("nebula-test-renamed");
    client.set_active(Some(&name)).await.unwrap();
    client.rename_script(&name, &renamed).await.unwrap();
    let listed = client.list_scripts().await.unwrap();
    assert!(
        listed.iter().any(|s| s.name == renamed && s.active),
        "{listed:?}"
    );
    assert!(!listed.iter().any(|s| s.name == name || s.name == bad));
    // The active script can't be deleted.
    assert!(matches!(
        client.delete_script(&renamed).await.unwrap_err().code(),
        Some(ResponseCode::Active)
    ));
    client.set_active(was_active.as_ref()).await.unwrap();
    client.delete_script(&renamed).await.unwrap();
    let error = client.get_script(&renamed).await.unwrap_err();
    assert!(matches!(error, Error::Refused(_)), "{error:?}");
    assert!(!error.is_fatal());
    client.noop().await.unwrap();
    let after = client.list_scripts().await.unwrap();
    assert_eq!(
        after.iter().find(|s| s.active).map(|s| &s.name),
        was_active.as_ref()
    );
    client.logout().await;
}

#[tokio::test]
async fn wrong_password_is_refused() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    // Stalwart blocks the whole IP after failed logins:
    // only Dovecot gets a wrong password.
    if server.kind == ServerKind::Stalwart {
        return;
    }
    let error = connector(server)
        .login(&Credentials::new(
            "alice",
            SecretString::from("not the password"),
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::AuthFailed(_)), "{error:?}");
}
