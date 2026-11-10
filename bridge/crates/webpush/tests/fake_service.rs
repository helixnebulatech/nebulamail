// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The client against a fake push service on loopback: what arrives
//! decrypts with the browser's private key and carries a valid VAPID
//! token; status codes map to outcomes; redirects are not followed; and
//! the SSRF rules hold at connection time (a name resolving to a private
//! address).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests"
)]

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use nebula_webpush::{
    ClientKeys, Contact, EndpointPolicy, Failure, Limits, Message, Outcome, PushClient,
    PushEndpoint, Urgency, VapidKey, ece,
};
use net_guard::{BlockReason, DnsError, DomainName, LookupFuture, Resolver};
use tokio::net::TcpListener;

const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

fn b64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).unwrap()
}

#[derive(Debug, Clone)]
struct Received {
    path: String,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

/// A push service answering `status` (with `extra` headers), after
/// `delay`; it records what it got.
async fn fake_service(
    status: StatusCode,
    extra: &'static [(&'static str, &'static str)],
    delay: Duration,
) -> (SocketAddr, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                    let log = Arc::clone(&log);
                    async move {
                        let path = request.uri().to_string();
                        let headers = request.headers().clone();
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        // The client here only uses 127.0.0.1. A `localhost`
                        // request comes from outside the test (e.g. an IDE
                        // that probes every new listening port): not recorded.
                        let outside = headers
                            .get("host")
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|h| h.split(':').next() == Some("localhost"));
                        if !outside {
                            log.lock().unwrap().push(Received {
                                path,
                                headers,
                                body: body.to_vec(),
                            });
                        }
                        tokio::time::sleep(delay).await;
                        let mut response = Response::builder().status(status);
                        for (name, value) in extra {
                            response = response.header(*name, *value);
                        }
                        Ok::<_, Infallible>(
                            response
                                .body(Full::new(Bytes::from_static(b"https://secret/echo")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (address, seen)
}

/// Answers every name with these addresses.
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

fn client(resolver: impl Resolver + 'static) -> (PushClient, Arc<VapidKey>) {
    let vapid = Arc::new(VapidKey::generate().unwrap());
    let contact: Contact = "mailto:admin@example.com".parse().unwrap();
    let client = PushClient::new(Arc::clone(&vapid), contact, Arc::new(resolver))
        .unwrap()
        .with_limits(Limits::new(
            Duration::from_secs(2),
            Duration::from_millis(1500),
        ));
    (client, vapid)
}

fn keys() -> ClientKeys {
    ClientKeys::parse(UA_PUBLIC, AUTH).unwrap()
}

fn message(payload: &[u8]) -> Message<'_> {
    Message {
        payload,
        ttl: Duration::from_hours(24),
        urgency: Urgency::Normal,
        topic: Some("state"),
    }
}

fn local(address: SocketAddr, path: &str) -> PushEndpoint {
    PushEndpoint::parse(
        &format!("http://{address}{path}"),
        &EndpointPolicy::only(vec![]).with_loopback(),
    )
    .unwrap()
}

#[tokio::test]
async fn delivered_message_decrypts_with_the_browser_key() {
    let (address, seen) = fake_service(StatusCode::CREATED, &[], Duration::ZERO).await;
    let (client, vapid) = client(Fixed(vec![]));
    let payload = br#"{"@type":"StateChange","changed":{"a1":{"EmailDelivery":"7"}}}"#;
    let outcome = client
        .send(
            &local(address, "/push/abc?x=1"),
            &keys(),
            message(payload),
            1_800_000_000,
        )
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Delivered);
    let got = seen.lock().unwrap().pop().unwrap();
    assert_eq!(got.path, "/push/abc?x=1");
    let header = |name: &str| got.headers.get(name).unwrap().to_str().unwrap().to_owned();
    assert_eq!(header("content-encoding"), "aes128gcm");
    // RFC 8620 §7.2: the JSON with its content coding on top.
    assert_eq!(header("content-type"), "application/json");
    assert_eq!(header("ttl"), "86400");
    assert_eq!(header("urgency"), "normal");
    assert_eq!(header("topic"), "state");
    assert_eq!(header("host"), address.to_string());
    // The browser decrypts it.
    let auth: [u8; 16] = b64(AUTH).try_into().unwrap();
    let plain = ece::decrypt(&got.body, &b64(UA_PRIVATE), &auth).unwrap();
    assert_eq!(plain, payload);
    // The VAPID token: for this origin, signed by the bridge's key.
    let authorization = header("authorization");
    let (token, k) = authorization
        .strip_prefix("vapid t=")
        .unwrap()
        .split_once(", k=")
        .unwrap();
    assert_eq!(k, vapid.application_server_key());
    let (input, signature) = token.rsplit_once('.').unwrap();
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED,
        vapid.public_key(),
    )
    .verify(input.as_bytes(), &b64(signature))
    .unwrap();
    let claims: serde_json::Value =
        serde_json::from_slice(&b64(input.split_once('.').unwrap().1)).unwrap();
    assert_eq!(claims["aud"], format!("http://{address}"));
    assert_eq!(claims["sub"], "mailto:admin@example.com");
    assert!(claims["exp"].as_i64().unwrap() > 1_800_000_000);
}

#[tokio::test]
async fn status_codes() {
    for (status, extra, expected) in [
        (StatusCode::GONE, &[][..], Outcome::Gone),
        (StatusCode::NOT_FOUND, &[][..], Outcome::Gone),
        (StatusCode::FORBIDDEN, &[][..], Outcome::Rejected(403)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            &[("retry-after", "30")][..],
            Outcome::Throttled(Some(Duration::from_secs(30))),
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            &[][..],
            Outcome::Failed(Failure::Server(503)),
        ),
    ] {
        let (address, _) = fake_service(status, extra, Duration::ZERO).await;
        let (client, _) = client(Fixed(vec![]));
        let outcome = client
            .send(&local(address, "/p"), &keys(), message(b"{}"), 0)
            .await
            .unwrap();
        assert_eq!(outcome, expected, "{status}");
    }
}

/// A redirect is never followed (here, none at all).
#[tokio::test]
async fn redirects_are_not_followed() {
    let (target, target_seen) = fake_service(StatusCode::CREATED, &[], Duration::ZERO).await;
    let location: &'static str = Box::leak(format!("http://{target}/stolen").into_boxed_str());
    let extra: &'static [(&'static str, &'static str)] =
        Box::leak(vec![("location", location)].into_boxed_slice());
    let (address, _) = fake_service(StatusCode::TEMPORARY_REDIRECT, extra, Duration::ZERO).await;
    let (client, _) = client(Fixed(vec![]));
    let outcome = client
        .send(&local(address, "/p"), &keys(), message(b"{}"), 0)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Rejected(307));
    assert!(target_seen.lock().unwrap().is_empty());
}

