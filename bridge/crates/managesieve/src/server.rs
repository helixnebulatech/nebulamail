// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The configured ManageSieve server and how to reach it
//! (`net-guard`, then TLS, then login).

use std::sync::Arc;

use nebula_tls::{TlsConnector, TlsVerify};
use net_guard::{Host, NetworkAccess, Policy, Port, PortPolicy, Resolver, Target};
use rustls_pki_types::ServerName;
use secrecy::SecretString;

use crate::proto::Capabilities;
use crate::{Authenticated, Client, Error, Limits};

/// How TLS starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Tls {
    /// TLS from the first byte.
    Implicit,
    /// Plain greeting, then STARTTLS, which is mandatory: if the server does
    /// not offer it, we do not connect. The usual mode on port 4190.
    #[default]
    StartTls,
}

/// How the server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CertificateCheck {
    /// The system verifier and trust store.
    #[default]
    Platform,
    /// No check, for a development server with a self-signed certificate.
    /// Refused unless the server is on loopback.
    InsecureLoopbackOnly,
}

/// The ManageSieve server of every account.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SieveServer {
    /// Host name (or IP) to connect to; also the TLS name.
    pub host: Host,
    /// Port (4190 by default).
    pub port: Port,
    /// Implicit TLS or required STARTTLS.
    pub tls: Tls,
    /// Certificate check.
    pub certificates: CertificateCheck,
    /// Whether the server may be on a private network.
    pub network: NetworkAccess,
    /// Ports `net-guard` accepts: only the configured one by default.
    pub ports: PortPolicy,
    /// Size limits and timeouts.
    pub limits: Limits,
}

impl SieveServer {
    /// A server with safe defaults: platform certificate checks, public
    /// addresses only, only this port, the default limits.
    #[must_use]
    pub fn new(host: Host, port: Port, tls: Tls) -> Self {
        Self {
            host,
            port,
            tls,
            certificates: CertificateCheck::default(),
            network: NetworkAccess::PublicOnly,
            ports: PortPolicy::only(vec![port]),
            limits: Limits::default(),
        }
    }

    /// Sets the certificate check.
    #[must_use]
    pub fn with_certificates(mut self, certificates: CertificateCheck) -> Self {
        self.certificates = certificates;
        self
    }

    /// Allows (or not) a server on a private network (`private_network`).
    #[must_use]
    pub fn with_private_network(mut self, allowed: bool) -> Self {
        self.network = if allowed {
            NetworkAccess::PrivateAllowed
        } else {
            NetworkAccess::PublicOnly
        };
        self
    }

    /// Sets the limits.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    fn server_name(&self) -> Result<ServerName<'static>, Error> {
        match &self.host {
            Host::Ip(ip) => Ok(ServerName::IpAddress((*ip).into())),
            Host::Domain(name) => ServerName::try_from(name.tls_name().to_owned())
                .map_err(|_| Error::InvalidServerName),
            _ => Err(Error::InvalidServerName),
        }
    }
}

/// A user name and password. Neither is ever printed.
pub struct Credentials {
    username: String,
    password: SecretString,
}

impl Credentials {
    /// Creates credentials.
    #[must_use]
    pub fn new(username: impl Into<String>, password: SecretString) -> Self {
        Self {
            username: username.into(),
            password,
        }
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials { .. }")
    }
}

/// Everything needed to open logged-in sessions to the configured server.
/// Cheap to clone.
#[derive(Clone)]
pub struct Connector {
    server: Arc<SieveServer>,
    server_name: ServerName<'static>,
    tls: TlsConnector,
    resolver: Arc<dyn Resolver>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl Connector {
    /// Builds the TLS configuration for `server`.
    ///
    /// # Errors
    ///
    /// The host is no valid TLS name, or TLS could not be set up.
    pub fn new(server: SieveServer, resolver: Arc<dyn Resolver>) -> Result<Self, Error> {
        let verify = match server.certificates {
            CertificateCheck::Platform => TlsVerify::Platform,
            CertificateCheck::InsecureLoopbackOnly => TlsVerify::InsecureLoopbackOnly,
        };
        let tls = TlsConnector::new(verify, &[]).map_err(|error| match error {
            nebula_tls::TlsError::Config(error) => Error::TlsConfig(error),
            other => Error::Io(std::io::Error::other(other)),
        })?;
        let server_name = server.server_name()?;
        Ok(Self {
            server: Arc::new(server),
            server_name,
            tls,
            resolver,
        })
    }

    /// The configured server.
    #[must_use]
    pub fn server(&self) -> &SieveServer {
        &self.server
    }

    /// Connects and returns what the server announces (over TLS), without
    /// logging in: for checking a server.
    ///
    /// # Errors
    ///
    /// Anything from the guard, TLS or the protocol.
    pub async fn capabilities(&self) -> Result<Capabilities, Error> {
        let server = &self.server;
        let stream = net_guard::connect(
            &Target::new(server.host.clone(), server.port),
            &Policy::new(server.ports.clone(), server.network),
            self.resolver.as_ref(),
        )
        .await?;
        let client = Client::connect(
            stream.into_stream(),
            self.server_name.clone(),
            server.tls,
            &self.tls,
            server.limits,
        )
        .await?;
        Ok(client.capabilities().clone())
    }

    /// Connects through `net-guard` (port policy, every resolved address
    /// checked, connection pinned to the validated IP), starts TLS and logs
    /// in with `PLAIN`.
    ///
    /// # Errors
    ///
    /// Anything from the guard, TLS, the protocol or the login.
    pub async fn login(&self, credentials: &Credentials) -> Result<Client<Authenticated>, Error> {
        let server = &self.server;
        let stream = net_guard::connect(
            &Target::new(server.host.clone(), server.port),
            &Policy::new(server.ports.clone(), server.network),
            self.resolver.as_ref(),
        )
        .await?;
        let client = Client::connect(
            stream.into_stream(),
            self.server_name.clone(),
            server.tls,
            &self.tls,
            server.limits,
        )
        .await?;
        client
            .login(&credentials.username, &credentials.password)
            .await
    }
}
