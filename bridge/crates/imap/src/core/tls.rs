// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! TLS: rustls + aws-lc-rs + platform verifier, from
//! the shared `nebula-tls` crate (the DAV client uses the same rules).

use nebula_tls::TlsError;
pub use nebula_tls::TlsVerify;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::Error;

/// How TLS starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Tls {
    /// TLS from the first byte (port 993).
    Implicit,
    /// Plain greeting, then STARTTLS, which is mandatory: if the server does
    /// not offer it, we do not connect.
    StartTls,
}

/// Builds TLS sessions (ALPN `imap`). Create it once and share it:
/// building the platform verifier is not free.
#[derive(Debug, Clone)]
pub struct TlsConnector {
    inner: nebula_tls::TlsConnector,
}

impl From<TlsError> for Error {
    fn from(error: TlsError) -> Self {
        match error {
            TlsError::Config(error) => Self::TlsConfig(error),
            TlsError::InvalidServerName => Self::InvalidServerName,
            TlsError::InsecureNotLoopback => Self::InsecureTlsNotLoopback,
            TlsError::Io(error) => Self::Io(error),
            other => Self::Io(std::io::Error::other(other)),
        }
    }
}

impl TlsConnector {
    /// Builds the TLS configuration.
    ///
    /// # Errors
    ///
    /// The platform verifier or the protocol versions could not be set up.
    pub fn new(verify: TlsVerify) -> Result<Self, Error> {
        Ok(Self {
            inner: nebula_tls::TlsConnector::new(verify, &[b"imap"])?,
        })
    }

    /// Runs the TLS handshake on `stream`.
    pub(crate) async fn handshake(
        &self,
        stream: TcpStream,
        server_name: ServerName<'static>,
    ) -> Result<TlsStream<TcpStream>, Error> {
        Ok(self.inner.handshake(stream, server_name).await?)
    }
}
