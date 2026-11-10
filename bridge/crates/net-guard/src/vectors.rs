// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `security/ssrf-vectors.json` against the Rust guard.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use serde::Deserialize;
use tokio::net::{TcpListener, TcpStream};

use crate::connect::connect_with;
use crate::resolve::testing::FakeDns;
use crate::{NetworkAccess, Policy, Port, PortPolicy, Target, UrlPolicy, guard_url};

#[derive(Debug, Deserialize)]
struct Vectors {
    version: u32,
    url: Vec<UrlVector>,
    fetch: Vec<FetchVector>,
}

#[derive(Debug, Deserialize)]
struct UrlVector {
    id: String,
    input: String,
    dns: Option<Dns>,
    expect: Expect,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Dns {
    #[serde(rename = "A", default)]
    a: Vec<String>,
    #[serde(rename = "AAAA", default)]
    aaaa: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FetchVector {
    id: String,
    #[serde(rename = "dnsSequence")]
    dns_sequence: Option<Vec<Vec<String>>>,
    expect: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Expect {
    Allow,
    Block,
}

const VECTORS: &str = include_str!("../../../../security/ssrf-vectors.json");

fn vectors() -> Vectors {
    serde_json::from_str(VECTORS).unwrap()
}

#[test]
fn vectors_file_is_valid() {
    let v = vectors();
    assert_eq!(v.version, 1);

    let mut ids = HashSet::new();
    for id in v
        .url
        .iter()
        .map(|u| &u.id)
        .chain(v.fetch.iter().map(|f| &f.id))
    {
        assert!(ids.insert(id), "duplicate vector id {id}");
    }
    for u in &v.url {
        if u.expect == Expect::Block {
            assert!(u.reason.is_some(), "blocked vector {} has no reason", u.id);
        }
    }
}

/// Every `url` vector gives the same allow/block and reason as TypeScript.
/// Without `dns` in the vector the guard must decide without resolving.
#[tokio::test]
async fn url_vectors() {
    let mut failures = Vec::new();
    for vector in vectors().url {
        let dns = match &vector.dns {
            Some(dns) => {
                let answer: Vec<&str> = dns.a.iter().chain(&dns.aaaa).map(String::as_str).collect();
                FakeDns::answering(&[&answer])
            }
            None => FakeDns::failing(),
        };
        let result = guard_url(&vector.input, &dns, &UrlPolicy::PROXY).await;
        let got = match &result {
            Ok(_) => "allow".to_owned(),
            Err(reason) => format!("block:{reason}"),
        };
        let want = match vector.expect {
            Expect::Allow => "allow".to_owned(),
            Expect::Block => format!("block:{}", vector.reason.as_deref().unwrap_or("?")),
        };
        if got != want {
            failures.push(format!("{}: got {got}, want {want}", vector.id));
        }
        if vector.dns.is_none() && !dns.calls().is_empty() {
            failures.push(format!(
                "{}: resolver called without dns in the vector",
                vector.id
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// `rebinding`: the first answer is validated and dialed; the resolver is
/// asked once, so the second (private) answer is never used. The dial is
/// redirected to a local listener so the test needs no network.
#[tokio::test]
async fn rebinding_vector() {
    let vector = vectors()
        .fetch
        .into_iter()
        .find(|f| f.id == "rebinding")
        .unwrap();
    assert_eq!(vector.expect, "connect-to-validated-ip");
    let sequence = vector.dns_sequence.unwrap();
    let answers: Vec<Vec<&str>> = sequence
        .iter()
        .map(|ips| ips.iter().map(String::as_str).collect())
        .collect();
    let answers: Vec<&[&str]> = answers.iter().map(Vec::as_slice).collect();
    let dns = FakeDns::answering(&answers);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let port = Port::try_from(local.port()).unwrap();
    let dialed = Mutex::new(Vec::new());
    let dial = |address: SocketAddr| {
        dialed.lock().unwrap().push(address);
        TcpStream::connect(local)
    };
    let target = Target::new("rebind.example.com".parse().unwrap(), port);
    let policy = Policy::new(PortPolicy::only(vec![port]), NetworkAccess::PublicOnly);

    let stream = connect_with(&target, &policy, &dns, dial).await.unwrap();

    let first = sequence[0][0].parse().unwrap();
    assert_eq!(stream.peer().get(), first);
    assert_eq!(
        *dialed.lock().unwrap(),
        [SocketAddr::new(first, port.get())]
    );
    assert_eq!(dns.calls(), ["rebind.example.com"]);
}

/// `fetch` vectors that apply to connecting and run elsewhere. Mixed
/// public/private DNS, AAAA-only private and a mapped IPv4 in AAAA are
/// `url` vectors, run above. The HTTP ones run against the CalDAV/CardDAV
/// client, the first Rust HTTP client, with a hostile fake server
/// (`crates/dav/tests/hostile.rs`, which checks each `reason` too).
const COVERED_ELSEWHERE: &[(&str, &str)] = &[
    ("rebinding", "rebinding_vector, above"),
    (
        "env-proxy-ignored",
        "connect dials a SocketAddr with TcpStream; no code reads proxy variables",
    ),
    ("redirect-to-private", "nebula-dav tests/hostile.rs"),
    ("redirect-to-metadata-name", "nebula-dav tests/hostile.rs"),
    ("redirect-scheme-change", "nebula-dav tests/hostile.rs"),
    ("redirect-loop", "nebula-dav tests/hostile.rs"),
    (
        "not-an-image",
        "nebula-dav tests/hostile.rs (a multistatus or event of the wrong type)",
    ),
    ("too-large", "nebula-dav tests/hostile.rs"),
    ("gzip-bomb", "nebula-dav tests/hostile.rs"),
    ("slow-loris", "nebula-dav tests/hostile.rs"),
];

/// Vectors with no Rust code to run them against yet (none today).
const SKIPPED: &[(&str, &str)] = &[];

/// Every `fetch` vector is either covered or explicitly skipped, so a new
/// vector cannot be forgotten.
#[test]
fn fetch_vectors_are_all_accounted_for() {
    let known: HashSet<&str> = COVERED_ELSEWHERE
        .iter()
        .chain(SKIPPED)
        .map(|(id, _)| *id)
        .collect();
    let ids: HashSet<String> = vectors().fetch.into_iter().map(|f| f.id).collect();
    for id in &ids {
        assert!(
            known.contains(id.as_str()),
            "fetch vector {id} is neither run nor skipped"
        );
    }
    for id in &known {
        assert!(
            ids.contains(*id),
            "{id} is listed but not in the vectors file"
        );
    }
}
