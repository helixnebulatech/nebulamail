// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The `url` vectors of `security/ssrf-vectors.json` as
//! push endpoints, with the strictest policy that still lets them through
//! the host check (any public host): no blocked vector may be accepted.
//! Endpoints are stricter than the image proxy (https and 443 only), so
//! a vector may be refused earlier; the `http://` ones are also tried as
//! `https://`, which must then fail for the vector's own reason (at
//! parsing, or when connecting with the vector's DNS answers).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use nebula_webpush::{
    ClientKeys, Contact, EndpointPolicy, Failure, Limits, Message, Outcome, PushClient,
    PushEndpoint, Urgency, VapidKey,
};
use net_guard::{DnsError, DomainName, LookupFuture, Resolver};
use serde_json::Value;

const VECTORS: &str = include_str!("../../../../security/ssrf-vectors.json");

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

fn answers(vector: &Value) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for kind in ["A", "AAAA"] {
        for ip in vector["dns"][kind].as_array().into_iter().flatten() {
            out.push(ip.as_str().unwrap().parse().unwrap());
        }
    }
    out
}

/// `Err(reason)`: refused at parsing or when connecting.
async fn try_endpoint(input: &str, dns: Vec<IpAddr>) -> Result<(), String> {
    let no_answer = dns.is_empty();
    let policy = EndpointPolicy::any_public_host();
    let endpoint = match PushEndpoint::parse(input, &policy) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return Err(error
                .block_reason()
                .map_or_else(|| error.to_string(), |r| r.as_str().to_owned()));
        }
    };
    let client = PushClient::new(
        Arc::new(VapidKey::generate().unwrap()),
        "mailto:admin@example.com".parse::<Contact>().unwrap(),
        Arc::new(Fixed(dns)),
    )
    .unwrap()
    .with_limits(Limits::new(Duration::from_secs(1), Duration::from_secs(2)));
    let keys = ClientKeys::parse(
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        "BTBZMqHH6r4Tts7J_aSIgg",
    )
    .unwrap();
    let message = Message {
        payload: b"{}",
        ttl: Duration::from_secs(60),
        urgency: Urgency::Normal,
        topic: None,
    };
    match client.send(&endpoint, &keys, message, 0).await.unwrap() {
        Outcome::Failed(Failure::Blocked(reason)) => Err(reason.as_str().to_owned()),
        // No DNS answer: nothing to connect to. It is a temporary failure
        // (retried, the subscription stays), not a policy refusal, but the
        // vector's own reason still holds.
        Outcome::Failed(Failure::Network) if no_answer => Err("dns".to_owned()),
        other => {
            // Allowed by the guard: the connection went (or would go) to
            // a public address; tests never reach one.
            let _ = other;
            Ok(())
        }
    }
}

#[tokio::test]
async fn blocked_vectors_never_become_endpoints() {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    let list = vectors["url"].as_array().unwrap();
    let mut checked = 0;
    for vector in list {
        if vector["expect"] != "block" {
            continue;
        }
        let id = vector["id"].as_str().unwrap();
        let input = vector["input"].as_str().unwrap();
        let reason = vector["reason"].as_str().unwrap();
        let result = try_endpoint(input, answers(vector)).await;
        assert!(result.is_err(), "{id}: accepted");
        if let Some(rest) = input.strip_prefix("http://")
            && matches!(reason, "private-ip" | "dns")
        {
            let https = format!("https://{rest}");
            let result = try_endpoint(&https, answers(vector)).await;
            // An explicit `:80` makes it a port refusal instead.
            let allowed = [reason, "port"];
            assert!(
                result
                    .as_ref()
                    .err()
                    .is_some_and(|got| allowed.contains(&got.as_str())),
                "{id} as https: {result:?}, expected {reason}"
            );
        }
        checked += 1;
    }
    assert!(checked > 20, "only {checked} vectors");
}
