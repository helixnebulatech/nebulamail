// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The little HTTP discovery needs.1: https on port 443,
//! public addresses, the connection pinned to the checked address, no
//! redirects, no proxy, no cookies, no credentials, 256 KiB, 10 s.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HOST};
use http::{Method, Request, StatusCode};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use nebula_tls::{TlsConnector, TlsVerify};
use net_guard::{
    BlockReason, Host, NetworkAccess, Policy, PortPolicy, Resolver, Target, UrlPolicy,
};
use rustls_pki_types::ServerName;

const MAX_BODY: usize = 256 * 1024;
const TOTAL: Duration = Duration::from_secs(10);

/// Why a fetch gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// `net-guard` said no.
    #[error("{0}")]
    Blocked(BlockReason),
    /// Network, TLS, HTTP or a time limit.
    #[error("no usable answer")]
    Failed,
    /// Anything but a plain `200` (a redirect is not followed).
    #[error("HTTP {0}")]
    Status(u16),
}

/// The future of [`Fetch`]'s methods.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<String, FetchError>> + Send + 'a>>;

/// HTTP for the sources. Injected, so tests can answer for a server.
pub trait Fetch: Send + Sync {
    /// `GET url`.
    fn get<'a>(&'a self, url: &'a str) -> FetchFuture<'a>;
    /// `POST url` with an XML body.
    fn post_xml<'a>(&'a self, url: &'a str, body: String) -> FetchFuture<'a>;
}

/// The real one, through `net-guard`.
#[derive(Clone)]
pub struct GuardedFetch {
    tls: TlsConnector,
    resolver: Arc<dyn Resolver>,
}

impl std::fmt::Debug for GuardedFetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedFetch").finish_non_exhaustive()
    }
}

impl GuardedFetch {
    /// A client checking certificates with the platform's roots.
    ///
    /// # Errors
    /// If TLS can't be set up.
    pub fn new(resolver: Arc<dyn Resolver>) -> Result<Self, nebula_tls::TlsError> {
        Ok(Self {
            tls: TlsConnector::new(TlsVerify::Platform, &[b"http/1.1"])?,
            resolver,
        })
    }

    async fn exchange(
        &self,
        url: &str,
        method: Method,
        body: Option<String>,
    ) -> Result<String, FetchError> {
        let parsed =
            net_guard::parse_target_url(url, &UrlPolicy::HTTPS).map_err(FetchError::Blocked)?;
        let target = Target::new(parsed.host().clone(), parsed.port());
        let policy = Policy::new(PortPolicy::HTTPS, NetworkAccess::PublicOnly);
        let guarded = net_guard::connect(&target, &policy, self.resolver.as_ref())
            .await
            .map_err(|error| FetchError::Blocked(error.reason()))?;
        let (tcp, _peer, host) = guarded.into_parts();
        let Host::Domain(name) = &host else {
            // Discovery only ever asks by name.
            return Err(FetchError::Failed);
        };
        let server =
            ServerName::try_from(name.tls_name().to_owned()).map_err(|_| FetchError::Failed)?;
        let stream = self
            .tls
            .handshake(tcp, server)
            .await
            .map_err(|_| FetchError::Failed)?;
        let (mut sender, connection) = http1::Builder::new()
            .max_headers(64)
            .max_buf_size(16 * 1024)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(|_| FetchError::Failed)?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut path = parsed.url().path().to_owned();
        if let Some(query) = parsed.url().query() {
            path.push('?');
            path.push_str(query);
        }
        let content = body.unwrap_or_default();
        let is_post = method == Method::POST;
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, name.as_str())
            .header(ACCEPT_ENCODING, "identity")
            .header(CONTENT_LENGTH, content.len());
        if is_post {
            request = request.header(CONTENT_TYPE, "text/xml; charset=utf-8");
        }
        let request = request
            .body(Full::new(Bytes::from(content)))
            .map_err(|_| FetchError::Failed)?;
        let outcome = async {
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| FetchError::Failed)?;
            if response.status() != StatusCode::OK {
                return Err(FetchError::Status(response.status().as_u16()));
            }
            if response.headers().contains_key(CONTENT_ENCODING) {
                return Err(FetchError::Failed);
            }
            let body = Limited::new(response.into_body(), MAX_BODY)
                .collect()
                .await
                .map_err(|_| FetchError::Failed)?;
            String::from_utf8(body.to_bytes().to_vec()).map_err(|_| FetchError::Failed)
        }
        .await;
        driver.abort();
        outcome
    }
}

impl Fetch for GuardedFetch {
    fn get<'a>(&'a self, url: &'a str) -> FetchFuture<'a> {
        Box::pin(async move {
            tokio::time::timeout(TOTAL, self.exchange(url, Method::GET, None))
                .await
                .unwrap_or(Err(FetchError::Failed))
        })
    }

    fn post_xml<'a>(&'a self, url: &'a str, body: String) -> FetchFuture<'a> {
        Box::pin(async move {
            tokio::time::timeout(TOTAL, self.exchange(url, Method::POST, Some(body)))
                .await
                .unwrap_or(Err(FetchError::Failed))
        })
    }
}
