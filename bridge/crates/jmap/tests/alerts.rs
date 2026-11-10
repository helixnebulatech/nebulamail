// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendar alerts end to end: the real HTTP
//! server of the bridge on a TCP port, with the JMAP backend over a real
//! CalDAV server (Stalwart's DAV or Radicale, see `nebula-tests`).
//! Skipped unless `NEBULA_TEST_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test alerts
//! ```
//!
//! Alice logs in through the OAuth form (as the web BFF does) and opens
//! the event source with `types=CalendarAlert`, as
//! apps/web/src/calendar/alerts.ts. Another DAV client puts two events in
//! a new calendar: one with an alert 20 s from now and a daily series
//! whose occurrence today has one too, and a task (`VTODO`) whose reminder
//! is relative to its `DUE` (and a completed one, which must stay quiet).
//! The three `calendarAlert` events must arrive,
//! in Stalwart's form, once. The calendar is deleted at the end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "integration test helpers"
)]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bytes::Bytes;
use http::{Request, StatusCode, header};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use nebula_crypto::{ClientSecretHash, KeyVersion, Keyring, MasterKey};
use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{Component, ComponentSet, ResourceName};
use nebula_dav::{Credentials, DavClient, DavServer, PutCondition, ResourceKind};
use nebula_groupware::{Groupware, GroupwareConfig};
use nebula_http::{AppOrigin, HttpConfig, OAuthClient, PublicUrl, RedirectUri, Server};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::Store;
use nebula_tests::{DavTestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://jmap.example.com";
const APP: &str = "https://mail.example.com";
const REDIRECT: &str = "https://mail.example.com/auth/callback";
const CLIENT_ID: &str = "nebulamail";
const CLIENT_SECRET: &str = "s3cret value";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// The backend with the mail server of `dav` and the DAV server itself,
/// polling every 2 s while an event source is open.
fn backend(store: &Store, dav: DavTestServer) -> JmapBackend {
    let server = dav.mail();
    let imap_port = Port::try_from(server.imap.port).unwrap();
    let smtp_port = Port::try_from(server.smtp.port).unwrap();
    let host: net_guard::Host = nebula_tests::HOST.parse().unwrap();
    let imap = ImapServer::new(host.clone(), imap_port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![imap_port]));
    let resolver: Arc<dyn net_guard::Resolver> = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store.clone(), MailConfig::new(imap), Arc::clone(&resolver)).unwrap();
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
    let mut config = JmapConfig::new(settings);
    config.email_domain = Some(nebula_tests::DOMAIN.to_owned());
    let mut groupware = GroupwareConfig::new(
        DavServer::new(&dav.base_url())
            .unwrap()
            .with_private_network(true),
    );
    groupware.poll_with_push = Duration::from_secs(2);
    let groupware = Groupware::new(store.clone(), groupware, Arc::clone(&resolver));
    JmapBackend::with_groupware(store.clone(), mail, Some(groupware), resolver, config)
}

fn http_config() -> HttpConfig {
    let hash = ClientSecretHash::hash(&SecretString::from(CLIENT_SECRET)).unwrap();
    let client = OAuthClient::new(
        CLIENT_ID.parse().unwrap(),
        hash,
        vec![RedirectUri::parse(REDIRECT).unwrap()],
    )
    .unwrap();
    HttpConfig::new(
        PublicUrl::parse(ISSUER).unwrap(),
        vec![AppOrigin::parse(APP).unwrap()],
        vec![client],
    )
    .unwrap()
}

/// One request on its own connection to the bridge.
async fn send(port: u16, request: Request<Full<Bytes>>) -> http::Response<Incoming> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(connection);
    sender.send_request(request).await.unwrap()
}

async fn text(response: http::Response<Incoming>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn form(params: &[(&str, &str)]) -> Full<Bytes> {
    Full::new(Bytes::from(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params)
            .finish(),
    ))
}

