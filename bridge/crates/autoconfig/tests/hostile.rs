// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Discovery fed by hostile sources: what it finds passes the same guard
//! as a configured server, and `security/ssrf-vectors.json` holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use nebula_autoconfig::candidate::{Candidate, Refusal, Security, Service, Source};
use nebula_autoconfig::http::{Fetch, FetchError, FetchFuture, GuardedFetch};
use nebula_autoconfig::srv::{SrvFuture, SrvLookup, SrvRecord};
use nebula_autoconfig::{Discovery, Options};
use net_guard::{BlockReason, DnsError, DomainName, Host, LookupFuture, NetworkAccess, Resolver};
use serde::Deserialize;

const PUBLIC: &str = "93.184.216.34";

#[derive(Default)]
struct Dns(HashMap<String, Vec<IpAddr>>);

impl Dns {
    fn with(mut self, name: &str, ip: &str) -> Self {
        self.0
            .entry(name.to_owned())
            .or_default()
            .push(ip.parse().unwrap());
        self
    }
}

impl Resolver for Dns {
    fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async move { self.0.get(name.as_str()).cloned().ok_or(DnsError) })
    }
}

#[derive(Default)]
struct Srv(HashMap<String, Vec<SrvRecord>>);

impl Srv {
    fn with(mut self, name: &str, target: &str, port: u16) -> Self {
        self.0
            .entry(format!("{name}.example.com."))
            .or_default()
            .push(SrvRecord {
                priority: 0,
                weight: 0,
                port,
                target: target.to_owned(),
            });
        self
    }
}

impl SrvLookup for Srv {
    fn srv<'a>(&'a self, name: &'a str) -> SrvFuture<'a> {
        Box::pin(async move { Ok(self.0.get(name).cloned().unwrap_or_default()) })
    }
}

/// Answers from a table and writes down what was asked.
#[derive(Default)]
struct Web {
    pages: HashMap<String, String>,
    asked: Mutex<Vec<String>>,
}

impl Web {
    fn with(mut self, url: &str, body: &str) -> Self {
        self.pages.insert(url.to_owned(), body.to_owned());
        self
    }

    fn answer(&self, url: &str) -> Result<String, FetchError> {
        self.asked.lock().unwrap().push(url.to_owned());
        self.pages.get(url).cloned().ok_or(FetchError::Status(404))
    }
}

impl Fetch for Web {
    fn get<'a>(&'a self, url: &'a str) -> FetchFuture<'a> {
        Box::pin(async move { self.answer(url) })
    }
    fn post_xml<'a>(&'a self, url: &'a str, _body: String) -> FetchFuture<'a> {
        Box::pin(async move { self.answer(url) })
    }
}

fn domain() -> DomainName {
    match "example.com".parse().unwrap() {
        Host::Domain(name) => name,
        other => panic!("{other:?}"),
    }
}

async fn run(dns: Dns, srv: Srv, web: Arc<Web>, options: Options) -> nebula_autoconfig::Report {
    Discovery::new(Arc::new(dns), Arc::new(srv), web, options)
        .run(&domain())
        .await
}

fn imap_file(host: &str, port: u16, socket: &str) -> String {
    format!(
        "<clientConfig><emailProvider><incomingServer type=\"imap\"><hostname>{host}</hostname>\
<port>{port}</port><socketType>{socket}</socketType></incomingServer></emailProvider></clientConfig>"
    )
}

const AUTOCONFIG: &str = "https://autoconfig.example.com/mail/config-v1.1.xml";
const ISPDB: &str = "https://autoconfig.thunderbird.net/v1.1/example.com";

