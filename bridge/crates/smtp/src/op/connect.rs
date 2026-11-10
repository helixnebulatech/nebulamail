// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::io;

use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::commands::Ehlo;
use lettre::transport::smtp::extension::{ClientId, Extension};
use tokio::net::TcpStream;

use crate::core::activity::Activity;
use crate::core::budget::{self, ReadBudget};
use crate::core::session::{self, SecureSession, SmtpClient, timed};
use crate::core::tls::{self, ImplicitTlsStream, TlsSetup};
use crate::dsn::ServerExtensions;
use crate::error::{SmtpError, Stage};

impl SmtpClient {
    /// Starts a session on an already connected TCP stream: TLS (implicit
    /// or required STARTTLS), greeting and `EHLO`.
    ///
    /// The caller opens the stream (through `net-guard`, pinned to a
    /// validated IP), so no DNS lookup happens here.
    ///
    /// # Errors
    ///
    /// [`SmtpError::InsecureTlsNotLoopback`], [`SmtpError::TlsUnavailable`],
    /// [`SmtpError::Tls`], [`SmtpError::Timeout`], [`SmtpError::Network`],
    /// [`SmtpError::Protocol`] (bad replies, or more than a session ever
    /// needs: 1 MiB per connection), or [`SmtpError::Rejected`] if the
    /// server greets with an error.
    pub async fn connect(&self, stream: TcpStream) -> Result<SecureSession, SmtpError> {
        tls::check_peer(self.server.certificates, &stream)?;
        // Under everything else (TLS, lettre): caps what the server can
        // make us read, since lettre reads lines without a limit.
        let activity = Activity::default();
        let stream = ReadBudget::new(stream).with_activity(activity.clone());
        let hello = self.server.hello.client_id();
        let mut conn = match &self.tls {
            TlsSetup::Implicit(connector) => {
                let tls = timed(
                    Stage::TlsHandshake,
                    self.server.timeouts.command,
                    ImplicitTlsStream::handshake(connector, self.server.host.server_name(), stream),
                )
                .await?
                .map_err(|error| handshake_error(&error))?;
                self.greet(Box::new(tls), &hello).await?
            }
            TlsSetup::StartTls(parameters) => {
                let mut conn = self.greet(Box::new(stream), &hello).await?;
                if !conn.server_info().supports_feature(Extension::StartTls) {
                    session::quit(&mut conn).await;
                    return Err(SmtpError::TlsUnavailable);
                }
                timed(
                    Stage::StartTls,
                    self.server.timeouts.command,
                    conn.starttls(parameters.clone(), &hello),
                )
                .await?
                .map_err(|error| starttls_error(&error))?;
                // lettre says so after a successful upgrade; checked anyway,
                // since everything after this point must be encrypted.
                if !conn.is_encrypted() {
                    return Err(SmtpError::Tls {
                        stage: Stage::StartTls,
                    });
                }
                conn
            }
        };
        // lettre keeps only the extensions it knows (no DSN): ask again,
        // over TLS, and keep the whole list. EHLO may be repeated
        // (RFC 5321 §4.1.4).
        let ehlo = timed(
            Stage::Greeting,
            self.server.timeouts.command,
            conn.command(Ehlo::new(hello)),
        )
        .await?
        .map_err(|error| SmtpError::from_lettre(Stage::Greeting, &error))?;
        let extensions = ServerExtensions::from_ehlo_lines(ehlo.message());
        Ok(SecureSession {
            conn,
            timeouts: self.server.timeouts,
            extensions,
            activity,
        })
    }

    /// Reads the greeting and sends `EHLO`.
    async fn greet(
        &self,
        stream: Box<dyn lettre::transport::smtp::client::AsyncTokioStream>,
        hello: &ClientId,
    ) -> Result<AsyncSmtpConnection, SmtpError> {
        timed(
            Stage::Greeting,
            self.server.timeouts.command,
            AsyncSmtpConnection::connect_with_transport(stream, hello),
        )
        .await?
        .map_err(|error| SmtpError::from_lettre(Stage::Greeting, &error))
    }
}

/// A rustls error inside the I/O error means the handshake itself failed
/// (certificate, protocol); too many bytes is a protocol error; anything
/// else is the network.
fn handshake_error(error: &io::Error) -> SmtpError {
    if budget::received_too_much(error) {
        return SmtpError::Protocol {
            stage: Stage::TlsHandshake,
        };
    }
    let is_tls = error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some());
    if is_tls {
        SmtpError::Tls {
            stage: Stage::TlsHandshake,
        }
    } else {
        SmtpError::Network {
            stage: Stage::TlsHandshake,
        }
    }
}