/// Logs in through the OAuth form and the token endpoint, as the BFF.
async fn access_token(port: u16, login: &str, password: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", CLIENT_ID),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("redirect_uri", REDIRECT),
            ("response_type", "code"),
            ("scope", "urn:ietf:params:oauth:scope:mail offline_access"),
            ("state", "s"),
        ])
        .finish();
    let page = send(
        port,
        Request::get(format!("/oauth/authorize?{query}"))
            .header(header::HOST, "jmap.example.com")
            .body(Full::default())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let cookie = page.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = text(page).await;
    let marker = "name=\"pending\" value=\"";
    let start = html.find(marker).unwrap() + marker.len();
    let pending = html.get(start..start + 43).unwrap().to_owned();
    let posted = send(
        port,
        Request::post("/oauth/authorize")
            .header(header::HOST, "jmap.example.com")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie)
            .header("sec-fetch-site", "same-origin")
            .body(form(&[
                ("pending", &pending),
                ("login", login),
                ("password", password),
                ("action", "login"),
            ]))
            .unwrap(),
    )
    .await;
    assert_eq!(posted.status(), StatusCode::SEE_OTHER);
    let location = posted.headers()[header::LOCATION].to_str().unwrap();
    let code = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned();
    let basic = base64::engine::general_purpose::STANDARD.encode(format!(
        "{CLIENT_ID}:{}",
        url::form_urlencoded::byte_serialize(CLIENT_SECRET.as_bytes()).collect::<String>()
    ));
    let token = send(
        port,
        Request::post("/oauth/token")
            .header(header::HOST, "jmap.example.com")
            .header(header::AUTHORIZATION, format!("Basic {basic}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT, "application/json")
            .body(form(&[
                ("code", &code),
                ("code_verifier", VERIFIER),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT),
            ]))
            .unwrap(),
    )
    .await;
    assert_eq!(token.status(), StatusCode::OK);
    let json: Value = serde_json::from_str(&text(token).await).unwrap();
    json["access_token"].as_str().unwrap().to_owned()
}

/// `20261027T161144Z`.
fn ical_utc(at: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

fn event(uid: &str, start: OffsetDateTime, rule: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//alerts test//EN\r\nBEGIN:VEVENT\r\n\
         UID:{uid}\r\nDTSTAMP:{stamp}\r\nDTSTART:{start}\r\nDURATION:PT30M\r\n{rule}\
         SUMMARY:Alert test\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Alert test\r\n\
         TRIGGER:-PT60S\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        stamp = ical_utc(OffsetDateTime::now_utc()),
        start = ical_utc(start),
    )
}

/// A task due at `due` with a reminder 60 s before it (`RELATED=END`).
fn task(uid: &str, due: OffsetDateTime, status: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//alerts test//EN\r\nBEGIN:VTODO\r\n\
         UID:{uid}\r\nDTSTAMP:{stamp}\r\nDUE:{due}\r\nSTATUS:{status}\r\nSUMMARY:Task alert\r\n\
         BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Task alert\r\n\
         TRIGGER;RELATED=END:-PT60S\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
        stamp = ical_utc(OffsetDateTime::now_utc()),
        due = ical_utc(due),
    )
}

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nebula-test-{prefix}-{nanos}")
}