/// A slow service is cut off at the total time limit.
#[tokio::test]
async fn slow_service_times_out() {
    let (address, _) = fake_service(StatusCode::CREATED, &[], Duration::from_secs(5)).await;
    let (client, _) = client(Fixed(vec![]));
    let started = std::time::Instant::now();
    let outcome = client
        .send(&local(address, "/p"), &keys(), message(b"{}"), 0)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Failed(Failure::Network));
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// A push host that resolves to a private address (DNS rebinding, a
/// hostile record) is refused when connecting, even if any public host
/// is allowed; so is one with a public and a private address.
#[tokio::test]
async fn private_addresses_behind_a_name_are_refused() {
    let (address, seen) = fake_service(StatusCode::CREATED, &[], Duration::ZERO).await;
    let endpoint = PushEndpoint::parse(
        "https://push.example.org/p",
        &EndpointPolicy::any_public_host(),
    )
    .unwrap();
    for answer in [
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        vec!["169.254.169.254".parse().unwrap()],
        vec![
            "93.184.215.14".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
        ],
    ] {
        let (client, _) = client(Fixed(answer));
        let outcome = client
            .send(&endpoint, &keys(), message(b"{}"), 0)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::Failed(Failure::Blocked(BlockReason::PrivateIp))
        );
    }
    assert!(seen.lock().unwrap().is_empty());
    let _ = address;
}

/// Answers every name with nothing (no records, not an error).
struct Empty;

impl Resolver for Empty {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[tokio::test]
async fn dns_trouble_is_temporary() {
    let endpoint = PushEndpoint::parse(
        "https://push.example.org/p",
        &EndpointPolicy::any_public_host(),
    )
    .unwrap();
    // A failing resolver: try again later, keep the subscription.
    let (failing, _) = client(Fixed(vec![]));
    assert_eq!(
        failing
            .send(&endpoint, &keys(), message(b"{}"), 0)
            .await
            .unwrap(),
        Outcome::Failed(Failure::Network)
    );
    // No address at all: the same.
    let (empty, _) = client(Empty);
    assert_eq!(
        empty
            .send(&endpoint, &keys(), message(b"{}"), 0)
            .await
            .unwrap(),
        Outcome::Failed(Failure::Network)
    );
}

#[tokio::test]
async fn a_name_that_is_never_public_stays_blocked() {
    // Policy, not DNS trouble: permanent, whatever the resolver says.
    let endpoint = PushEndpoint::parse(
        "https://push.internal/p",
        &EndpointPolicy::any_public_host(),
    )
    .unwrap();
    let (client, _) = client(Fixed(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 215, 14))]));
    assert_eq!(
        client
            .send(&endpoint, &keys(), message(b"{}"), 0)
            .await
            .unwrap(),
        Outcome::Failed(Failure::Blocked(BlockReason::Dns))
    );
}
