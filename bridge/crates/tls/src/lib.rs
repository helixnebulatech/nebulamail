// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! TLS for the bridge's clients (IMAP, CalDAV/CardDAV): rustls with
//! aws-lc-rs and the platform verifier.
//!
//! Moved here from the IMAP client so the DAV client uses the very same
//! rules: [`TlsVerify::InsecureLoopbackOnly`] turns the
//! certificate check off only when the connected peer is a loopback
//! address (development servers in Docker), and says so in the logs.
//!
//! ```no_run
//! # async fn example(stream: tokio::net::TcpStream) -> Result<(), nebula_tls::TlsError> {
//! use nebula_tls::{TlsConnector, TlsVerify};
//!
//! let connector = TlsConnector::new(TlsVerify::Platform, &[b"http/1.1"])?;
//! let name = "dav.example.com".try_into().map_err(|_| nebula_tls::TlsError::InvalidServerName)?;
//! let tls = connector.handshake(stream, name).await?;
//! # drop(tls);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

use std::net::IpAddr;
use std::sync::Arc;

use rustls::ClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, aws_lc_rs};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls_platform_verifier::BuilderVerifierExt as _;
use tokio::net::TcpStream;
pub use tokio_rustls::client::TlsStream;

/// How the server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TlsVerify {
    /// The system's trust store and rules (`rustls-platform-verifier`).
    Platform,
    /// No certificate check, for development servers with self-signed
    /// certificates. Refused unless the peer address is loopback, and logged
    /// as a warning on every connection.
    InsecureLoopbackOnly,
}

/// TLS errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsError {
    /// The TLS configuration could not be built.
    #[error("TLS configuration error: {0}")]
    Config(#[from] rustls::Error),
    /// The server name is not a valid DNS name or IP address.
    #[error("invalid server name")]
    InvalidServerName,
    /// Certificate checks can only be turned off for loopback peers.
    #[error("TLS verification can only be disabled for loopback servers")]
    InsecureNotLoopback,
    /// The handshake failed (also a certificate the verifier refused).
    #[error("TLS handshake failed: {0}")]
    Io(#[from] std::io::Error),
}

/// The peer of a connection is a loopback address (also `::ffff:127.x`).
///
/// # Errors
///
/// The peer address cannot be read.
pub fn peer_is_loopback(stream: &TcpStream) -> std::io::Result<bool> {
    let address = stream.peer_addr()?.ip();
    let canonical = match address {
        IpAddr::V6(v6) => v6.to_canonical(),
        IpAddr::V4(_) => address,
    };
    Ok(canonical.is_loopback())
}

/// Builds TLS sessions. Create it once and share it: building the platform
/// verifier is not free.
#[derive(Clone)]
pub struct TlsConnector {
    verify: TlsVerify,
    inner: tokio_rustls::TlsConnector,
}

impl std::fmt::Debug for TlsConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConnector")
            .field("verify", &self.verify)
            .finish_non_exhaustive()
    }
}

impl TlsConnector {
    /// Builds the TLS configuration, offering these ALPN protocols
    /// (`imap`, `http/1.1`…).
    ///
    /// # Errors
    ///
    /// The platform verifier or the protocol versions could not be set up.
    pub fn new(verify: TlsVerify, alpn: &[&[u8]]) -> Result<Self, TlsError> {
        let provider = Arc::new(aws_lc_rs::default_provider());
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()?;
        let mut config = match verify {
            TlsVerify::Platform => builder.with_platform_verifier()?.with_no_client_auth(),
            TlsVerify::InsecureLoopbackOnly => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoCertificateCheck::new(&provider)))
                .with_no_client_auth(),
        };
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        Ok(Self {
            verify,
            inner: tokio_rustls::TlsConnector::from(Arc::new(config)),
        })
    }

    /// How certificates are checked.
    #[must_use]
    pub const fn verify(&self) -> TlsVerify {
        self.verify
    }

    /// Runs the TLS handshake on `stream`. The caller puts a timeout
    /// around it.
    ///
    /// # Errors
    ///
    /// [`TlsError::InsecureNotLoopback`] if checks are off and the peer is
    /// not loopback; [`TlsError::Io`] if the handshake fails.
    pub async fn handshake(
        &self,
        stream: TcpStream,
        server_name: ServerName<'static>,
    ) -> Result<TlsStream<TcpStream>, TlsError> {
        if self.verify == TlsVerify::InsecureLoopbackOnly {
            if !peer_is_loopback(&stream)? {
                return Err(TlsError::InsecureNotLoopback);
            }
            tracing::warn!(
                "TLS certificate verification is disabled (loopback development server)"
            );
        }
        Ok(self.inner.connect(server_name, stream).await?)
    }
}

/// Accepts any certificate, but still checks the handshake signatures (so
/// the session keys belong to whoever holds the certificate's key).
#[derive(Debug)]
struct NoCertificateCheck {
    algorithms: WebPkiSupportedAlgorithms,
}

impl NoCertificateCheck {
    fn new(provider: &CryptoProvider) -> Self {
        Self {
            algorithms: provider.signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for NoCertificateCheck {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::net::{TcpListener, TcpStream};

    use super::{TlsConnector, TlsVerify, peer_is_loopback};

    #[tokio::test]
    async fn loopback_peer() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        assert!(peer_is_loopback(&stream).unwrap());
    }

    #[test]
    fn both_modes_build() {
        for verify in [TlsVerify::Platform, TlsVerify::InsecureLoopbackOnly] {
            let connector = TlsConnector::new(verify, &[b"http/1.1"]);
            assert!(matches!(connector, Ok(ref c) if c.verify() == verify));
        }
    }
}
