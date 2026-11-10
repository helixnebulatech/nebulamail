// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a source offers ([`Candidate`], text from a stranger) and what
//! passed the guard ([`Endpoint`]).

use std::fmt;

use net_guard::{BlockReason, Host, NetworkAccess, Port, PortPolicy, Resolver};

/// A kind of server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Service {
    /// IMAP.
    Imap,
    /// SMTP submission.
    Smtp,
    /// ManageSieve.
    Sieve,
    /// CalDAV.
    CalDav,
    /// CardDAV.
    CardDav,
}

impl Service {
    const fn is_http(self) -> bool {
        matches!(self, Self::CalDav | Self::CardDav)
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Imap => "IMAP",
            Self::Smtp => "SMTP",
            Self::Sieve => "ManageSieve",
            Self::CalDav => "CalDAV",
            Self::CardDav => "CardDAV",
        })
    }
}

/// How the connection is encrypted. There is no plain-text choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Security {
    /// TLS from the first byte.
    ImplicitTls,
    /// STARTTLS.
    StartTls,
}

/// Where a server came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// A DNS SRV record.
    Srv,
    /// The domain's own Thunderbird autoconfig file.
    Autoconfig,
    /// Autodiscover.
    Autodiscover,
    /// Thunderbird's public database.
    Ispdb,
    /// Not named by anyone: the usual port on the IMAP host.
    SameHost,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Srv => "DNS SRV",
            Self::Autoconfig => "autoconfig",
            Self::Autodiscover => "Autodiscover",
            Self::Ispdb => "ISPDB",
            Self::SameHost => "same host as IMAP",
        })
    }
}

/// A server someone offered, unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What it is.
    pub service: Service,
    /// Who said so.
    pub source: Source,
    /// The host, as offered.
    pub host: String,
    /// The port, as offered.
    pub port: u16,
    /// `None`: plain text.
    pub security: Option<Security>,
}

/// Why a candidate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Refusal {
    /// `net-guard` said no.
    #[error("{0}")]
    Blocked(BlockReason),
    /// The source offers it without encryption.
    #[error("no encryption")]
    PlainText,
    /// Not a host name.
    #[error("not a host name")]
    BadHost,
}

/// A candidate that was refused, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// What it was.
    pub service: Service,
    /// Who offered it.
    pub source: Source,
    /// The host, as offered (for showing only).
    pub host: String,
    /// Why not.
    pub reason: Refusal,
}

/// A server that passed the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// What it is.
    pub service: Service,
    /// Who offered it.
    pub source: Source,
    /// The host, normalised.
    pub host: Host,
    /// A port of the closed list.
    pub port: Port,
    /// How to connect.
    pub security: Security,
}

impl Candidate {
    /// The same checks as a server in the configuration: the closed list
    /// of ports (only https for DAV), and every address the name has.
    ///
    /// # Errors
    /// Why it isn't acceptable.
    pub async fn accept(
        self,
        resolver: &dyn Resolver,
        network: NetworkAccess,
    ) -> Result<Endpoint, Refusal> {
        let security = self.security.ok_or(Refusal::PlainText)?;
        let host: Host = self.host.parse().map_err(|_| Refusal::BadHost)?;
        let port = Port::try_from(self.port).map_err(|_| Refusal::Blocked(BlockReason::Port))?;
        let ports = if self.service.is_http() {
            PortPolicy::HTTPS
        } else {
            PortPolicy::MAIL
        };
        ports.check(port).map_err(Refusal::Blocked)?;
        net_guard::resolve(&host, network, resolver)
            .await
            .map_err(Refusal::Blocked)?;
        Ok(Endpoint {
            service: self.service,
            source: self.source,
            host,
            port,
            security,
        })
    }
}
