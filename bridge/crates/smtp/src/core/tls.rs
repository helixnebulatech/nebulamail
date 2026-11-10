// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! TLS settings for both ways of starting TLS.
//!
//! lettre can upgrade a stream it was given only with `STARTTLS`: there is
//! no public way to do implicit TLS on an existing stream (its
//! `connect_with_transport` reads the greeting straight away). So:
//!
//! - **Implicit TLS**: the handshake is done here with tokio-rustls and the
//!   encrypted stream is given to lettre, which then speaks SMTP over it.
//! - **STARTTLS**: lettre does the upgrade with [`TlsParameters`] built here
//!   from the same policy.
//!
//! Both use rustls with aws-lc-rs and the platform verifier, or no
//! verification at all for [`CertificateCheck::InsecureLoopbackOnly`].

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use lettre::transport::smtp::client::{AsyncTokioStream, TlsParameters, TlsParametersBuilder};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::core::budget::ReadBudget;

use crate::core::config::{CertificateCheck, Security, SmtpServer};
use crate::error::SmtpError;

/// The TLS settings of one server, built once (loading the system trust
/// store is not free).
#[derive(Clone)]
pub(crate) enum TlsSetup {
    Implicit(TlsConnector),
    StartTls(TlsParameters),
}

impl TlsSetup {
    pub(crate) fn new(server: &SmtpServer) -> Result<Self, SmtpError> {
        match server.security {
            Security::ImplicitTls => Ok(Self::Implicit(TlsConnector::from(Arc::new(
                client_config(server.certificates)?,
            )))),
            Security::StartTls => Ok(Self::StartTls(starttls_parameters(server)?)),
        }
    }
}

impl std::fmt::Debug for TlsSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Implicit(_) => "TlsSetup::Implicit",
            Self::StartTls(_) => "TlsSetup::StartTls",
        })
    }
}

/// Refuses [`CertificateCheck::InsecureLoopbackOnly`] unless the peer is a
/// loopback address. Called on the stream itself, so it holds whatever the
/// configuration or DNS said.
pub(crate) fn check_peer(check: CertificateCheck, stream: &TcpStream) -> Result<(), SmtpError> {
    match check {
        CertificateCheck::Platform => Ok(()),
        CertificateCheck::InsecureLoopbackOnly => {
            let peer = stream
                .peer_addr()
                .map_err(|_| SmtpError::Network {
                    stage: crate::Stage::TlsHandshake,
                })?
                .ip()
                .to_canonical();
            if !peer.is_loopback() {
                return Err(SmtpError::InsecureTlsNotLoopback { peer });
            }
            tracing::warn!(%peer, "SMTP certificate checks are off (development mode)");
            Ok(())
        }
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn client_config(check: CertificateCheck) -> Result<ClientConfig, SmtpError> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|_| SmtpError::TlsSetup)?
        .dangerous();
    let verifier: Arc<dyn ServerCertVerifier> = match check {
        CertificateCheck::Platform => Arc::new(
            rustls_platform_verifier::Verifier::new(provider).map_err(|_| SmtpError::TlsSetup)?,
        ),
        CertificateCheck::InsecureLoopbackOnly => Arc::new(AcceptAnyCertificate(
            provider.signature_verification_algorithms,
        )),
    };
    Ok(builder
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

fn starttls_parameters(server: &SmtpServer) -> Result<TlsParameters, SmtpError> {
    let builder = TlsParametersBuilder::new(server.host.as_str().to_owned());
    let builder = match server.certificates {
        // With lettre's `rustls-platform-verifier` feature, the default
        // certificate store is the platform verifier.
        CertificateCheck::Platform => builder,
        CertificateCheck::InsecureLoopbackOnly => builder.dangerous_accept_invalid_certs(true),
    };
    builder.build_rustls().map_err(|_| SmtpError::TlsSetup)
}

/// Accepts any certificate, but still checks the handshake signatures.
/// Only for [`CertificateCheck::InsecureLoopbackOnly`].
#[derive(Debug)]
struct AcceptAnyCertificate(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AcceptAnyCertificate {
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
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// A TCP stream with TLS already on, in the form lettre accepts. The
/// [`ReadBudget`] is under TLS, so it counts the handshake too.
#[derive(Debug)]
pub(crate) struct ImplicitTlsStream(TlsStream<ReadBudget<TcpStream>>);

impl ImplicitTlsStream {
    pub(crate) async fn handshake(
        connector: &TlsConnector,
        name: ServerName<'static>,
        stream: ReadBudget<TcpStream>,
    ) -> io::Result<Self> {
        connector.connect(name, stream).await.map(Self)
    }
}

impl AsyncRead for ImplicitTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for ImplicitTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl AsyncTokioStream for ImplicitTlsStream {
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0.get_ref().0.get_ref().peer_addr()
    }
}
