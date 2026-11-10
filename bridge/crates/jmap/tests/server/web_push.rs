// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Web Push through the JMAP backend, with the
//! IMAP server of `NEBULA_TEST_IMAP=1` and a fake push service on
//! loopback that decrypts each message with the browser's private key
//! (RFC 8291 §5 test keys), as the browser would:
//!
//! - the `PushSubscription/set` that `lib/web-push.ts` sends creates it,
//!   and a `PushVerification` arrives; nothing else does until the code
//!   comes back (a wrong code is refused);
//! - endpoints of private addresses, plain http, other hosts and bad keys
//!   are refused;
//! - a new email arrives as a `StateChange` with only `EmailDelivery` (the
//!   type the web asked for); a flag change sends nothing;
//! - a 410 from the push service deletes the subscription;
//! - the same browser subscribing from a new login session replaces the
//!   subscription of the old one (RFC 8620 §7.2);
//! - with `emailPush` (Stalwart 1.0, what `mail/email-push.ts` asks for),
//!   mail delivered by someone else arrives inside the push as an
//!   `EmailPush` with sender and subject, only the emails the filter
//!   keeps, with the `Urgency` asked for; a flag change sends nothing;
//!   unsupported requests are refused.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::service::service_fn;
use hyper::{Request as HttpRequest, Response, StatusCode};
use hyper_util::rt::TokioIo;
use nebula_http::{Backend, UploadBody};
use nebula_jmap::WebPushSettings;
use nebula_store::{AccountId, GrantId, NewTokens, Store, TokenHash, UnixTime};
use nebula_tests::{TestServer, User};
use nebula_webpush::{Contact, EndpointPolicy, PushClient, VapidKey, ece};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::{call, call_in, find, mailbox, new_backend_full, start};

const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

fn b64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).unwrap()
}

/// What the push service got: the decrypted object and its `Urgency`.
type Pushed = (Value, String);

