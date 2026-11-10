// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The upstream IMAP server and the tuning of the mail layer. Built by the
//! binary from the TOML configuration.
//!
//! ```
//! use net_guard::{Port, PortPolicy};
//! use nebula_mail::{CertificateCheck, ImapServer, MailConfig, TlsMode};
//!
//! // Stalwart in Docker: implicit TLS on 1993, self-signed, on loopback.
//! let server = ImapServer::new("127.0.0.1".parse().unwrap(), Port::try_from(1993).unwrap(), TlsMode::Implicit)
//!     .with_certificates(CertificateCheck::InsecureLoopbackOnly)
//!     .with_private_network(true)
//!     .with_ports(PortPolicy::only(vec![Port::try_from(1993).unwrap()]));
//! let config = MailConfig::new(server);
//! assert_eq!(config.batch_size, 500);
//! ```

use std::time::Duration;

use nebula_imap::{Limits, Tls, TlsVerify};
use net_guard::{Host, NetworkAccess, Policy, Port, PortPolicy, Target};
use rustls_pki_types::ServerName;

use crate::MailError;

/// How TLS starts. There is no plaintext mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TlsMode {
    /// TLS from the first byte (port 993). Preferred.
    Implicit,
    /// Plain greeting, then STARTTLS, which is required (port 143).
    StartTls,
}

/// How the server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CertificateCheck {
    /// The system verifier and trust store.
    #[default]
    Platform,
    /// No check, for a development server with a self-signed certificate
    /// (`tls_verify = false`). Refused unless the server is on loopback.
    InsecureLoopbackOnly,
}

/// The IMAP server of every account (one server in this phase).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImapServer {
    /// Host name (or IP) to connect to; also the TLS name.
    pub host: Host,
    /// Port.
    pub port: Port,
    /// Implicit TLS or required STARTTLS.
    pub tls: TlsMode,
    /// Certificate check.
    pub certificates: CertificateCheck,
    /// `private_network = true`: the server may be on a local network.
    pub network: NetworkAccess,
    /// Ports `net-guard` accepts. [`PortPolicy::MAIL`] by default; an admin
    /// may allow another port for this server.
    pub ports: PortPolicy,
    /// Size limits and timeouts of each connection.
    pub limits: Limits,
}

impl ImapServer {
    /// A server with safe defaults: platform certificate checks, public
    /// addresses only, the standard mail ports and the default limits.
    #[must_use]
    pub fn new(host: Host, port: Port, tls: TlsMode) -> Self {
        Self {
            host,
            port,
            tls,
            certificates: CertificateCheck::default(),
            network: NetworkAccess::PublicOnly,
            ports: PortPolicy::MAIL,
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

    /// Sets the allowed ports.
    #[must_use]
    pub fn with_ports(mut self, ports: PortPolicy) -> Self {
        self.ports = ports;
        self
    }

    /// Sets the connection limits.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub(crate) fn target(&self) -> Target {
        Target::new(self.host.clone(), self.port)
    }

    pub(crate) fn policy(&self) -> Policy {
        Policy::new(self.ports.clone(), self.network)
    }

    pub(crate) fn tls(&self) -> Tls {
        match self.tls {
            TlsMode::Implicit => Tls::Implicit,
            TlsMode::StartTls => Tls::StartTls,
        }
    }

    pub(crate) fn verify(&self) -> TlsVerify {
        match self.certificates {
            CertificateCheck::Platform => TlsVerify::Platform,
            CertificateCheck::InsecureLoopbackOnly => TlsVerify::InsecureLoopbackOnly,
        }
    }

    /// The name for SNI and the certificate: the configured host, never
    /// something from DNS.
    pub(crate) fn server_name(&self) -> Result<ServerName<'static>, MailError> {
        let invalid = || MailError::Config(nebula_imap::Error::InvalidServerName);
        match &self.host {
            Host::Ip(ip) => Ok(ServerName::IpAddress((*ip).into())),
            Host::Domain(name) => {
                ServerName::try_from(name.tls_name().to_owned()).map_err(|_| invalid())
            }
            _ => Err(invalid()),
        }
    }
}

/// Settings of the mail layer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MailConfig {
    /// The IMAP server.
    pub server: ImapServer,
    /// Messages per FETCH batch in the first sync.
    pub batch_size: usize,
    /// How often the mailboxes other than INBOX are checked with STATUS.
    pub status_interval: Duration,
    /// Bytes of raw messages kept in memory (all accounts together).
    pub raw_cache_bytes: u64,
    /// Largest message kept in the cache (bigger ones are fetched each
    /// time). Also where downloads switch to streaming: bigger messages
    /// and their parts are fetched in pieces, never whole.
    pub raw_cache_max_item: u64,
    /// Streamed part downloads use `BINARY` (the server decodes) when the
    /// server has it. Off: always `BODY[part]`, decoded by the bridge.
    pub binary_downloads: bool,
    /// How long an email without locations keeps its id (to come back
    /// after a move by another client or a UIDVALIDITY reset).
    pub orphan_ttl: Duration,
    /// Memory for everything held whole (downloads, `Email/get` of one
    /// message, creates, imports, submissions), all accounts together.
    pub whole_memory: u64,
    /// How long an operation waits for its share of [`Self::whole_memory`]
    /// before failing with [`Unavailable::Busy`](crate::Unavailable).
    pub whole_wait: Duration,
    /// How long a load of a message bigger than the cache waits for the
    /// account's turn (one at a time per account).
    pub whole_turn_wait: Duration,
}

impl MailConfig {
    /// The defaults: batches of 500, STATUS every 2 minutes, 64 MiB of
    /// cached messages (at most 8 MiB each), orphans kept for 1 day, 256 MiB
    /// for whole messages (waited for 30 s; the account's turn 120 s).
    #[must_use]
    pub fn new(server: ImapServer) -> Self {
        Self {
            server,
            batch_size: 500,
            status_interval: Duration::from_mins(2),
            raw_cache_bytes: 64 * 1024 * 1024,
            raw_cache_max_item: 8 * 1024 * 1024,
            binary_downloads: true,
            orphan_ttl: Duration::from_hours(24),
            whole_memory: 256 * 1024 * 1024,
            whole_wait: Duration::from_secs(30),
            whole_turn_wait: Duration::from_mins(2),
        }
    }
}
