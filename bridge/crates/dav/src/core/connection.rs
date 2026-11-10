// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One HTTP/1.1 connection: `net-guard` (IP pinned, every address
//! checked), then TLS, then hyper's client connection. No proxy: hyper's
//! `client::conn` never reads `HTTP_PROXY` and `net-guard` dials a
//! `SocketAddr`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::client::conn::http1::{self, SendRequest};
use hyper::rt::{Read, Write};
use hyper_util::rt::TokioIo;
use nebula_tls::{TlsConnector, TlsError, peer_is_loopback};
use net_guard::{Host, NetworkAccess, Policy, PortPolicy, Resolver, Target};
use rustls_pki_types::ServerName;

use crate::DavError;
use crate::config::{Origin, Scheme};

/// A request sender on one connection.
pub(crate) type Sender = SendRequest<Full<Bytes>>;

/// Longest response head (status line and headers) read.
pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Most response headers.
const MAX_HEADERS: usize = 100;

pub(crate) struct Connector {
    tls: TlsConnector,
    resolver: Arc<dyn Resolver>,
    handshake_timeout: Duration,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("tls", &self.tls)
            .finish_non_exhaustive()
    }
}

fn server_name(host: &Host) -> Result<ServerName<'static>, DavError> {
    match host {
        Host::Ip(ip) => Ok(ServerName::IpAddress((*ip).into())),
        Host::Domain(name) => ServerName::try_from(name.tls_name().to_owned())
            .map_err(|_| DavError::Tls(TlsError::InvalidServerName)),
        _ => Err(DavError::Tls(TlsError::InvalidServerName)),
    }
}

impl Connector {
    pub(crate) fn new(
        tls: TlsConnector,
        resolver: Arc<dyn Resolver>,
        handshake_timeout: Duration,
    ) -> Self {
        Self {
            tls,
            resolver,
            handshake_timeout,
        }
    }

    /// Opens a connection to `origin`, allowed in `network`, on its port
    /// only.
    pub(crate) async fn open(
        &self,
        origin: &Origin,
        network: NetworkAccess,
    ) -> Result<Sender, DavError> {
        let target = Target::new(origin.host.clone(), origin.port);
        let policy = Policy::new(PortPolicy::only(vec![origin.port]), network);
        let guarded = net_guard::connect(&target, &policy, self.resolver.as_ref()).await?;
        let (tcp, _peer, host) = guarded.into_parts();
        match origin.scheme {
            Scheme::Https => {
                let name = server_name(&host)?;
                let tls =
                    tokio::time::timeout(self.handshake_timeout, self.tls.handshake(tcp, name))
                        .await
                        .map_err(|_| DavError::Timeout)??;
                self.handshake(TokioIo::new(tls)).await
            }
            Scheme::Http => {
                let loopback =
                    peer_is_loopback(&tcp).map_err(|e| DavError::Tls(TlsError::Io(e)))?;
                if !loopback {
                    return Err(DavError::PlainHttpNotLoopback);
                }
                self.handshake(TokioIo::new(tcp)).await
            }
        }
    }

    async fn handshake<T>(&self, io: T) -> Result<Sender, DavError>
    where
        T: Read + Write + Unpin + Send + 'static,
    {
        let mut builder = http1::Builder::new();
        builder
            .max_headers(MAX_HEADERS)
            .max_buf_size(MAX_HEAD_BYTES);
        let (sender, connection) =
            tokio::time::timeout(self.handshake_timeout, builder.handshake(io))
                .await
                .map_err(|_| DavError::Timeout)??;
        // Drives the connection; ends when the sender is dropped or the
        // server closes it.
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "dav: connection ended with an error");
            }
        });
        Ok(sender)
    }
}