/// A push service that decrypts what it gets and answers `status`.
async fn push_service(
    status: Arc<Mutex<StatusCode>>,
) -> (SocketAddr, mpsc::UnboundedReceiver<Pushed>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let sender = sender.clone();
            let status = Arc::clone(&status);
            tokio::spawn(async move {
                let service = service_fn(move |request: HttpRequest<hyper::body::Incoming>| {
                    let sender = sender.clone();
                    let status = Arc::clone(&status);
                    async move {
                        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
                        assert!(
                            request.headers()["authorization"]
                                .to_str()
                                .unwrap()
                                .starts_with("vapid t=")
                        );
                        let urgency = request.headers()["urgency"].to_str().unwrap().to_owned();
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        assert!(body.len() <= ece::MAX_BODY);
                        let auth: [u8; 16] = b64(AUTH).try_into().unwrap();
                        let plain = ece::decrypt(&body, &b64(UA_PRIVATE), &auth).unwrap();
                        let object = serde_json::from_slice::<Value>(&plain).unwrap();
                        let _ = sender.send((object, urgency));
                        let status = *status.lock().unwrap();
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(status)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (address, receiver)
}

async fn next(receiver: &mut mpsc::UnboundedReceiver<Pushed>, wait: Duration) -> Option<Value> {
    next_pushed(receiver, wait).await.map(|(object, _)| object)
}

async fn next_pushed(
    receiver: &mut mpsc::UnboundedReceiver<Pushed>,
    wait: Duration,
) -> Option<Pushed> {
    tokio::time::timeout(wait, receiver.recv())
        .await
        .ok()
        .flatten()
}

fn settings() -> WebPushSettings {
    let resolver: Arc<dyn net_guard::Resolver> =
        Arc::new(net_guard::SystemResolver::new().unwrap());
    let client = PushClient::new(
        Arc::new(VapidKey::generate().unwrap()),
        "mailto:admin@nebulamail.test".parse::<Contact>().unwrap(),
        resolver,
    )
    .unwrap();
    let mut settings =
        WebPushSettings::new(client, EndpointPolicy::known_services().with_loopback());
    settings.min_interval = Duration::from_millis(100);
    settings
}

/// A login session of `account` (push subscriptions belong to one).
async fn session(store: &Store, account: AccountId, n: u8) -> GrantId {
    let now = UnixTime::now().unwrap();
    let ends = now.saturating_add(Duration::from_hours(24 * 30));
    store
        .create_grant(
            account,
            "web",
            "mail",
            now,
            ends,
            NewTokens {
                access: TokenHash([n; 32]),
                access_expires_at: ends,
                refresh: TokenHash([n.wrapping_add(128); 32]),
                refresh_expires_at: ends,
            },
        )
        .await
        .unwrap()
}

fn create(url: &str, p256dh: &str) -> Value {
    json!({
        "deviceClientId": "nebula-test-device",
        "expires": "2099-01-01T00:00:00Z",
        "keys": {"auth": AUTH, "p256dh": p256dh},
        "types": ["EmailDelivery", "CalendarAlert"],
        "url": url,
    })
}

async fn import(
    backend: &nebula_jmap::JmapBackend,
    account: &nebula_http::AccountContext,
    inbox: &str,
    subject: &str,
) -> String {
    import_with(backend, account, inbox, subject, &json!({})).await
}

async fn import_with(
    backend: &nebula_jmap::JmapBackend,
    account: &nebula_http::AccountContext,
    inbox: &str,
    subject: &str,
    keywords: &Value,
) -> String {
    let raw = format!(
        "From: Ana <ana@example.com>\r\nTo: alice@nebulamail.test\r\nSubject: {subject}\r\n\
         Message-ID: <{subject}@example.com>\r\n\r\nHola.\r\n"
    );
    let uploaded = backend
        .upload(
            account,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(vec![Bytes::from(raw.into_bytes())], 1_000_000),
        )
        .await
        .unwrap();
    let a = account.jmap_id.as_str();
    let imported = call(
        backend,
        account,
        json!([["Email/import", {"accountId": a, "emails": {"m": {
            "blobId": uploaded.blob_id.as_str(), "mailboxIds": {inbox: true},
            "keywords": keywords
        }}}, "0"]]),
    )
    .await;
    imported[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not imported: {imported:?}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one story, step by step")]
async fn subscriptions_verification_and_new_mail() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend_full(
        &store,
        server,
        |_| {},
        |config| {
            config.web_push = Some(settings());
        },
    );
    assert_eq!(backend.web_push_key().map(|k| k.len()), Some(87));
    let alice = start(&backend, &store, server, User::Alice).await;
    let one = session(&store, alice.id, 1).await;
    let other = session(&store, alice.id, 2).await;
    let a = alice.jmap_id.as_str();
    let inbox = mailbox(&backend, &alice, "inbox").await;
    let status = Arc::new(Mutex::new(StatusCode::CREATED));
    let (address, mut pushes) = push_service(Arc::clone(&status)).await;

    // Refused endpoints and keys, and no verification sent.
    let mut bad = serde_json::Map::new();
    for (name, url, key) in [
        ("private", "https://10.0.0.1/push", UA_PUBLIC),
        ("metadata", "https://169.254.169.254/latest", UA_PUBLIC),
        ("plain", "http://fcm.googleapis.com/fcm/send/x", UA_PUBLIC),
        ("other", "https://push.evil.example/x", UA_PUBLIC),
        ("port", "https://fcm.googleapis.com:8443/x", UA_PUBLIC),
        ("keys", &format!("http://{address}/push/a"), "BCVx"),
    ] {
        bad.insert(name.to_owned(), create(url, key));
    }
    let refused = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/set", {"create": bad}, "0"]]),
    )
    .await;
    for name in ["private", "metadata", "plain", "other", "port", "keys"] {
        assert_eq!(
            refused[0][1]["notCreated"][name]["type"], "invalidProperties",
            "{name}: {refused:?}"
        );
        assert!(
            !refused[0][1]["notCreated"][name]
                .to_string()
                .contains("evil")
        );
    }

    // What lib/web-push.ts sends.
    let url = format!("http://{address}/push/alice");
    let created = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/set", {"create": {"push": create(&url, UA_PUBLIC)}}, "0"]]),
    )
    .await;
    let id = created[0][1]["created"]["push"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    // `expires` is shortened to 7 days.
    let expires = created[0][1]["created"]["push"]["expires"]
        .as_str()
        .unwrap();
    assert!(expires < "2099", "{expires}");
    let verification = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(verification["@type"], "PushVerification");
    assert_eq!(verification["pushSubscriptionId"], id);
    let code = verification["verificationCode"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(code.len() >= 32);

    // Unverified: new mail is not pushed.
    let first = import(&backend, &alice, &inbox, "push-before-verify").await;
    assert!(next(&mut pushes, Duration::from_secs(3)).await.is_none());

    let wrong = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/set", {"update": {&id: {"verificationCode": "nope"}}}, "0"]]),
    )
    .await;
    assert_eq!(wrong[0][1]["notUpdated"][&id]["type"], "invalidProperties");
    let verified = call_in(
        &backend,
        &alice,
        one,
        json!([
            ["PushSubscription/set", {"update": {&id: {"verificationCode": code}}}, "0"],
            ["PushSubscription/get", {}, "1"]
        ]),
    )
    .await;
    assert!(verified[0][1]["updated"].get(&id).is_some(), "{verified:?}");
    let listed = &verified[1][1]["list"][0];
    assert_eq!(listed["id"], id);
    assert_eq!(listed["deviceClientId"], "nebula-test-device");
    assert_eq!(listed["verificationCode"], code);
    assert_eq!(listed["types"], json!(["CalendarAlert", "EmailDelivery"]));
    assert!(listed.get("url").is_none() && listed.get("keys").is_none());
    // Another session of the account neither sees nor changes it.
    let elsewhere = call_in(
        &backend,
        &alice,
        other,
        json!([
            ["PushSubscription/get", {}, "0"],
            ["PushSubscription/set", {"destroy": [&id]}, "1"]
        ]),
    )
    .await;
    assert_eq!(elsewhere[0][1]["list"], json!([]), "{elsewhere:?}");
    assert_eq!(elsewhere[1][1]["notDestroyed"][&id]["type"], "notFound");

    // New mail: a StateChange with EmailDelivery only.
    let second = import(&backend, &alice, &inbox, "push-after-verify").await;
    let change = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(change["@type"], "StateChange", "{change}");
    let changed = change["changed"][a].as_object().unwrap();
    assert_eq!(
        changed.keys().collect::<Vec<_>>(),
        ["EmailDelivery"],
        "{change}"
    );
    // A flag change is not a delivery.
    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "update": {&second: {"keywords/$seen": true}}}, "0"]]),
    )
    .await;
    let extra = next(&mut pushes, Duration::from_secs(3)).await;
    assert!(extra.is_none(), "{extra:?}");

    // The browser unsubscribed: 410 deletes the subscription.
    *status.lock().unwrap() = StatusCode::GONE;
    let third = import(&backend, &alice, &inbox, "push-gone").await;
    assert!(next(&mut pushes, Duration::from_secs(10)).await.is_some());
    let mut gone = false;
    for _ in 0..20 {
        let list = call_in(
            &backend,
            &alice,
            one,
            json!([["PushSubscription/get", {}, "0"]]),
        )
        .await;
        if list[0][1]["list"].as_array().unwrap().is_empty() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(gone, "the subscription should be deleted after a 410");

    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [first, second, third]}, "0"]]),
    )
    .await;
}