/// lettre reports a failed upgrade as a connection error, not a TLS one;
/// after the server said yes to `STARTTLS`, it is the handshake.
fn starttls_error(error: &lettre::transport::smtp::Error) -> SmtpError {
    match SmtpError::from_lettre(Stage::StartTls, error) {
        SmtpError::Network { stage } => SmtpError::Tls { stage },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::core::config::{Security, SmtpServer, Timeouts};

    /// A fake plaintext server: sends `script` replies, one per line the
    /// client sends (the first one before reading anything), then returns
    /// everything it received.
    async fn fake_server(script: &'static [&'static str]) -> (TcpStream, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            let mut received = String::new();
            for (index, reply) in script.iter().enumerate() {
                if index > 0 {
                    read.read_line(&mut received).await.unwrap();
                }
                write.write_all(reply.as_bytes()).await.unwrap();
            }
            let mut rest = String::new();
            let _ = read.read_to_string(&mut rest).await;
            received + &rest
        });
        (TcpStream::connect(address).await.unwrap(), server)
    }

    fn client(security: Security) -> SmtpClient {
        let server =
            SmtpServer::new("localhost".parse().unwrap(), security).with_timeouts(Timeouts {
                command: Duration::from_millis(500),
                data_start: Duration::from_millis(500),
                data_block: Duration::from_millis(500),
                data_end: Duration::from_millis(500),
            });
        SmtpClient::new(server).unwrap()
    }

    #[tokio::test]
    async fn starttls_is_required() {
        let (stream, server) =
            fake_server(&["220 fake ESMTP\r\n", "250-fake\r\n250 AUTH PLAIN LOGIN\r\n"]).await;
        let error = client(Security::StartTls)
            .connect(stream)
            .await
            .unwrap_err();
        assert_eq!(error, SmtpError::TlsUnavailable);
        let received = server.await.unwrap();
        // RFC 5321 §4.1.4: an address literal, not `localhost`.
        let hello = format!("EHLO [{}]\r\n", ["127", "0", "0", "1"].join("."));
        assert!(received.starts_with(&hello), "{received:?}");
        assert!(!received.contains("AUTH"), "{received:?}");
        assert!(!received.contains("MAIL"), "{received:?}");
    }

    #[tokio::test]
    async fn starttls_refused_by_the_server() {
        let (stream, _server) = fake_server(&[
            "220 fake ESMTP\r\n",
            "250-fake\r\n250 STARTTLS\r\n",
            "454 4.7.0 TLS not available\r\n",
            // lettre sends QUIT after an error reply and waits for this.
            "221 bye\r\n",
        ])
        .await;
        let error = client(Security::StartTls)
            .connect(stream)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                SmtpError::Rejected {
                    stage: Stage::StartTls,
                    permanent: false,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn implicit_tls_against_a_plaintext_server_fails() {
        let (stream, _server) = fake_server(&["220 fake ESMTP\r\n"]).await;
        let error = client(Security::ImplicitTls)
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

    #[tokio::test]
    async fn greeting_timeout() {
        let (stream, _server) = fake_server(&[""]).await;
        let error = client(Security::StartTls)
            .connect(stream)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            SmtpError::Timeout {
                stage: Stage::Greeting,
                after: Duration::from_millis(500)
            }
        );
    }

    #[tokio::test]
    async fn endless_greeting_line_is_cut_before_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut flood = b"220 ".to_vec();
            flood.resize(2 * 1024 * 1024, b'A');
            // The client hangs up in the middle: that write fails.
            let _ = socket.write_all(&flood).await;
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let server = SmtpServer::new("localhost".parse().unwrap(), Security::StartTls)
            .with_timeouts(Timeouts {
                command: Duration::from_secs(5),
                data_start: Duration::from_secs(5),
                data_block: Duration::from_secs(5),
                data_end: Duration::from_secs(5),
            });
        let started = std::time::Instant::now();
        let error = SmtpClient::new(server)
            .unwrap()
            .connect(stream)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            SmtpError::Protocol {
                stage: Stage::Greeting
            }
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn ehlo_line_of_only_spaces_does_not_panic() {
        let (stream, _server) =
            fake_server(&["220 fake ESMTP\r\n", "250-fake\r\n250-  \r\n250 OK\r\n"]).await;
        let error = client(Security::StartTls)
            .connect(stream)
            .await
            .unwrap_err();
        assert_eq!(error, SmtpError::TlsUnavailable);
    }

    #[tokio::test]
    async fn error_greeting_is_rejected() {
        let (stream, _server) = fake_server(&["554 go away\r\n"]).await;
        let error = client(Security::StartTls)
            .connect(stream)
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                SmtpError::Rejected { stage: Stage::Greeting, permanent: true, text, .. }
                    if text.as_str().contains("go away")
            ),
            "{error:?}"
        );
    }
}