#[tokio::test(flavor = "multi_thread")]
async fn alerts_arrive_on_the_event_source() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("bridge.db")).await.unwrap();
    let shutdown = CancellationToken::new();
    let keyring = Keyring::new(
        MasterKey::from_bytes(KeyVersion::new(1).unwrap(), &[7; 32]).unwrap(),
        [],
    )
    .unwrap();
    let server = Server::new(
        http_config(),
        store.clone(),
        keyring,
        backend(&store, dav),
        shutdown.clone(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = tokio::spawn({
        let server = server.clone();
        async move { server.serve(listener).await }
    });

    let password = dav.password(User::Alice);
    let token = access_token(port, dav.login(User::Alice), password.expose_secret()).await;
    let session: Value = serde_json::from_str(
        &text(
            send(
                port,
                Request::get("/.well-known/jmap")
                    .header(header::HOST, "jmap.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Full::default())
                    .unwrap(),
            )
            .await,
        )
        .await,
    )
    .unwrap();
    let account = session["primaryAccounts"]["urn:ietf:params:jmap:mail"]
        .as_str()
        .unwrap()
        .to_owned();
    // The web's subscription.
    let events = send(
        port,
        Request::get("/jmap/eventsource/?types=CalendarAlert&closeafter=no&ping=0")
            .header(header::HOST, "jmap.example.com")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "text/event-stream")
            .body(Full::default())
            .unwrap(),
    )
    .await;
    assert_eq!(events.status(), StatusCode::OK);
    let mut body = events.into_body();

    // Another client: a new calendar, a single event whose alert rings in
    // 20 s, and a daily series (from yesterday) whose occurrence today
    // rings then too.
    let other = DavClient::new(
        DavServer::new(&dav.base_url())
            .unwrap()
            .with_private_network(true),
        Some(Credentials::new(dav.login(User::Alice), dav.password(User::Alice)).unwrap()),
    )
    .unwrap();
    let found = other.discover().await.unwrap();
    let name = unique("alerts");
    let href = found.calendar_homes[0]
        .member(&ResourceName::try_from(name.as_str()).unwrap(), true)
        .unwrap();
    other
        .mkcalendar(
            &href,
            &[
                PropValue::DisplayName(name.clone()),
                PropValue::SupportedComponents(ComponentSet::new(&[
                    Component::Event,
                    Component::Todo,
                ])),
            ],
        )
        .await
        .unwrap();
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let start = now + Duration::from_secs(80);
    let single = format!("{name}-single");
    let daily = format!("{name}-daily");
    let todo = format!("{name}-todo");
    let done = format!("{name}-todo-done");
    for (uid, text) in [
        (&single, event(&single, start, "")),
        (&todo, task(&todo, start, "NEEDS-ACTION")),
        (&done, task(&done, start, "COMPLETED")),
        (
            &daily,
            event(
                &daily,
                start - Duration::from_hours(24),
                "RRULE:FREQ=DAILY;COUNT=5\r\n",
            ),
        ),
    ] {
        let member = href
            .member(
                &ResourceName::try_from(format!("{uid}.ics").as_str()).unwrap(),
                false,
            )
            .unwrap();
        other
            .put(
                &member,
                ResourceKind::Calendar,
                &text,
                &PutCondition::Create,
            )
            .await
            .unwrap();
    }

    // Both alerts, once, in Stalwart's form.
    let mut stream = String::new();
    let mut alerts: Vec<Value> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while alerts.len() < 3 {
        let frame = tokio::time::timeout_at(deadline, body.frame())
            .await
            .unwrap_or_else(|_| panic!("no alerts in time; got {stream:?}"))
            .unwrap()
            .unwrap();
        if let Ok(data) = frame.into_data() {
            stream.push_str(std::str::from_utf8(&data).unwrap());
        }
        while let Some(end) = stream.find("\n\n") {
            let event: String = stream.drain(..end + 2).collect();
            if let Some(data) = event
                .strip_prefix("event: calendarAlert\ndata: ")
                .map(str::trim_end)
            {
                alerts.push(serde_json::from_str(data).unwrap());
            }
        }
    }
    let fired_at = OffsetDateTime::now_utc();
    // Not before its time (start − 60 s), and not much later.
    assert!(fired_at >= start - Duration::from_secs(61), "{fired_at}");
    alerts.sort_by_key(|alert| alert["uid"].as_str().unwrap().to_owned());
    let (series, one, reminder) = (&alerts[0], &alerts[1], &alerts[2]);
    // The task's, relative to its due; never the completed one's.
    assert_eq!(reminder["uid"], todo.as_str(), "{alerts:?}");
    assert_eq!(reminder["alertId"], "k1");
    assert_eq!(reminder["recurrenceId"], Value::Null);
    assert_eq!(one["@type"], "CalendarAlert");
    assert_eq!(one["accountId"], account.as_str());
    assert_eq!(one["uid"], single.as_str());
    assert_eq!(one["alertId"], "k1");
    assert_eq!(one["recurrenceId"], Value::Null);
    assert!(one["calendarEventId"].as_str().unwrap().starts_with('v'));
    assert_eq!(series["uid"], daily.as_str());
    let local = start.format(time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second]"
    ));
    assert_eq!(series["recurrenceId"], local.unwrap().as_str());
    // Nothing else (no repeat) within a few seconds.
    let extra = tokio::time::timeout(Duration::from_secs(3), body.frame()).await;
    if let Ok(Some(Ok(frame))) = extra
        && let Ok(data) = frame.into_data()
    {
        assert!(
            !std::str::from_utf8(&data)
                .unwrap()
                .contains("calendarAlert"),
            "an alert came twice"
        );
    }
    // The event the web reads with that id is the one.
    let get = serde_json::json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
        "methodCalls": [["CalendarEvent/get", {
            "accountId": account, "ids": [one["calendarEventId"]],
            "properties": ["title", "alerts"]
        }, "0"]]
    });
    let reply: Value = serde_json::from_str(
        &text(
            send(
                port,
                Request::post("/jmap/api")
                    .header(header::HOST, "jmap.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(get.to_string())))
                    .unwrap(),
            )
            .await,
        )
        .await,
    )
    .unwrap();
    let got = &reply["methodResponses"][0][1]["list"][0];
    assert_eq!(got["title"], "Alert test");
    assert!(got["alerts"]["k1"].is_object(), "{reply}");

    drop(body);
    other.delete(&href, None).await.unwrap();
    shutdown.cancel();
    let _ = serving.await;
}