/// The same browser (same `deviceClientId`) subscribing again from a new
/// login session replaces what it made in the old one (RFC 8620 §7.2): the
/// old session no longer has it, and the delivery task drops it too.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one story, step by step")]
async fn a_new_session_of_the_device_replaces_the_old_one() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend_full(
        &store,
        server,
        |_| {},
        |config| {
            config.web_push = Some(settings());
        },
    );
    let alice = start(&backend, &store, server, User::Alice).await;
    let one = session(&store, alice.id, 1).await;
    let other = session(&store, alice.id, 2).await;
    let inbox = mailbox(&backend, &alice, "inbox").await;
    let status = Arc::new(Mutex::new(StatusCode::CREATED));
    let (address, mut pushes) = push_service(Arc::clone(&status)).await;
    let url = format!("http://{address}/push/alice");

    // The old session's subscription, verified.
    let created = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/set", {"create": {"push": create(&url, UA_PUBLIC)}}, "0"]]),
    )
    .await;
    let old = created[0][1]["created"]["push"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    let verification = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(verification["pushSubscriptionId"], old);
    let code = verification["verificationCode"].clone();
    let verified = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/set", {"update": {&old: {"verificationCode": code}}}, "0"]]),
    )
    .await;
    assert!(
        verified[0][1]["updated"].get(&old).is_some(),
        "{verified:?}"
    );

    // Another session that knows the device id but not its push URL can't
    // take this device's notifications away: both must match.
    let third = session(&store, alice.id, 3).await;
    let foreign = format!("http://{address}/push/someone-else");
    let created = call_in(
        &backend,
        &alice,
        third,
        json!([["PushSubscription/set", {"create": {"push": create(&foreign, UA_PUBLIC)}}, "0"]]),
    )
    .await;
    let foreign_id = created[0][1]["created"]["push"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    let verification = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(verification["pushSubscriptionId"], foreign_id);
    let still = call_in(
        &backend,
        &alice,
        one,
        json!([["PushSubscription/get", {}, "0"]]),
    )
    .await;
    assert_eq!(still[0][1]["list"][0]["id"], json!(old), "{still:?}");
    let gone = call_in(
        &backend,
        &alice,
        third,
        json!([["PushSubscription/set", {"destroy": [&foreign_id]}, "0"]]),
    )
    .await;
    assert_eq!(gone[0][1]["destroyed"], json!([foreign_id]), "{gone:?}");

    // The same browser logs in again and sends what lib/web-push.ts does.
    let created = call_in(
        &backend,
        &alice,
        other,
        json!([["PushSubscription/set", {"create": {"push": create(&url, UA_PUBLIC)}}, "0"]]),
    )
    .await;
    let new = created[0][1]["created"]["push"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    assert_ne!(new, old);
    // Its verification, so it is not taken for a push of the old one.
    let verification = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(verification["@type"], "PushVerification");
    assert_eq!(verification["pushSubscriptionId"], new);

    // The old session no longer has it.
    let before = call_in(
        &backend,
        &alice,
        one,
        json!([
            ["PushSubscription/get", {}, "0"],
            ["PushSubscription/set", {"destroy": [&old]}, "1"]
        ]),
    )
    .await;
    assert_eq!(before[0][1]["list"], json!([]), "{before:?}");
    assert_eq!(before[1][1]["notDestroyed"][&old]["type"], "notFound");
    let left: Vec<String> = store
        .deliverable_push_subscriptions(alice.id, UnixTime::now().unwrap())
        .await
        .unwrap()
        .into_iter()
        .map(|row| format!("w{}", row.id.get()))
        .collect();
    assert_eq!(left, std::slice::from_ref(&new));

    // New mail: the old one (verified) is gone and the new one is not
    // verified yet, so nothing is pushed.
    let mail = import(&backend, &alice, &inbox, "push-replaced").await;
    let extra = next(&mut pushes, Duration::from_secs(3)).await;
    assert!(extra.is_none(), "{extra:?}");

    let a = alice.jmap_id.as_str();
    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [mail]}, "0"]]),
    )
    .await;
}

