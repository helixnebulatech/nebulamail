// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Finds the mail servers of a domain, for the admin:
//! DNS SRV (RFC 6186, RFC 8314, RFC 6764 and `_sieve._tcp` from RFC 5804),
//! Thunderbird's autoconfig (the domain's own file, then its public
//! database, ISPDB) and Autodiscover.
//!
//! All of it comes from strangers, so:
//!
//! - every host and port found passes `net-guard` as a configured server
//!   would: the closed port list, every address checked ([`candidate`]).
//!   HTTP goes to https on port 443 only, follows no redirect and reads
//!   256 KiB at most ([`http`]);
//! - plain-text servers are refused;
//! - no password is sent anywhere: [`check`] only connects and looks at
//!   the greeting and the certificate;
//! - the result is a proposal the admin copies into the configuration. The
//!   bridge never changes servers on its own: without DNSSEC, whoever can
//!   forge DNS answers would get every user's password.

#![forbid(unsafe_code)]

pub mod candidate;
pub mod check;
pub mod http;
pub mod srv;

mod autodiscover;
mod thunderbird;

use std::sync::Arc;

use net_guard::{DomainName, NetworkAccess, Resolver};

use crate::candidate::{Candidate, Endpoint, Refused, Security, Service, Source};
use crate::http::Fetch;
use crate::srv::{SrvLookup, lookup_all};

/// What to ask and which networks count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Ask Thunderbird's public database. It learns the domain.
    pub ispdb: bool,
    /// Servers on private addresses are fine (`private_network = true`).
    pub network: NetworkAccess,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ispdb: true,
            network: NetworkAccess::PublicOnly,
        }
    }
}

/// What was found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// At most one per service, the best found.
    pub found: Vec<Endpoint>,
    /// What was offered and refused, and why.
    pub refused: Vec<Refused>,
}

impl Report {
    /// The endpoint of `service`, if any.
    #[must_use]
    pub fn get(&self, service: Service) -> Option<&Endpoint> {
        self.found.iter().find(|e| e.service == service)
    }

    fn has_mail(&self) -> bool {
        self.get(Service::Imap).is_some() && self.get(Service::Smtp).is_some()
    }
}

/// Finds a domain's servers.
pub struct Discovery {
    resolver: Arc<dyn Resolver>,
    srv: Arc<dyn SrvLookup>,
    fetch: Arc<dyn Fetch>,
    options: Options,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Discovery {
    /// With these lookups (the tests' are fake).
    #[must_use]
    pub fn new(
        resolver: Arc<dyn Resolver>,
        srv: Arc<dyn SrvLookup>,
        fetch: Arc<dyn Fetch>,
        options: Options,
    ) -> Self {
        Self {
            resolver,
            srv,
            fetch,
            options,
        }
    }

    /// Keeps what passes the guard: the first of each service wins.
    async fn take(&self, report: &mut Report, candidates: Vec<Candidate>) {
        for candidate in candidates {
            if report.get(candidate.service).is_some() {
                continue;
            }
            let (service, source) = (candidate.service, candidate.source);
            let host = candidate.host.clone();
            match candidate
                .accept(self.resolver.as_ref(), self.options.network)
                .await
            {
                Ok(endpoint) => report.found.push(endpoint),
                Err(reason) => {
                    tracing::info!(%host, %reason, "autoconfig: refused");
                    report.refused.push(Refused {
                        service,
                        source,
                        host,
                        reason,
                    });
                }
            }
        }
    }

    /// Asks every source, the domain's own first, and stops once it has
    /// IMAP and SMTP (fewer requests to strangers).
    pub async fn run(&self, domain: &DomainName) -> Report {
        let mut report = Report::default();
        self.take(&mut report, lookup_all(self.srv.as_ref(), domain).await)
            .await;
        for url in thunderbird::own_urls(domain) {
            if report.has_mail() {
                break;
            }
            let found = self.thunderbird(&url, domain, Source::Autoconfig).await;
            self.take(&mut report, found).await;
        }
        if !report.has_mail() {
            let found = autodiscover::ask(self.fetch.as_ref(), domain).await;
            self.take(&mut report, found).await;
        }
        if !report.has_mail() && self.options.ispdb {
            let url = thunderbird::ispdb_url(domain);
            let found = self.thunderbird(&url, domain, Source::Ispdb).await;
            self.take(&mut report, found).await;
        }
        // Neither Thunderbird's file nor Autodiscover name ManageSieve: its
        // usual place is next to IMAP. `check` tells whether it is there.
        if report.get(Service::Sieve).is_none()
            && let Some(imap) = report.get(Service::Imap)
        {
            let guess = Candidate {
                service: Service::Sieve,
                source: Source::SameHost,
                host: imap.host.to_string(),
                port: net_guard::Port::SIEVE.get(),
                security: Some(Security::StartTls),
            };
            self.take(&mut report, vec![guess]).await;
        }
        report
    }

    async fn thunderbird(&self, url: &str, domain: &DomainName, source: Source) -> Vec<Candidate> {
        match self.fetch.get(url).await {
            Ok(body) => thunderbird::parse(&body, domain, source),
            Err(error) => {
                tracing::debug!(%error, "autoconfig: nothing there");
                Vec::new()
            }
        }
    }
}
