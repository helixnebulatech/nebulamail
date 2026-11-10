// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The client against fake servers on real sockets with real TLS: STARTTLS
//! downgrades and injection, capabilities before and after TLS, implicit
//! TLS, certificate checks, and the `net-guard` connector.
//!
//! Every fake server binds 127.0.0.1 and serves only the connection that
//! comes from the test's own socket: something on a developer machine (an
//! IDE) may probe every new listening port, and that must not count.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nebula_managesieve::proto::{Command, CommandDecoder, Limits as FrameLimits};
use nebula_managesieve::{Client, Error, Limits, Tls, TlsConnector, TlsVerify};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use secrecy::SecretString;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio_rustls::TlsAcceptor;

const CERT: &[u8] = include_bytes!("../../smtp/tests/data/localhost-cert.pem");
const KEY: &[u8] = include_bytes!("../../smtp/tests/data/localhost-key.pem");

const CAPS_TLS: &[u8] = b"\"IMPLEMENTATION\" \"Fake\"\r\n\"SASL\" \"PLAIN\"\r\n\
\"SIEVE\" \"fileinto vacation\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n";

fn acceptor() -> TlsAcceptor {
    let certs = CertificateDer::pem_slice_iter(CERT)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
    let config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

fn limits() -> Limits {
    Limits {
        greeting_timeout: Duration::from_secs(5),
        read_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
        ..Limits::default()
    }
}

/// A client socket and the server side of the same connection. Other
/// connections to the listener are dropped.
async fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = listener.local_addr().unwrap();
    let socket = TcpSocket::new_v4().unwrap();
    socket
        .bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap();
    let ours = socket.local_addr().unwrap();
    let accept = async move {
        loop {
            let (stream, peer) = listener.accept().await.unwrap();
            if peer == ours {
                return stream;
            }
        }
    };
    let (client, server) = tokio::join!(socket.connect(server_address), accept);
    (client.unwrap(), server)
}

fn name() -> ServerName<'static> {
    ServerName::try_from("localhost").unwrap()
}

fn insecure() -> TlsConnector {
    TlsConnector::new(TlsVerify::InsecureLoopbackOnly, &[]).unwrap()
}

async fn next_command<S: AsyncRead + Unpin>(
    stream: &mut S,
    decoder: &mut CommandDecoder,
) -> Option<Command> {
    let mut buffer = [0_u8; 4096];
    loop {
        if let Ok(Some(command)) = decoder.decode_command() {
            return Some(command);
        }
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => decoder.feed(&buffer[..n]),
        }
    }
}

/// After TLS: capabilities, then OK to AUTHENTICATE and CAPABILITY, an
/// empty LISTSCRIPTS, OK to LOGOUT. Returns the commands.
async fn serve_secure<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    caps: &[u8],
) -> Vec<Command> {
    let mut seen = Vec::new();
    stream.write_all(caps).await.unwrap();
    let mut decoder = CommandDecoder::new(FrameLimits::default());
    while let Some(command) = next_command(&mut stream, &mut decoder).await {
        let reply: &[u8] = match &command {
            Command::Authenticate { .. } | Command::ListScripts | Command::Logout => b"OK\r\n",
            Command::Capability => caps,
            _ => b"NO \"unexpected\"\r\n",
        };
        let logout = matches!(command, Command::Logout);
        seen.push(command);
        if stream.write_all(reply).await.is_err() || logout {
            break;
        }
    }
    seen
}

/// The plain part of a STARTTLS server: greeting, then STARTTLS's answer.
/// `after_ok` is sent in the same write as the OK (an injection test).
async fn plain_part(
    stream: &mut TcpStream,
    greeting: &[u8],
    answer: &[u8],
    after_ok: &[u8],
) -> bool {
    stream.write_all(greeting).await.unwrap();
    let mut decoder = CommandDecoder::new(FrameLimits::default());
    let Some(command) = next_command(stream, &mut decoder).await else {
        return false;
    };
    assert_eq!(
        command,
        Command::StartTls,
        "nothing but STARTTLS before TLS"
    );
    let mut bytes = answer.to_vec();
    bytes.extend_from_slice(after_ok);
    stream.write_all(&bytes).await.unwrap();
    answer.starts_with(b"OK")
}

const PLAIN_GREETING: &[u8] = b"\"IMPLEMENTATION\" \"Fake\"\r\n\"SASL\" \"\"\r\n\"STARTTLS\"\r\n\
\"VERSION\" \"1.0\"\r\nOK \"hi\"\r\n";

#[tokio::test]
async fn starttls_then_login() {
    let (client, mut server) = pair().await;
    let task = tokio::spawn(async move {
        assert!(plain_part(&mut server, PLAIN_GREETING, b"OK\r\n", b"").await);
        let tls = acceptor().accept(server).await.unwrap();
        serve_secure(tls, CAPS_TLS).await
    });
    let session = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap();
    // The capabilities are the ones sent over TLS (SASL was empty before).
    assert!(session.capabilities().has_sasl("PLAIN"));
    assert!(!session.capabilities().starttls);
    let mut session = session
        .login("alice", &SecretString::from("pw"))
        .await
        .unwrap();
    assert!(session.list_scripts().await.unwrap().is_empty());
    session.logout().await;
    let seen = task.await.unwrap();
    assert!(
        matches!(seen.first(), Some(Command::Authenticate { initial_response: Some(data), .. })
        if data.expose() == b"\0alice\0pw")
    );
}