/// Creates and verifies a subscription; returns its id.
async fn subscribe(
    backend: &nebula_jmap::JmapBackend,
    alice: &nebula_http::AccountContext,
    pushes: &mut mpsc::UnboundedReceiver<Pushed>,
    object: Value,
) -> String {
    let created = call(
        backend,
        alice,
        json!([["PushSubscription/set", {"create": {"push": object}}, "0"]]),
    )
    .await;
    let id = created[0][1]["created"]["push"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("not created: {created:?}"))
        .to_owned();
    let verification = next(pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(verification["@type"], "PushVerification");
    let code = verification["verificationCode"].clone();
    let verified = call(
        backend,
        alice,
        json!([["PushSubscription/set", {"update": {&id: {"verificationCode": code}}}, "0"]]),
    )
    .await;
    assert!(verified[0][1]["updated"].get(&id).is_some(), "{verified:?}");
    id
}

/// Stalwart 1.0's `emailPush` through the bridge: the mail inside the
/// push. The mail is "delivered" by another client of the same IMAP
/// account (a second bridge with its own store), which this bridge sees
/// by syncing, as it sees mail the server delivers.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one story, step by step")]
async fn email_push_has_the_mail_inside() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend_full(
        &store,
        server,
        |_| {},
        |config| {
            config.web_push = Some(settings());
        },
    );
    let alice = start(&backend, &store, server, User::Alice).await;
    // `call` uses the login session 1 (push subscriptions belong to one).
    session(&store, alice.id, 1).await;
    let a = alice.jmap_id.as_str();
    let inbox = mailbox(&backend, &alice, "inbox").await;
    let status = Arc::new(Mutex::new(StatusCode::CREATED));
    let (address, mut pushes) = push_service(Arc::clone(&status)).await;
    let url = format!("http://{address}/push/alice-email-push");

    // Announced on the account (and so in the session), with `[push]`.
    let capabilities = backend.account_capabilities(&alice).await.unwrap();
    assert!(capabilities.email_push);
    assert!(
        serde_json::to_value(&capabilities)
            .unwrap()
            .get("urn:ietf:params:jmap:emailpush")
            .is_some()
    );

    // What mail/email-push.ts asks for.
    let web = json!({a: {
        "filter": {"inMailbox": &inbox, "notKeyword": "$seen"},
        "properties": ["id", "from", "subject", "receivedAt", "keywords", "mailboxIds"],
        "urgency": "high"
    }});
    let with = |email_push: Value| {
        let mut object = create(&url, UA_PUBLIC);
        object["emailPush"] = email_push;
        object
    };

    // Refused, never ignored: another account, bodies, text conditions.
    let mut bad = serde_json::Map::new();
    for (name, email_push) in [
        ("account", json!({"zzz": {"properties": ["id"]}})),
        ("body", json!({a: {"properties": ["id", "textBody"]}})),
        ("header", json!({a: {"properties": ["header:Subject"]}})),
        ("text", json!({a: {"filter": {"text": "x"}}})),
        (
            "thread",
            json!({a: {"filter": {"someInThreadHaveKeyword": "$seen"}}}),
        ),
        ("urgency", json!({a: {"urgency": "now"}})),
    ] {
        bad.insert(name.to_owned(), with(email_push));
    }
    let refused = call(
        &backend,
        &alice,
        json!([["PushSubscription/set", {"create": bad}, "0"]]),
    )
    .await;
    assert_eq!(
        refused[0][1]["notCreated"]["account"]["type"], "forbidden",
        "{refused:?}"
    );
    for name in ["body", "header", "text", "thread", "urgency"] {
        assert_eq!(
            refused[0][1]["notCreated"][name]["type"], "invalidProperties",
            "{name}: {refused:?}"
        );
        assert_eq!(
            refused[0][1]["notCreated"][name]["properties"],
            json!(["emailPush"]),
            "{name}: {refused:?}"
        );
    }
    assert!(next(&mut pushes, Duration::from_secs(2)).await.is_none());

    let id = subscribe(&backend, &alice, &mut pushes, with(web)).await;
    // Shown as Stalwart shows it.
    let listed = call(&backend, &alice, json!([["PushSubscription/get", {}, "0"]])).await;
    let listed = &listed[0][1]["list"][0];
    assert_eq!(listed["id"], id);
    assert_eq!(
        listed["emailPush"],
        json!({a: {
            "filter": {"operator": "AND", "conditions": [
                {"inMailbox": &inbox}, {"notKeyword": "$seen"}]},
            "properties": ["id", "from", "subject", "receivedAt", "keywords", "mailboxIds"],
            "urgency": "high"
        }})
    );

    // Our own import is no delivery: a StateChange, as with Stalwart.
    let own = import(&backend, &alice, &inbox, "email-push-own").await;
    let change = next(&mut pushes, Duration::from_secs(10)).await.unwrap();
    assert_eq!(change["@type"], "StateChange", "{change}");

    // Another client of the account adds mail: a read one (the filter
    // leaves it out) and then an unread one.
    let other_store = Store::open(&dir.path().join("other.db")).await.unwrap();
    let other = new_backend_full(&other_store, server, |_| {}, |_| {});
    let other_alice = start(&other, &other_store, server, User::Alice).await;
    let other_inbox = mailbox(&other, &other_alice, "inbox").await;
    import_with(
        &other,
        &other_alice,
        &other_inbox,
        "email-push-read",
        &json!({"$seen": true}),
    )
    .await;
    import_with(
        &other,
        &other_alice,
        &other_inbox,
        "email-push-new",
        &json!({}),
    )
    .await;

    // Inside the push: sender and subject of the unread one only, never
    // the body or the preview, with the urgency asked for.
    let mut found = None;
    for _ in 0..10 {
        let Some((object, urgency)) = next_pushed(&mut pushes, Duration::from_secs(60)).await
        else {
            break;
        };
        // A StateChange of the read one may come first (other types).
        if object["@type"] == "EmailPush" {
            found = Some((object, urgency));
            break;
        }
        assert_eq!(object["@type"], "StateChange", "{object}");
        assert!(
            object["changed"][a].get("EmailDelivery").is_none(),
            "{object}"
        );
    }
    let (email_push, urgency) = found.expect("no EmailPush");
    assert_eq!(urgency, "high");
    assert_eq!(email_push["accountId"], a, "{email_push}");
    assert!(email_push["state"].is_string(), "{email_push}");
    let emails = email_push["emails"].as_array().unwrap();
    assert_eq!(emails.len(), 1, "{email_push}");
    let email = &emails[0];
    assert_eq!(email["subject"], "email-push-new");
    assert_eq!(
        email["from"],
        json!([{"name": "Ana", "email": "ana@example.com"}])
    );
    assert_eq!(email["mailboxIds"], json!({&inbox: true}));
    assert!(email["keywords"].get("$seen").is_none());
    assert!(email["receivedAt"].is_string());
    let delivered = email["id"].as_str().unwrap().to_owned();
    let text = email_push.to_string();
    assert!(
        !text.contains("Hola") && !text.contains("preview"),
        "{text}"
    );

    // A flag change is not a delivery: nothing.
    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "update": {&delivered: {"keywords/$seen": true}}}, "0"]]),
    )
    .await;
    let extra = next(&mut pushes, Duration::from_secs(3)).await;
    assert!(extra.is_none(), "{extra:?}");

    // Without emailPush again (as a whole), new mail is a StateChange.
    let cleared = call(
        &backend,
        &alice,
        json!([["PushSubscription/set", {"update": {&id: {"emailPush": null}}}, "0"],
               ["PushSubscription/get", {"properties": ["emailPush"]}, "1"]]),
    )
    .await;
    assert!(cleared[0][1]["updated"].get(&id).is_some(), "{cleared:?}");
    assert_eq!(cleared[1][1]["list"][0]["emailPush"], Value::Null);

    let mut ids = vec![own, delivered];
    for subject in ["email-push-read"] {
        ids.extend(find(&backend, &alice, subject, Duration::from_secs(30)).await);
    }
    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": ids}, "0"]]),
    )
    .await;
    call(
        &backend,
        &alice,
        json!([["PushSubscription/set", {"destroy": [id]}, "0"]]),
    )
    .await;
    other.account_stopped(other_alice.id).await;
    backend.account_stopped(alice.id).await;
}
