// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::time::Duration;

use axum::body::Body;
use futures::StreamExt as _;
use http::{Method, Request, StatusCode, header};
use nebula_jmap_proto::State as JmapState;
use nebula_jmap_proto::error::OVER_QUOTA_URI;
use nebula_jmap_proto::push::{CalendarAlert, StateChange};
use nebula_jmap_proto::types::DataType;
use serde_json::json;
use tower::ServiceExt as _;

use crate::backend::{PushEvent, StartReason};
use crate::jmap::event_id::Seen;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_object() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header(header::CACHE_CONTROL), Some("no-store"));
    let session = reply.json();
    assert_eq!(session["username"], "alice");
    assert_eq!(session["apiUrl"], format!("{ISSUER}/jmap/api"));
    assert_eq!(
        session["downloadUrl"],
        format!("{ISSUER}/jmap/download/{{accountId}}/{{blobId}}/{{name}}?accept={{type}}")
    );
    assert_eq!(
        session["uploadUrl"],
        format!("{ISSUER}/jmap/upload/{{accountId}}")
    );
    assert_eq!(
        session["eventSourceUrl"],
        format!(
            "{ISSUER}/jmap/eventsource/?types={{types}}&closeafter={{closeafter}}&ping={{ping}}"
        )
    );
    let account_id = session["primaryAccounts"]["urn:ietf:params:jmap:mail"]
        .as_str()
        .unwrap();
    assert_eq!(account_id, "a1");
    assert_eq!(session["accounts"]["a1"]["name"], "alice@example.test");
    assert_eq!(
        session["capabilities"]["urn:ietf:params:jmap:core"]["maxSizeRequest"],
        10_000_000
    );
    assert!(session["capabilities"]["urn:ietf:params:jmap:submission"].is_object());
    // The bridge says it is the bridge.
    assert_eq!(
        session["capabilities"]["https://helixnebula.tech/jmap/bridge"],
        json!({})
    );
    assert_eq!(session["state"].as_str().unwrap().len(), 16);
    // Stable while nothing changes.
    let again = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(again.json()["state"], session["state"]);

    // A well-formed token that isn't valid: `invalid_token` (RFC 6750
    // §3.1).
    for token in ["not-a-token", &"A".repeat(43), "abc=="] {
        let reply = h
            .with_bearer("GET", "/.well-known/jmap", token, Body::empty())
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{token}");
        assert_eq!(
            reply.header(header::WWW_AUTHENTICATE),
            Some("Bearer realm=\"jmap\", error=\"invalid_token\""),
        );
    }
}

