// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Shared by the fake TLS servers of the integration tests.
//!
//! The certificate is self-signed for `localhost`, made only for these tests
//! (`tests/data`, valid until 2126): it protects nothing.

use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;

const CERT: &[u8] = include_bytes!("../data/localhost-cert.pem");
const KEY: &[u8] = include_bytes!("../data/localhost-key.pem");

/// A TLS acceptor with the test certificate.
pub(crate) fn acceptor() -> TlsAcceptor {
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
