// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Does a found server answer? No password is ever sent.

use std::sync::Arc;
use std::time::Duration;

use nebula_managesieve::{CertificateCheck, Connector, SieveServer, Tls};
use nebula_tls::{TlsConnector, TlsVerify};
use net_guard::{Host, NetworkAccess, Policy, PortPolicy, Resolver, Target};
use rustls_pki_types::ServerName;
use tokio::io::AsyncReadExt as _;

use crate::candidate::{Endpoint, Security, Service};

const TIME_LIMIT: Duration = Duration::from_secs(15);

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Checked {
    /// It speaks the protocol or has a valid certificate.
    Works,
    /// It greets, but STARTTLS wasn't tried. The bridge requires it when it
    /// connects; this check doesn't look further without a login.
    Answers,
    /// It didn't work; the reason, for the admin.
    Failed(String),
}

fn server_name(host: &Host) -> Option<ServerName<'static>> {
    match host {
        Host::Ip(ip) => Some(ServerName::IpAddress((*ip).into())),
        Host::Domain(name) => ServerName::try_from(name.tls_name().to_owned()).ok(),
        _ => None,
    }
}

async fn sieve(
    endpoint: &Endpoint,
    resolver: &Arc<dyn Resolver>,
    network: NetworkAccess,
) -> Checked {
    let tls = match endpoint.security {
        Security::ImplicitTls => Tls::Implicit,
        Security::StartTls => Tls::StartTls,
    };
    let server = SieveServer::new(endpoint.host.clone(), endpoint.port, tls)
        .with_certificates(CertificateCheck::Platform)
        .with_private_network(network == NetworkAccess::PrivateAllowed);
    let connector = match Connector::new(server, Arc::clone(resolver)) {
        Ok(connector) => connector,
        Err(error) => return Checked::Failed(error.to_string()),
    };
    match connector.capabilities().await {
        Ok(_) => Checked::Works,
        Err(error) => Checked::Failed(error.to_string()),
    }
}

async fn plain(
    endpoint: &Endpoint,
    resolver: &dyn Resolver,
    network: NetworkAccess,
    tls: &TlsConnector,
) -> Checked {
    let ports = if matches!(endpoint.service, Service::CalDav | Service::CardDav) {
        PortPolicy::HTTPS
    } else {
        PortPolicy::MAIL
    };
    let target = Target::new(endpoint.host.clone(), endpoint.port);
    let guarded = match net_guard::connect(&target, &Policy::new(ports, network), resolver).await {
        Ok(guarded) => guarded,
        Err(error) => return Checked::Failed(error.reason().to_string()),
    };
    let (mut tcp, _peer, host) = guarded.into_parts();
    match endpoint.security {
        Security::ImplicitTls => {
            let Some(name) = server_name(&host) else {
                return Checked::Failed("no TLS name".to_owned());
            };
            match tls.handshake(tcp, name).await {
                Ok(_) => Checked::Works,
                Err(error) => Checked::Failed(error.to_string()),
            }
        }
        Security::StartTls => {
            let mut first = [0_u8; 1];
            match tcp.read(&mut first).await {
                Ok(1..) => Checked::Answers,
                Ok(0) | Err(_) => Checked::Failed("no greeting".to_owned()),
            }
        }
    }
}

/// Connects to `endpoint`, through the guard again, and looks.
pub async fn check(
    endpoint: &Endpoint,
    resolver: &Arc<dyn Resolver>,
    network: NetworkAccess,
) -> Checked {
    let attempt = async {
        if endpoint.service == Service::Sieve {
            return sieve(endpoint, resolver, network).await;
        }
        let Ok(tls) = TlsConnector::new(TlsVerify::Platform, &[]) else {
            return Checked::Failed("TLS setup".to_owned());
        };
        plain(endpoint, resolver.as_ref(), network, &tls).await
    };
    tokio::time::timeout(TIME_LIMIT, attempt)
        .await
        .unwrap_or_else(|_| Checked::Failed("no answer in time".to_owned()))
}
