// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! How `JmapBackend::jmap` runs the calls of one request, without a server:
//! `Core/echo` never touches IMAP, so the IMAP server is a closed port.
//!
//! The result references of one request share one budget, `maxSizeRequest`
//!a few `Core/echo` calls that each copy a big
//! earlier answer would otherwise grow a small request into gigabytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a panic is a failed test"
)]

use std::sync::Arc;

use nebula_http::{AccountContext, ApiCall, Backend, jmap_account_id};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{GrantId, NewAccount, Store, UnixTime};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use serde_json::{Map, Value, json};

/// A loopback port nobody listens on.
fn closed_port() -> Port {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    Port::try_from(listener.local_addr().unwrap().port()).unwrap()
}

/// A backend whose IMAP and SMTP servers are closed ports (nothing here
/// connects), with one account in a temporary store.
async fn backend() -> (JmapBackend, AccountContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).await.unwrap();
    let email = format!("alice@{}", nebula_tests::DOMAIN);
    let account = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: &email,
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap();
    let host: net_guard::Host = nebula_tests::HOST.parse().unwrap();
    let imap_port = closed_port();
    let imap = ImapServer::new(host.clone(), imap_port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![imap_port]));
    let resolver: Arc<dyn net_guard::Resolver> = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store.clone(), MailConfig::new(imap), Arc::clone(&resolver)).unwrap();
    let smtp_port = closed_port();
    let smtp = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(SmtpCertificates::InsecureLoopbackOnly);
    let settings = SmtpSettings::new(
        SmtpClient::new(smtp).unwrap(),
        Target::new(host, smtp_port),
        Policy::new(
            PortPolicy::only(vec![smtp_port]),
            NetworkAccess::PrivateAllowed,
        ),
    );
    let backend = JmapBackend::new(store, mail, resolver, JmapConfig::new(settings));
    let context = AccountContext {
        id: account.id,
        jmap_id: jmap_account_id(account.id).unwrap(),
        login: "alice".to_owned(),
        email,
    };
    (backend, context, dir)
}

/// Runs `calls` through `Backend::jmap` (the path of `POST /jmap/api`)
/// with `maxSizeRequest` = `max_size_request` for the method calls.
async fn run(
    backend: &JmapBackend,
    account: &AccountContext,
    max_size_request: usize,
    calls: &Value,
) -> Vec<Value> {
    let body = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": calls,
    });
    // The request itself is small: its own limit is not what is tested.
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 16,
        },
        &[Capability::Core],
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                session_state: State::new("s"),
                limits: MethodLimits {
                    max_objects_in_get: 500,
                    max_objects_in_set: 500,
                    max_size_request,
                },
                languages: nebula_http::AcceptLanguage::default(),
                session: GrantId::from_raw(1).unwrap(),
            },
        )
        .await
        .unwrap();
    let value = serde_json::to_value(response).unwrap();
    value["methodResponses"].as_array().unwrap().clone()
}

/// `Core/echo` whose arguments are six copies of `/s` of the answer to
/// `c0`.
fn six_copies(call_id: &str) -> Value {
    let mut arguments = Map::new();
    for n in 0..6 {
        arguments.insert(
            format!("#r{n}"),
            json!({"resultOf": "c0", "name": "Core/echo", "path": "/s"}),
        );
    }
    json!(["Core/echo", arguments, call_id])
}

#[tokio::test]
async fn result_references_share_one_budget_per_request() {
    let (backend, account, _dir) = backend().await;
    let big = "x".repeat(100_000);
    // `c1` and `c2` copy about 600 KB each: either fits in 1 MB alone,
    // both together don't.
    let calls = json!([
        ["Core/echo", {"s": big}, "c0"],
        six_copies("c1"),
        six_copies("c2"),
        ["Core/echo", {"ok": true}, "c3"],
    ]);

    let limited = run(&backend, &account, 1_000_000, &calls).await;
    assert_eq!(limited.len(), 4);
    assert_eq!(limited[0][0], "Core/echo");
    assert_eq!(limited[1][0], "Core/echo");
    assert_eq!(limited[1][2], "c1");
    for n in 0..6 {
        assert_eq!(
            limited[1][1][format!("r{n}")].as_str().map(str::len),
            Some(100_000),
            "copy {n} of c1"
        );
    }
    // The second ~600 KB passes what is left of the request's budget: a
    // budget per call would have let it through.
    assert_eq!(
        limited[2][0], "error",
        "c2 answered although both copies pass 1 MB together"
    );
    assert_eq!(limited[2][1]["type"], "requestTooLarge");
    assert_eq!(limited[2][2], "c2");
    // A failing call answers only itself (RFC 8620 §3.6.2).
    assert_eq!(limited[3], json!(["Core/echo", {"ok": true}, "c3"]));

    // Control: the same request with room for both copies answers all
    // four, so the error above comes from the budget.
    let roomy = run(&backend, &account, 10_000_000, &calls).await;
    assert_eq!(roomy.len(), 4);
    for (answer, id) in roomy.iter().zip(["c0", "c1", "c2", "c3"]) {
        assert_eq!(answer[0], "Core/echo", "{id}: {}", answer[1]["type"]);
        assert_eq!(answer[2], id);
    }
    assert_eq!(roomy[2][1]["r5"].as_str().map(str::len), Some(100_000));
}