#[tokio::test]
async fn starttls_missing_is_refused() {
    let (client, mut server) = pair().await;
    let task = tokio::spawn(async move {
        server
            .write_all(b"\"SASL\" \"PLAIN\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n")
            .await
            .unwrap();
        // Nothing may come: the client hangs up without sending a byte.
        let mut buffer = Vec::new();
        server.read_to_end(&mut buffer).await.unwrap();
        buffer
    });
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::StartTlsUnavailable), "{error:?}");
    assert_eq!(task.await.unwrap(), b"");
}

#[tokio::test]
async fn starttls_refused_is_refused() {
    let (client, mut server) = pair().await;
    let task = tokio::spawn(async move {
        plain_part(&mut server, PLAIN_GREETING, b"NO \"not now\"\r\n", b"").await;
        let mut buffer = Vec::new();
        server.read_to_end(&mut buffer).await.unwrap();
        buffer
    });
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::StartTlsUnavailable), "{error:?}");
    // No AUTHENTICATE in plaintext after the refusal.
    assert_eq!(task.await.unwrap(), b"");
}

#[tokio::test]
async fn bytes_after_starttls_ok_are_refused() {
    let (client, mut server) = pair().await;
    let _task = tokio::spawn(async move {
        // A man in the middle appends fake capabilities to the OK.
        plain_part(
            &mut server,
            PLAIN_GREETING,
            b"OK\r\n",
            b"\"SASL\" \"PLAIN\"\r\nOK\r\n",
        )
        .await;
        let mut buffer = Vec::new();
        let _ = server.read_to_end(&mut buffer).await;
    });
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InjectedBeforeTls), "{error:?}");
}

#[tokio::test]
async fn capabilities_before_tls_are_not_trusted() {
    // Before TLS the "server" (an attacker) offers PLAIN; after TLS the
    // real one doesn't: only what came over TLS counts.
    let (client, mut server) = pair().await;
    let _task = tokio::spawn(async move {
        let greeting = b"\"SASL\" \"PLAIN\"\r\n\"STARTTLS\"\r\nOK\r\n";
        assert!(plain_part(&mut server, greeting, b"OK\r\n", b"").await);
        let tls = acceptor().accept(server).await.unwrap();
        serve_secure(
            tls,
            b"\"SASL\" \"OAUTHBEARER\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n",
        )
        .await
    });
    let session = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap();
    let error = session
        .login("alice", &SecretString::from("pw"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NoAuthMechanism), "{error:?}");
}

#[tokio::test]
async fn bye_greeting() {
    let (client, mut server) = pair().await;
    let _task = tokio::spawn(async move {
        server
            .write_all(b"BYE (TRYLATER) \"too many connections\"\r\n")
            .await
            .unwrap();
    });
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Bye(ref text) if text == "too many connections"),
        "{error:?}"
    );
}

#[tokio::test]
async fn big_literal_before_tls_is_refused_at_once() {
    let (client, mut server) = pair().await;
    let _task = tokio::spawn(async move {
        // A 64 KiB literal in the plaintext greeting: fine after TLS, too
        // much before it. The server never ends the line.
        let mut greeting = b"\"IMPLEMENTATION\" {65536}\r\n".to_vec();
        greeting.resize(greeting.len() + 65536, b'x');
        let _ = server.write_all(&greeting).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    let started = std::time::Instant::now();
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::Decode(nebula_managesieve::proto::DecodeError::LiteralTooLong)
        ),
        "{error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn implicit_tls() {
    let (client, server) = pair().await;
    let task = tokio::spawn(async move {
        let tls = acceptor().accept(server).await.unwrap();
        serve_secure(tls, CAPS_TLS).await
    });
    let session = Client::connect(client, name(), Tls::Implicit, &insecure(), limits())
        .await
        .unwrap();
    let session = session
        .login("alice", &SecretString::from("pw"))
        .await
        .unwrap();
    session.logout().await;
    assert_eq!(task.await.unwrap().len(), 3);
}

#[tokio::test]
async fn plaintext_server_on_implicit_tls_fails() {
    let (client, mut server) = pair().await;
    let _task = tokio::spawn(async move {
        let _ = server.write_all(CAPS_TLS).await;
        let mut buffer = Vec::new();
        let _ = server.read_to_end(&mut buffer).await;
    });
    let error = Client::connect(client, name(), Tls::Implicit, &insecure(), limits())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Io(_)), "{error:?}");
}

#[tokio::test]
async fn certificates_are_checked() {
    // The platform verifier refuses the self-signed test certificate.
    let (client, server) = pair().await;
    let _task = tokio::spawn(async move {
        let _ = acceptor().accept(server).await;
    });
    let platform = TlsConnector::new(TlsVerify::Platform, &[]).unwrap();
    let error = Client::connect(client, name(), Tls::Implicit, &platform, limits())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Io(_)), "{error:?}");
}

#[tokio::test]
async fn slow_server_times_out() {
    let (client, _server) = pair().await;
    let limits = Limits {
        greeting_timeout: Duration::from_millis(300),
        ..limits()
    };
    let error = Client::connect(client, name(), Tls::StartTls, &insecure(), limits)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
}

#[tokio::test]
async fn connector_goes_through_net_guard() {
    use nebula_managesieve::{CertificateCheck, Connector, Credentials, SieveServer};
    use net_guard::{Port, SystemResolver};

    // A private (loopback) address without `private_network`: refused
    // before any connection.
    let server = SieveServer::new(
        "127.0.0.1".parse().unwrap(),
        Port::try_from(4190).unwrap(),
        Tls::StartTls,
    )
    .with_certificates(CertificateCheck::InsecureLoopbackOnly);
    let connector = Connector::new(server, Arc::new(SystemResolver::new().unwrap())).unwrap();
    let error = connector
        .login(&Credentials::new("alice", SecretString::from("pw")))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Guard(_)), "{error:?}");
}