/// RFC 6750 §3.1: no error code without Bearer authentication; a
/// malformed one is `invalid_request` with 400.
#[tokio::test(flavor = "multi_thread")]
async fn bearer_challenges() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let challenge = |reply: &Reply| reply.header(header::WWW_AUTHENTICATE).map(str::to_owned);

    let none = h.get("/.well-known/jmap").await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&none).as_deref(), Some("Bearer realm=\"jmap\""));
    assert_eq!(none.json()["status"], 401);

    let send = |values: &[&str]| {
        let mut request = Request::get("/.well-known/jmap");
        for value in values {
            request = request.header(header::AUTHORIZATION, *value);
        }
        h.send(request.body(Body::empty()).unwrap())
    };
    let basic = send(&["Basic YWxpY2U6c2VjcmV0"]).await;
    assert_eq!(basic.status, StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&basic).as_deref(), Some("Bearer realm=\"jmap\""));

    let malformed = Some("Bearer realm=\"jmap\", error=\"invalid_request\"");
    let bearer = format!("Bearer {access}");
    for values in [
        &["Bearer"][..],
        &["Bearer "],
        &["Bearer a b"],
        &["Bearer a=b"],
        &["Bearer ="],
        &["Bearer t\u{f1}"],
        &[bearer.as_str(), bearer.as_str()],
    ] {
        let reply = send(values).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{values:?}");
        assert_eq!(challenge(&reply).as_deref(), malformed, "{values:?}");
    }

    // The scheme is case-insensitive and may have more spaces.
    let lower = send(&[&format!("bearer   {access}")]).await;
    assert_eq!(lower.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn access_tokens_expire() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    h.advance(901);
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn api_requests() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let request = json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [["Core/echo", {"hello": true}, "c1"]]
    });
    let reply = h
        .with_bearer(
            "POST",
            "/jmap/api",
            &access,
            Body::from(request.to_string()),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let response = reply.json();
    assert_eq!(
        response["methodResponses"][0],
        json!(["Core/echo", {"hello": true}, "c1"])
    );
    let session = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(response["sessionState"], session.json()["state"]);

    let unknown = json!({"using": ["urn:ietf:params:jmap:calendars"], "methodCalls": []});
    let reply = h
        .with_bearer(
            "POST",
            "/jmap/api",
            &access,
            Body::from(unknown.to_string()),
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply.header(header::CONTENT_TYPE),
        Some("application/problem+json")
    );
    assert_eq!(
        reply.json()["type"],
        "urn:ietf:params:jmap:error:unknownCapability"
    );

    let reply = h
        .with_bearer("POST", "/jmap/api", &access, Body::from("{"))
        .await;
    assert_eq!(reply.json()["type"], "urn:ietf:params:jmap:error:notJSON");

    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(10_000_001));
    let reply = h
        .with_bearer("POST", "/jmap/api", &access, Body::from(big))
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json()["limit"], "maxSizeRequest");
    assert_eq!(h.fake.seen.lock().unwrap().api_calls, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_and_download() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let reply = h
        .send(
            Request::post("/jmap/upload/a1")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .header(header::CONTENT_TYPE, "Image/PNG")
                .body(Body::from("png bytes"))
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    assert_eq!(
        reply.json(),
        json!({"accountId": "a1", "blobId": "b1", "type": "image/png", "size": 9})
    );
    let other = h
        .with_bearer("POST", "/jmap/upload/a2", &access, Body::from("x"))
        .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    let big = h
        .with_bearer(
            "POST",
            "/jmap/upload/a1",
            &access,
            Body::from(vec![0u8; 50_000_001]),
        )
        .await;
    assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(big.json()["limit"], "maxSizeUpload");
    // Without a Content-Length the limit holds while the body streams.
    let chunks = futures::stream::iter(
        (0..60).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0u8; 1_000_000]))),
    );
    let streamed = h
        .with_bearer(
            "POST",
            "/jmap/upload/a1",
            &access,
            Body::from_stream(chunks),
        )
        .await;
    assert_eq!(streamed.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(streamed.json()["limit"], "maxSizeUpload");
    // The account's upload storage is full: 413 with `overQuota`.
    let full = h
        .send(
            Request::post("/jmap/upload/a1")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .header(header::CONTENT_TYPE, "application/x-full")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    assert_eq!(full.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(full.json()["type"], OVER_QUOTA_URI);

    // A streamed blob arrives whole, with its length.
    let reply = h
        .with_bearer("GET", "/jmap/download/a1/b2/x.txt", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body.as_ref(), b"abcdefghi");
    assert_eq!(reply.header(header::CONTENT_LENGTH), Some("9"));

    let reply = h
        .with_bearer(
            "GET",
            "/jmap/download/a1/b1/..%2Finvoice%E2%80%AEfdp.exe?accept=text%2Fplain",
            &access,
            Body::empty(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body.as_ref(), b"<script>alert(1)</script>");
    assert_eq!(reply.header(header::CONTENT_TYPE), Some("text/plain"));
    assert_eq!(
        reply.header(header::CONTENT_DISPOSITION),
        Some("attachment; filename=\"invoicefdp.exe\"; filename*=UTF-8''invoicefdp.exe")
    );
    assert_eq!(
        reply.header(header::CONTENT_SECURITY_POLICY),
        Some("default-src 'none'; sandbox")
    );
    assert_eq!(
        reply.header(header::X_CONTENT_TYPE_OPTIONS),
        Some("nosniff")
    );
    // Without `accept`, the stored type.
    let reply = h
        .with_bearer(
            "GET",
            "/jmap/download/a1/b1/page.html",
            &access,
            Body::empty(),
        )
        .await;
    assert_eq!(reply.header(header::CONTENT_TYPE), Some("text/html"));

    for uri in [
        "/jmap/download/a2/b1/x",
        "/jmap/download/a1/b9/x",
        "/jmap/download/a1/b%20c/x",
    ] {
        let reply = h.with_bearer("GET", uri, &access, Body::empty()).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_download_says_when_to_retry() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let reply = h
        .send(
            Request::get("/jmap/download/a1/busy/x")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .header(header::ORIGIN, APP)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.header(header::RETRY_AFTER), Some("5"));
    // The web app is on another origin: the browser only shows it the
    // header if CORS exposes it.
    let exposed = reply
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .unwrap_or_default();
    assert!(exposed.contains("retry-after"), "{exposed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_hold_a_slot_until_sent() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let get = || {
        h.router.clone().oneshot(
            Request::get("/jmap/download/a1/b1/x")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    // Four answers whose bodies nobody reads yet.
    let mut open = Vec::new();
    for _ in 0..4 {
        let response = get().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let fifth = get().await.unwrap();
    assert_eq!(fifth.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = axum::body::to_bytes(fifth.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["limit"], "maxConcurrentRequests");
    // One is read to the end: another fits.
    let done = open.pop().unwrap();
    let body = axum::body::to_bytes(done.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"<script>alert(1)</script>");
    assert_eq!(get().await.unwrap().status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn event_source_streams_state_changes() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let response = h
        .router
        .clone()
        .oneshot(
            Request::get("/jmap/eventsource/?types=Email,Mailbox&closeafter=state&ping=0")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let mut body = response.into_body().into_data_stream();

    let change = |account: &str, data_type: DataType, state: &str| {
        PushEvent::State(StateChange {
            changed: [(
                account.parse().unwrap(),
                [(data_type, JmapState::new(state))].into(),
            )]
            .into(),
        })
    };
    // Wait until the stream subscribed.
    while h.fake.events.receiver_count() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.fake
        .events
        .send(change("a2", DataType::Email, "other"))
        .unwrap();
    h.fake
        .events
        .send(change("a1", DataType::Identity, "skip"))
        .unwrap();
    h.fake
        .events
        .send(change("a1", DataType::Email, "s7"))
        .unwrap();
    let mut text = String::new();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
    {
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    // closeafter=state: one event, then the stream ended. Its id has
    // every state seen (RFC 8620 §7.3).
    let id = state_id(&[("a1", DataType::Email, "s7")]);
    assert_eq!(
        text,
        format!(
            "event: state\nid: {id}\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"a1\":{{\"Email\":\"s7\"}}}}}}\n\n"
        )
    );
}

/// The id of a connection of a1 that has seen `states`.
fn state_id(states: &[(&str, DataType, &str)]) -> String {
    Seen::new(state_change(states)).id(&"a1".parse().unwrap())
}

fn state_change(states: &[(&str, DataType, &str)]) -> StateChange {
    let mut change = StateChange::default();
    for (account, data_type, state) in states {
        change
            .changed
            .entry(account.parse().unwrap())
            .or_default()
            .insert(data_type.clone(), JmapState::new(*state));
    }
    change
}

/// Opens an event source of alice with `Last-Event-ID: last` (if any) and
/// reads it until it ends (`closeafter=state`) or `wait` passes.
async fn read_events(
    h: &Harness,
    access: &str,
    query: &str,
    last: Option<&str>,
    wait: Duration,
) -> String {
    let mut request = Request::get(format!("/jmap/eventsource/?{query}"))
        .header(header::AUTHORIZATION, format!("Bearer {access}"));
    if let Some(last) = last {
        request = request.header("last-event-id", last);
    }
    let response = h
        .router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    while let Ok(Some(chunk)) = tokio::time::timeout(wait, body.next()).await {
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    text
}

/// RFC 8620 §7.3: a reconnection with `Last-Event-ID` gets at once the
/// states that changed since; a new connection (no header) nothing; an
/// id that isn't ours every state.
#[tokio::test(flavor = "multi_thread")]
async fn event_source_resumes_from_last_event_id() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let before = [
        ("a1", DataType::Email, "e1"),
        ("a1", DataType::Mailbox, "m1"),
        ("a1", DataType::Identity, "i1"),
        ("a2", DataType::Email, "x"),
    ];
    let last = state_id(&before[..3]);
    let now = [
        ("a1", DataType::Email, "e2"),
        ("a1", DataType::Mailbox, "m1"),
        ("a1", DataType::Identity, "i2"),
        ("a2", DataType::Email, "y"),
    ];
    *h.fake.states.lock().unwrap() = state_change(&now);
    let query = "types=Email,Mailbox&closeafter=state&ping=0";
    let wait = Duration::from_secs(5);

    // Only Email changed of the types asked for; the id covers them.
    let text = read_events(&h, &access, query, Some(&last), wait).await;
    let id = state_id(&now[..2]);
    assert_eq!(
        text,
        format!(
            "event: state\nid: {id}\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"a1\":{{\"Email\":\"e2\"}}}}}}\n\n"
        )
    );
    // With that id, nothing is missed; nor without an id.
    let quiet = Duration::from_millis(300);
    let no_query = "types=Email,Mailbox&closeafter=no&ping=0";
    assert_eq!(
        read_events(&h, &access, no_query, Some(&id), quiet).await,
        ""
    );
    assert_eq!(read_events(&h, &access, no_query, None, quiet).await, "");
    // Not one of ours: every state of the types asked for.
    let text = read_events(&h, &access, query, Some("someone else's"), wait).await;
    assert_eq!(
        text,
        format!(
            "event: state\nid: {id}\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"a1\":{{\"Mailbox\":\"m1\",\"Email\":\"e2\"}}}}}}\n\n"
        )
    );

    // Later events carry ids of everything seen, not just their change.
    let response = h
        .router
        .clone()
        .oneshot(
            Request::get(format!("/jmap/eventsource/?{no_query}"))
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    while h.fake.events.receiver_count() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.fake
        .events
        .send(PushEvent::State(state_change(&[(
            "a1",
            DataType::Mailbox,
            "m2",
        )])))
        .unwrap();
    let chunk = tokio::time::timeout(wait, body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let id = state_id(&[
        ("a1", DataType::Email, "e2"),
        ("a1", DataType::Mailbox, "m2"),
    ]);
    assert_eq!(
        std::str::from_utf8(&chunk).unwrap(),
        format!(
            "event: state\nid: {id}\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"a1\":{{\"Mailbox\":\"m2\"}}}}}}\n\n"
        )
    );
}

/// `calendarAlert` events: only the account's own, only for connections
/// that asked for `CalendarAlert` or `*` (as Stalwart), and they don't
/// count for `closeafter=state`.
#[tokio::test(flavor = "multi_thread")]
async fn event_source_streams_calendar_alerts() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let alert = |account: &str, alert: &str| {
        PushEvent::CalendarAlert(
            CalendarAlert::new(
                account.parse().unwrap(),
                "v7".parse().unwrap(),
                "u1@example.com",
                Some("2026-10-27T17:11:44".parse().unwrap()),
                alert,
            )
            .unwrap(),
        )
    };
    let state = PushEvent::State(StateChange {
        changed: [(
            "a1".parse().unwrap(),
            [(DataType::Email, JmapState::new("s7"))].into(),
        )]
        .into(),
    });
    let alert_text = "event: calendarAlert\ndata: {\"@type\":\"CalendarAlert\",\"accountId\":\"a1\",\
        \"calendarEventId\":\"v7\",\"uid\":\"u1@example.com\",\"recurrenceId\":\"2026-10-27T17:11:44\",\
        \"alertId\":\"k1\"}\n\n";
    let state_text = format!(
        "event: state\nid: {}\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"a1\":{{\"Email\":\"s7\"}}}}}}\n\n",
        state_id(&[("a1", DataType::Email, "s7")])
    );
    for (types, expected, alerts) in [
        (
            "CalendarAlert,Email",
            format!("{alert_text}{state_text}"),
            true,
        ),
        ("*", format!("{alert_text}{state_text}"), true),
        ("CalendarEvent,Email", state_text.clone(), false),
    ] {
        let response = h
            .router
            .clone()
            .oneshot(
                Request::get(format!(
                    "/jmap/eventsource/?types={types}&closeafter=state&ping=0"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        while h.fake.events.receiver_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        h.fake.events.send(alert("a2", "other")).unwrap();
        h.fake.events.send(alert("a1", "k1")).unwrap();
        h.fake.events.send(state.clone()).unwrap();
        h.fake.events.send(alert("a1", "after")).unwrap();
        let mut text = String::new();
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .unwrap()
        {
            text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
        }
        assert_eq!(text, expected, "types={types}");
        // Only a connection that asked for alerts counts as hearing them.
        assert_eq!(
            h.fake.seen.lock().unwrap().event_alerts.last(),
            Some(&alerts),
            "types={types}"
        );
        drop(body);
        while h.fake.events.receiver_count() > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn event_source_pings_and_limits() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let open = |uri: &'static str| {
        h.router.clone().oneshot(
            Request::get(uri)
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let bad = open("/jmap/eventsource/?types=Email&closeafter=maybe&ping=0")
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let missing = open("/jmap/eventsource/").await.unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    let response = open("/jmap/eventsource/?types=*&closeafter=no&ping=1")
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&chunk).unwrap(),
        "event: ping\ndata: {\"interval\":1}\n\n"
    );

    // 8 streams per account; this one is the second.
    let mut streams = Vec::new();
    for _ in 0..7 {
        let response = open("/jmap/eventsource/?types=*&closeafter=no&ping=0")
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response);
    }
    let refused = open("/jmap/eventsource/?types=*&closeafter=no&ping=0")
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(streams);

    // Shutdown ends open streams.
    h.shutdown.cancel();
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = body.next().await {
            chunk.unwrap();
        }
    })
    .await;
    assert!(end.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_starts_accounts_from_the_store() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let Harness { store, dir, .. } = h;
    let restarted = Harness::on_store(config(), store, dir, Fake::default());
    let reply = restarted
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        restarted.fake.seen.lock().unwrap().started,
        [("alice".to_owned(), "right password".to_owned())]
    );
    // Only once.
    restarted
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(restarted.fake.seen.lock().unwrap().started.len(), 1);
    assert_eq!(
        restarted.fake.seen.lock().unwrap().reasons,
        [StartReason::Restore]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_stored_password_is_401() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    assert_eq!(h.fake.seen.lock().unwrap().reasons, [StartReason::Login]);
    let Harness { store, dir, .. } = h;
    let fake = Fake::default();
    fake.seen.lock().unwrap().refuse_password = true;
    let restarted = Harness::on_store(config(), store, dir, fake);
    let reply = restarted
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn cors_only_for_app_origins() {
    let h = Harness::new().await;
    let preflight = |origin: &str| {
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/jmap/api")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap()
    };
    let reply = h.send(preflight(APP)).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header(header::ACCESS_CONTROL_ALLOW_ORIGIN), Some(APP));
    let headers = reply.header(header::ACCESS_CONTROL_ALLOW_HEADERS).unwrap();
    assert!(headers.contains("authorization"), "{headers}");
    assert!(
        reply
            .header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none()
    );

    let evil = h.send(preflight("https://evil.example")).await;
    assert!(evil.header(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());

    // Errors carry CORS headers too, so the app can read a 401.
    let reply = h
        .send(
            Request::get("/.well-known/jmap")
                .header(header::ORIGIN, APP)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.header(header::ACCESS_CONTROL_ALLOW_ORIGIN), Some(APP));

    // OAuth endpoints are not for browsers' fetch.
    let reply = h
        .send(
            Request::get("/.well-known/oauth-authorization-server")
                .header(header::ORIGIN, APP)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(reply.header(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn security_headers_everywhere() {
    let h = Harness::new().await;
    for uri in [
        "/healthz",
        "/nothing-here",
        "/.well-known/jmap",
        "/.well-known/oauth-authorization-server",
    ] {
        let reply = h.get(uri).await;
        assert_eq!(
            reply.header(header::X_CONTENT_TYPE_OPTIONS),
            Some("nosniff"),
            "{uri}"
        );
        assert_eq!(
            reply.header(header::REFERRER_POLICY),
            Some("no-referrer"),
            "{uri}"
        );
        assert_eq!(reply.header(header::X_FRAME_OPTIONS), Some("DENY"), "{uri}");
        assert!(
            reply.header(header::CONTENT_SECURITY_POLICY).is_some(),
            "{uri}"
        );
        assert_eq!(
            reply.header(header::STRICT_TRANSPORT_SECURITY),
            Some("max-age=63072000; includeSubDomains"),
            "{uri}"
        );
    }
    assert_eq!(h.get("/nothing-here").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.get("/oauth/token").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_hsts_over_plain_http() {
    let client = OAuthClient::new(
        CLIENT_ID.parse().unwrap(),
        ClientSecretHash::hash(&SecretString::from(CLIENT_SECRET)).unwrap(),
        vec![RedirectUri::parse("http://localhost:3001/auth/callback").unwrap()],
    )
    .unwrap();
    let config = HttpConfig::new(
        PublicUrl::parse("http://127.0.0.1:8080").unwrap(),
        vec![AppOrigin::parse("http://localhost:3001").unwrap()],
        vec![client],
    )
    .unwrap();
    let h = Harness::with_config(config).await;
    let reply = h.get("/healthz").await;
    assert!(reply.header(header::STRICT_TRANSPORT_SECURITY).is_none());
    let mut params = authorize_query();
    for (name, value) in &mut params {
        if *name == "redirect_uri" {
            *value = "http://localhost:3001/auth/callback";
        }
    }
    let reply = h
        .get(&format!("/oauth/authorize?{}", encode(&params)))
        .await;
    let cookie = reply.header(header::SET_COOKIE).unwrap();
    assert!(cookie.starts_with("nebula-authz="), "{cookie}");
    assert!(!cookie.contains("Secure"));
}

/// Logs bob in; his access token.
async fn bob(h: &Harness) -> String {
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "bob"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    let code = query(&reply.location())["code"].clone();
    let reply = h.exchange(&code).await;
    reply.json()["access_token"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_are_limited_per_account() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let start = |account: &str, token: &str| {
        let request = Request::get(format!("/jmap/download/{account}/b3/x.txt"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        h.router.clone().oneshot(request)
    };
    // Four downloads that are never read: their headers are out, their
    // bodies hold the slots.
    let mut open = Vec::new();
    for _ in 0..4 {
        let response = start("a1", &access).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let fifth = start("a1", &access).await.unwrap();
    assert_eq!(fifth.status(), StatusCode::TOO_MANY_REQUESTS);
    // Another account isn't affected.
    let bob = bob(&h).await;
    let response = start("a2", &bob).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // A finished (or dropped) body frees its slot.
    drop(open.pop());
    let next = start("a1", &access).await.unwrap();
    assert_eq!(next.status(), StatusCode::OK);
}