#[tokio::test]
async fn srv_wins_and_sieve_comes_from_its_record() {
    let dns = Dns::default()
        .with("imap.example.com", PUBLIC)
        .with("smtp.example.com", PUBLIC)
        .with("sieve.example.com", PUBLIC);
    let srv = Srv::default()
        .with("_imaps._tcp", "imap.example.com.", 993)
        .with("_submission._tcp", "smtp.example.com.", 587)
        .with("_sieve._tcp", "sieve.example.com.", 4190);
    let web = Arc::new(Web::default());
    let report = run(dns, srv, Arc::clone(&web), Options::default()).await;
    assert_eq!(
        report.get(Service::Imap).unwrap().security,
        Security::ImplicitTls
    );
    assert_eq!(
        report.get(Service::Smtp).unwrap().security,
        Security::StartTls
    );
    let sieve = report.get(Service::Sieve).unwrap();
    assert_eq!(sieve.host.to_string(), "sieve.example.com");
    assert_eq!(sieve.source, Source::Srv);
    // Enough from DNS: nobody else was asked.
    assert!(web.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sieve_is_guessed_next_to_imap_and_ispdb_is_last() {
    let dns = Dns::default().with("imap.example.com", PUBLIC);
    let web = Arc::new(Web::default().with(ISPDB, &imap_file("imap.example.com", 993, "SSL")));
    let report = run(dns, Srv::default(), Arc::clone(&web), Options::default()).await;
    let sieve = report.get(Service::Sieve).unwrap();
    assert_eq!(sieve.source, Source::SameHost);
    assert_eq!(sieve.port.get(), 4190);
    assert_eq!(web.asked.lock().unwrap().last().unwrap(), ISPDB);
}

#[tokio::test]
async fn the_public_database_can_be_left_out() {
    let web = Arc::new(Web::default());
    let options = Options {
        ispdb: false,
        ..Options::default()
    };
    run(Dns::default(), Srv::default(), Arc::clone(&web), options).await;
    assert!(!web.asked.lock().unwrap().iter().any(|url| url == ISPDB));
}

/// One hostile offer each; none may be kept.
#[tokio::test]
async fn hostile_offers_are_refused() {
    let dns = Dns::default()
        .with("private.example.com", "10.0.0.5")
        .with("metadata.example.com", "169.254.169.254")
        .with("mixed.example.com", PUBLIC)
        .with("mixed.example.com", "127.0.0.1")
        .with("imap.example.com", PUBLIC);
    for (host, port, socket, reason) in [
        (
            "127.0.0.1",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "[::1]",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "2130706433",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "private.example.com",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "metadata.example.com",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "mixed.example.com",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::PrivateIp),
        ),
        (
            "gone.example.com",
            993,
            "SSL",
            Refusal::Blocked(BlockReason::Dns),
        ),
        (
            "imap.example.com",
            25,
            "SSL",
            Refusal::Blocked(BlockReason::Port),
        ),
        (
            "imap.example.com",
            8080,
            "SSL",
            Refusal::Blocked(BlockReason::Port),
        ),
        ("imap.example.com", 143, "plain", Refusal::PlainText),
        ("user@imap.example.com", 993, "SSL", Refusal::BadHost),
        ("imap.example.com/x", 993, "SSL", Refusal::BadHost),
    ] {
        let web = Web::default().with(AUTOCONFIG, &imap_file(host, port, socket));
        let dns = Dns(dns.0.clone());
        let report = run(dns, Srv::default(), Arc::new(web), Options::default()).await;
        assert_eq!(report.get(Service::Imap), None, "{host}");
        assert_eq!(report.refused[0].reason, reason, "{host}");
    }
}

#[tokio::test]
async fn a_bad_srv_does_not_stop_the_next_source() {
    let dns = Dns::default()
        .with("private.example.com", "192.168.1.2")
        .with("imap.example.com", PUBLIC);
    let srv = Srv::default().with("_imaps._tcp", "private.example.com.", 993);
    let web = Web::default().with(AUTOCONFIG, &imap_file("imap.example.com", 993, "SSL"));
    let report = run(dns, srv, Arc::new(web), Options::default()).await;
    assert_eq!(
        report.get(Service::Imap).unwrap().source,
        Source::Autoconfig
    );
    assert_eq!(report.refused.len(), 1);
}

#[tokio::test]
async fn private_servers_only_if_the_admin_allows_them() {
    let candidate = || Candidate {
        service: Service::Sieve,
        source: Source::Srv,
        host: "10.0.0.5".to_owned(),
        port: 4190,
        security: Some(Security::StartTls),
    };
    let dns = Dns::default();
    assert!(
        candidate()
            .accept(&dns, NetworkAccess::PublicOnly)
            .await
            .is_err()
    );
    assert!(
        candidate()
            .accept(&dns, NetworkAccess::PrivateAllowed)
            .await
            .is_ok()
    );
    // Link-local stays closed, as for a configured server.
    let metadata = Candidate {
        host: "169.254.169.254".to_owned(),
        ..candidate()
    };
    assert!(
        metadata
            .accept(&dns, NetworkAccess::PrivateAllowed)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dav_is_https_only() {
    let dns = Dns::default().with("dav.example.com", PUBLIC);
    let dav = |port| Candidate {
        service: Service::CalDav,
        source: Source::Srv,
        host: "dav.example.com".to_owned(),
        port,
        security: Some(Security::ImplicitTls),
    };
    assert!(
        dav(443)
            .accept(&dns, NetworkAccess::PublicOnly)
            .await
            .is_ok()
    );
    assert!(
        dav(993)
            .accept(&dns, NetworkAccess::PublicOnly)
            .await
            .is_err()
    );
}

#[derive(Debug, Deserialize)]
struct Vectors {
    url: Vec<UrlVector>,
}

#[derive(Debug, Deserialize)]
struct UrlVector {
    id: String,
    input: String,
    dns: Option<VectorDns>,
    expect: String,
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct VectorDns {
    #[serde(rename = "A", default)]
    a: Vec<String>,
    #[serde(rename = "AAAA", default)]
    aaaa: Vec<String>,
}

/// Answers every name with the vector's addresses.
struct Fixed(Vec<IpAddr>);

impl Resolver for Fixed {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async move {
            if self.0.is_empty() {
                Err(DnsError)
            } else {
                Ok(self.0.clone())
            }
        })
    }
}

/// Every vector that must be blocked is blocked on the two ways in: as a
/// URL to fetch, and as a host offered as a mail server.
#[tokio::test]
async fn ssrf_vectors() {
    let vectors: Vectors =
        serde_json::from_str(include_str!("../../../../security/ssrf-vectors.json")).unwrap();
    let mut blocked = 0;
    for vector in vectors.url.iter().filter(|v| v.expect == "block") {
        let dns = vector.dns.as_ref().map_or_else(Vec::new, |dns| {
            dns.a
                .iter()
                .chain(&dns.aaaa)
                .map(|ip| ip.parse().unwrap())
                .collect()
        });
        let resolver = Arc::new(Fixed(dns));
        let fetch = GuardedFetch::new(resolver.clone()).unwrap();
        let result = fetch.get(&vector.input).await;
        assert!(
            matches!(result, Err(FetchError::Blocked(_))),
            "{}: {result:?}",
            vector.id
        );
        // Only the vectors about where the host points apply to a host.
        if !matches!(vector.reason.as_deref(), Some("private-ip" | "dns")) {
            continue;
        }
        let host = vector
            .input
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .unwrap();
        let host = host.rsplit('@').next().unwrap();
        let host = match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => host,
        };
        let offer = Candidate {
            service: Service::Imap,
            source: Source::Srv,
            host: host.to_owned(),
            port: 993,
            security: Some(Security::ImplicitTls),
        };
        let outcome = offer
            .accept(resolver.as_ref(), NetworkAccess::PublicOnly)
            .await;
        assert!(outcome.is_err(), "{}: {outcome:?}", vector.id);
        blocked += 1;
    }
    assert!(blocked > 50, "only {blocked} vectors ran");
}
