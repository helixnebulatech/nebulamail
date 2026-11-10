// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! DNS SRV: RFC 6186 (`_imap`, `_submission`), RFC 8314 (`_imaps`,
//! `_submissions`), RFC 6764 (`_caldavs`, `_carddavs`) and RFC 5804
//! (`_sieve`).

use std::future::Future;
use std::pin::Pin;

use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::RData;
use net_guard::{DNS_TIMEOUT, DnsError, DomainName};

use crate::candidate::{Candidate, Security, Service, Source};

/// One SRV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    /// Lower is tried first.
    pub priority: u16,
    /// Higher is preferred among the same priority.
    pub weight: u16,
    /// The port.
    pub port: u16,
    /// The host; `.` means "there is no such service here".
    pub target: String,
}

/// The future of [`SrvLookup::srv`].
pub type SrvFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<SrvRecord>, DnsError>> + Send + 'a>>;

/// Looks up SRV records. Injected, so tests can use a fake DNS.
pub trait SrvLookup: Send + Sync {
    /// The records of `name`.
    fn srv<'a>(&'a self, name: &'a str) -> SrvFuture<'a>;
}

/// The system's DNS.
#[derive(Clone)]
pub struct SystemSrv {
    inner: TokioResolver,
}

impl std::fmt::Debug for SystemSrv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemSrv").finish_non_exhaustive()
    }
}

impl SystemSrv {
    /// From the system configuration. Needs a Tokio runtime.
    ///
    /// # Errors
    /// [`DnsError`] if it cannot be read.
    pub fn new() -> Result<Self, DnsError> {
        let inner = TokioResolver::builder_tokio()
            .map_err(|_| DnsError)?
            .build()
            .map_err(|_| DnsError)?;
        Ok(Self { inner })
    }
}

impl SrvLookup for SystemSrv {
    fn srv<'a>(&'a self, name: &'a str) -> SrvFuture<'a> {
        Box::pin(async move {
            let answer = tokio::time::timeout(DNS_TIMEOUT, self.inner.srv_lookup(name))
                .await
                .map_err(|_| DnsError)?
                .map_err(|_| DnsError)?;
            Ok(answer
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    RData::SRV(srv) => Some(SrvRecord {
                        priority: srv.priority,
                        weight: srv.weight,
                        port: srv.port,
                        target: srv.target.to_utf8(),
                    }),
                    _ => None,
                })
                .collect())
        })
    }
}

/// Best first: `_imaps` before `_imap` and so on (RFC 8314 §5.1).
const NAMES: &[(&str, Service, Security)] = &[
    ("_imaps._tcp", Service::Imap, Security::ImplicitTls),
    ("_imap._tcp", Service::Imap, Security::StartTls),
    ("_submissions._tcp", Service::Smtp, Security::ImplicitTls),
    ("_submission._tcp", Service::Smtp, Security::StartTls),
    ("_sieve._tcp", Service::Sieve, Security::StartTls),
    ("_caldavs._tcp", Service::CalDav, Security::ImplicitTls),
    ("_carddavs._tcp", Service::CardDav, Security::ImplicitTls),
];

/// The usable records of one name, best first.
fn sorted(mut records: Vec<SrvRecord>) -> Vec<SrvRecord> {
    records.retain(|record| record.target != ".");
    records.sort_by_key(|record| (record.priority, std::cmp::Reverse(record.weight)));
    records
}

/// Every candidate SRV records offer for `domain`, best first.
pub(crate) async fn lookup_all(srv: &dyn SrvLookup, domain: &DomainName) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (prefix, service, security) in NAMES {
        // No record and a failed lookup are the same here: nothing offered.
        let records = srv
            .srv(&format!("{prefix}.{}.", domain.as_str()))
            .await
            .unwrap_or_default();
        for record in sorted(records) {
            candidates.push(Candidate {
                service: *service,
                source: Source::Srv,
                host: record
                    .target
                    .strip_suffix('.')
                    .unwrap_or(&record.target)
                    .to_owned(),
                port: record.port,
                security: Some(*security),
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(priority: u16, weight: u16, target: &str) -> SrvRecord {
        SrvRecord {
            priority,
            weight,
            port: 993,
            target: target.to_owned(),
        }
    }

    #[test]
    fn priority_then_weight_and_no_service_marker() {
        let got = sorted(vec![
            record(20, 0, "c."),
            record(10, 1, "b."),
            record(10, 5, "a."),
            record(0, 0, "."),
        ]);
        let names: Vec<_> = got.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(names, ["a.", "b.", "c."]);
    }
}
