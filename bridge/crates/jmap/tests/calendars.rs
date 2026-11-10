// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP Calendars through the bridge against the DAV servers of
//! `docker/compose.yaml`.
//!
//! Skipped unless `NEBULA_TEST_DAV=1`; `NEBULA_TEST_SERVER=stalwart`
//! (default: Stalwart's IMAP and DAV) or `radicale` (Dovecot's IMAP,
//! Radicale's DAV):
//!
//! ```sh
//! NEBULA_TEST_DAV=1 cargo test -p nebula-jmap --test calendars
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test calendars
//! ```
//!
//! Another DAV client puts the corpus (`groupware/tests/data/corpus`) in a
//! new calendar; the bridge syncs it and answers the exact requests of
//! `apps/web/src/calendar/api.ts` and `invitations.ts`. With Stalwart it
//! is also the **differential test**: the same queries to Stalwart's own
//! JMAP (which reads the same calendar) and to the bridge (whose DAV
//! upstream is that Stalwart) must give the same events, except the
//! differences listed in [`KNOWN`]. The calendar is deleted at the end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::print_stderr,
    reason = "integration test helpers; the differences are printed for the report"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bytes::Bytes;
use futures::StreamExt as _;
use http_body_util::{BodyExt as _, Full};
use hyper_util::rt::TokioIo;
use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{Component, ComponentSet, Href, ResourceName};
use nebula_dav::{Credentials, DavClient, DavServer, PutCondition, ResourceKind};
use nebula_groupware::{Groupware, GroupwareConfig};
use nebula_http::{
    AccountContext, ApiCall, Backend, LoginOutcome, StartReason, UploadBody, jmap_account_id,
};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{NewAccount, Store, UnixTime};
use nebula_tests::{DavKind, DavTestServer, ServerKind, TestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};

/// Differences between Stalwart 1.0.1 and the bridge (both calcard 0.4)
/// that are expected: `(key fragment, field)`, the key being
/// `uid|recurrenceId` for occurrences and `uid` for series and parse.
/// Anything else fails the test (4.4).
const KNOWN: &[(&str, &str)] = &[
    // Stalwart keeps the first 3000 occurrences of a series from its start
    // (`maxRecurrenceExpansions`, at write time): a daily event since 2015
    // is gone by 2023. The bridge expands up to 10 000 (and calcard's
    // budget of unproductive iterations) at query time, so it still shows
    // (its occurrences and so its series).
    ("nebula-corpus-daily-old", "(presence)"),
    // A recurring event in UTC (`DTSTART:…Z` + RRULE, as Google exports):
    // Stalwart (0.16.27 and 1.0.1) reads its occurrences as floating times
    // in the query's zone (one hour early in Madrid, four late in New
    // York); the bridge keeps them in UTC, like Stalwart does for a single
    // event.
    ("nebula-corpus-google@google.com|", "(presence)"),
    // An occurrence asked for `recurrenceRule`: Stalwart sends `null`, the
    // bridge leaves it out (a JSCalendar body here has no top-level nulls).
    ("|", "recurrenceRule"),
    // Occurrences of an all-day series: Stalwart 1.0 gives them
    // `showWithoutTime` and no zone, the bridge (like 0.16.27) neither and
    // the query's zone; the web takes "all day" from the series.
    ("nebula-corpus-allday-yearly|", "showWithoutTime"),
    ("nebula-corpus-allday-yearly|", "timeZone"),
    // Stalwart 1.0's parse adds `isDraft: null`, `isOrigin: null` and
    // `version: "2.0"` (JSCalendar); the bridge leaves them out (the web
    // reads none of them).
    ("", "isDraft"),
    ("", "isOrigin"),
    ("", "version"),
];

/// The properties `listAccountOccurrences` asks for (api.ts).
const OCCURRENCE_PROPERTIES: &[&str] = &[
    "calendarIds",
    "title",
    "start",
    "duration",
    "utcStart",
    "utcEnd",
    "baseEventId",
    "recurrenceId",
    "status",
    "participants",
    "organizerCalendarAddress",
    "locations",
    "isOrigin",
];

/// The series' properties (api.ts).
const SERIES_PROPERTIES: &[&str] = &[
    "calendarIds",
    "title",
    "showWithoutTime",
    "timeZone",
    "recurrenceRule",
    "organizerCalendarAddress",
    "participants",
    "status",
    "isOrigin",
];

/// `getEvent` (api.ts): the occurrence's and these.
const DETAIL_EXTRA: &[&str] = &[
    "uid",
    "description",
    "descriptionContentType",
    "virtualLocations",
    "alerts",
    "sequence",
    "showWithoutTime",
    "timeZone",
    "recurrenceRule",
];

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nebula-test-{prefix}-{nanos}")
}

/// The corpus: (name, CRLF text). `reply-*` files are iTIP messages, only
/// for `CalendarEvent/parse` (a calendar does not store `METHOD`).
fn corpus() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../groupware/tests/data/corpus");
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace('\n', "\r\n");
            (name, text)
        })
        // Tasks (`task-*`): Stalwart 0.16's JMAP has no tasks to compare
        // with; they go through `server/tasks.rs`.
        .filter(|(name, _)| !name.starts_with("task-"))
        .collect();
    files.sort();
    files
}

fn stored_corpus() -> Vec<(String, String)> {
    corpus()
        .into_iter()
        .filter(|(name, _)| !name.starts_with("reply-"))
        .collect()
}

fn mail_server(dav: DavTestServer) -> TestServer {
    TestServer::of(match dav.kind {
        DavKind::Stalwart => ServerKind::Stalwart,
        DavKind::Radicale | DavKind::Nextcloud => ServerKind::Dovecot,
    })
}

fn new_backend(store: &Store, dav: DavTestServer) -> JmapBackend {
    let server = mail_server(dav);
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
    let dav_server = DavServer::new(&dav.base_url())
        .unwrap()
        .with_private_network(true);
    let mut groupware_config = GroupwareConfig::new(dav_server);
    // With the push stream open the test waits at most a second.
    groupware_config.poll_with_push = Duration::from_secs(1);
    let groupware = Groupware::new(store.clone(), groupware_config, Arc::clone(&resolver));
    JmapBackend::with_groupware(store.clone(), mail, Some(groupware), resolver, config)
}

async fn start(backend: &JmapBackend, store: &Store, dav: DavTestServer) -> AccountContext {
    let server = mail_server(dav);
    let password = || server.password(User::Alice);
    let login = User::Alice.login();
    let outcome = backend
        .verify_login(&login.parse().unwrap(), &password())
        .await;
    let LoginOutcome::Success { email } = outcome else {
        panic!("login failed: {outcome:?}")
    };
    let account = store
        .get_or_create_account(
            NewAccount {
                login,
                email: &email,
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap();
    let context = AccountContext {
        id: account.id,
        jmap_id: jmap_account_id(account.id).unwrap(),
        login: login.to_owned(),
        email,
    };
    let key = nebula_crypto::AccountKey::new(
        nebula_crypto::AccountId::new(format!("login:{login}").as_bytes()).unwrap(),
        nebula_crypto::DataKey::generate().unwrap(),
    );
    backend
        .account_started(&context, password(), key, StartReason::Login)
        .await
        .unwrap();
    context
}

const USING: &[&str] = &[
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:mail",
    "urn:ietf:params:jmap:calendars",
    "urn:ietf:params:jmap:calendars:parse",
];

/// Runs one request through the bridge; the method responses.
async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({"using": USING, "methodCalls": calls});
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        &[
            Capability::Core,
            Capability::Mail,
            Capability::Calendars,
            Capability::CalendarsParse,
        ],
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                // Push subscriptions are per session; nothing else looks at it.
                session: nebula_store::GrantId::from_raw(1).unwrap(),
                session_state: State::new("s"),
                languages: nebula_http::AcceptLanguage::default(),
                limits: MethodLimits {
                    max_objects_in_get: 500,
                    max_objects_in_set: 500,
                    max_size_request: 10_000_000,
                },
            },
        )
        .await
        .unwrap();
    let value = serde_json::to_value(response).unwrap();
    value["methodResponses"].as_array().unwrap().clone()
}

/// Stalwart's own JMAP (plain HTTP on loopback).
struct Stalwart {
    authorization: String,
    account: String,
}

impl Stalwart {
    async fn new(dav: DavTestServer) -> Self {
        let password = dav.password(User::Alice);
        let token = base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            User::Alice.login(),
            password.expose_secret()
        ));
        let mut stalwart = Self {
            authorization: format!("Basic {token}"),
            account: String::new(),
        };
        let session = stalwart.http("GET", "/jmap/session", None, "").await;
        session["primaryAccounts"]["urn:ietf:params:jmap:calendars"]
            .as_str()
            .unwrap()
            .clone_into(&mut stalwart.account);
        stalwart
    }

    async fn http(&self, method: &str, path: &str, body: Option<Vec<u8>>, media: &str) -> Value {
        let stream = tokio::net::TcpStream::connect("127.0.0.1:8080")
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let mut request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(http::header::HOST, "localhost:8080")
            .header(http::header::AUTHORIZATION, &self.authorization);
        if !media.is_empty() {
            request = request.header(http::header::CONTENT_TYPE, media);
        }
        let request = request
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| panic!("Stalwart: {}", String::from_utf8_lossy(&bytes)))
    }

    async fn call(&self, calls: Value) -> Vec<Value> {
        let body = json!({"using": USING, "methodCalls": calls});
        let value = self
            .http(
                "POST",
                "/jmap/",
                Some(body.to_string().into_bytes()),
                "application/json",
            )
            .await;
        value["methodResponses"].as_array().unwrap().clone()
    }

    async fn upload(&self, data: &str) -> String {
        let value = self
            .http(
                "POST",
                &format!("/jmap/upload/{}/", self.account),
                Some(data.as_bytes().to_vec()),
                "text/calendar",
            )
            .await;
        value["blobId"].as_str().unwrap().to_owned()
    }
}

/// The "other client": the DAV client on its own.
fn other_client(dav: DavTestServer) -> DavClient {
    let config = DavServer::new(&dav.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials = Credentials::new(dav.login(User::Alice), dav.password(User::Alice)).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

/// A new calendar named `name` with every stored corpus file in it.
async fn scratch_calendar(client: &DavClient, name: &str) -> Href {
    let found = client.discover().await.unwrap();
    let segment = ResourceName::try_from(unique("calendars").as_str()).unwrap();
    let href = found.calendar_homes[0].member(&segment, true).unwrap();
    let display = PropValue::DisplayName(name.to_owned());
    let components = PropValue::SupportedComponents(ComponentSet::new(&[Component::Event]));
    client
        .mkcalendar(&href, &[display, components])
        .await
        .unwrap();
    for (file, text) in stored_corpus() {
        let member = href
            .member(
                &ResourceName::try_from(format!("{file}.ics").as_str()).unwrap(),
                false,
            )
            .unwrap();
        client
            .put(
                &member,
                ResourceKind::Calendar,
                &text,
                &PutCondition::Create,
            )
            .await
            .unwrap_or_else(|e| panic!("PUT {file}: {e}"));
    }
    href
}

fn calendar_id(get: &Value, name: &str) -> String {
    get["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no calendar {name}: {get}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Waits until the bridge has every corpus event in the calendar.
async fn synced(backend: &JmapBackend, account: &AccountContext, name: &str) -> String {
    let id = account.jmap_id.as_str();
    let want = stored_corpus().len();
    for _ in 0..120 {
        let responses = call(
            backend,
            account,
            json!([["Calendar/get", {"accountId": id}, "0"]]),
        )
        .await;
        if responses[0][0] == "Calendar/get"
            && let Some(calendar) = responses[0][1]["list"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
        {
            let calendar = calendar["id"].as_str().unwrap().to_owned();
            let found = call(
                backend,
                account,
                json!([["CalendarEvent/query", {"accountId": id, "filter": {"inCalendar": calendar}}, "0"]]),
            )
            .await;
            if found[0][1]["ids"].as_array().map(Vec::len) == Some(want) {
                return calendar;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the bridge did not sync the corpus")
}

/// The web's week: expanded query, occurrences with their series (one
/// request with a back-reference, as api.ts). Returns (occurrences, series).
async fn week(
    run: impl AsyncFn(Value) -> Vec<Value>,
    account: &str,
    calendar: Option<&str>,
    (after, before, zone): (&str, &str, &str),
    occurrence_properties: &[&str],
) -> (Vec<Value>, Vec<Value>) {
    let mut filter = json!({"after": after, "before": before});
    if let Some(calendar) = calendar {
        filter["inCalendar"] = json!(calendar);
    }
    let found = run(json!([["CalendarEvent/query", {
        "accountId": account, "expandRecurrences": true, "filter": filter,
        "sort": [{"isAscending": true, "property": "start"}], "timeZone": zone
    }, "q"]]))
    .await;
    assert_eq!(found[0][0], "CalendarEvent/query", "{}", found[0]);
    let ids = found[0][1]["ids"].clone();
    let responses = run(json!([
        ["CalendarEvent/get", {"accountId": account, "ids": ids,
            "properties": occurrence_properties, "timeZone": zone}, "g"],
        ["CalendarEvent/get", {"accountId": account,
            "#ids": {"resultOf": "g", "name": "CalendarEvent/get", "path": "/list/*/baseEventId"},
            "properties": SERIES_PROPERTIES}, "b"]
    ]))
    .await;
    assert_eq!(responses[0][0], "CalendarEvent/get", "{}", responses[0]);
    assert_eq!(responses[1][0], "CalendarEvent/get", "{}", responses[1]);
    let list = |v: &Value| v[1]["list"].as_array().unwrap().clone();
    (list(&responses[0]), list(&responses[1]))
}

/// An event without ids, with maps turned into sorted lists of values (the
/// keys are ids each server picks).
fn normalized(event: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (key, value) in event.as_object().unwrap() {
        if matches!(key.as_str(), "id" | "baseEventId" | "calendarIds") {
            continue;
        }
        let value = match (key.as_str(), value) {
            (
                "participants" | "alerts" | "locations" | "virtualLocations" | "links",
                Value::Object(map),
            ) => {
                let mut values: Vec<Value> = map.values().cloned().collect();
                values.sort_by_key(Value::to_string);
                Value::Array(values)
            }
            _ => value.clone(),
        };
        out.insert(key.clone(), value);
    }
    out
}

/// Field by field differences: `uid fragment|key: field: stalwart → bridge`.
fn compare(
    label: &str,
    stalwart: &BTreeMap<String, BTreeMap<String, Value>>,
    bridge: &BTreeMap<String, BTreeMap<String, Value>>,
    diffs: &mut Vec<(String, String, String)>,
) {
    for key in stalwart.keys().chain(bridge.keys()) {
        let (a, b) = (stalwart.get(key), bridge.get(key));
        let (Some(a), Some(b)) = (a, b) else {
            diffs.push((
                key.clone(),
                "(presence)".to_owned(),
                format!("{label}: stalwart={} bridge={}", a.is_some(), b.is_some()),
            ));
            continue;
        };
        for field in a.keys().chain(b.keys()) {
            if a.get(field) != b.get(field) {
                diffs.push((
                    key.clone(),
                    field.clone(),
                    format!(
                        "{label}: stalwart={} bridge={}",
                        a.get(field).map_or("-".to_owned(), Value::to_string),
                        b.get(field).map_or("-".to_owned(), Value::to_string)
                    ),
                ));
            }
        }
    }
    diffs.sort();
    diffs.dedup();
}

/// Occurrences keyed by `uid|recurrenceId` (the get asks for `uid`).
fn by_occurrence(list: &[Value]) -> BTreeMap<String, BTreeMap<String, Value>> {
    list.iter()
        .map(|event| {
            let key = format!(
                "{}|{}",
                event["uid"].as_str().unwrap_or("?"),
                event["recurrenceId"].as_str().unwrap_or("-")
            );
            (key, normalized(event))
        })
        .collect()
}

fn by_uid(list: &[Value]) -> BTreeMap<String, BTreeMap<String, Value>> {
    list.iter()
        .map(|event| {
            (
                event["uid"].as_str().unwrap_or("?").to_owned(),
                normalized(event),
            )
        })
        .collect()
}

#[tokio::test]
async fn calendars_through_the_bridge() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, dav);
    let alice = start(&backend, &store, dav).await;
    let id = alice.jmap_id.as_str().to_owned();
    // Push stream open: sync every second, and changes are pushed.
    let mut pushes = backend.events(&alice, true).await.unwrap();

    // The capability comes once discovery found a calendar home.
    let mut announced = None;
    for _ in 0..120 {
        let capabilities = backend.account_capabilities(&alice).await.unwrap();
        if capabilities.calendars.is_some() {
            announced = Some(capabilities);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let announced = announced.expect("calendars announced");
    assert!(announced.calendars_parse);
    let session = serde_json::to_value(&announced).unwrap();
    assert_eq!(
        session["urn:ietf:params:jmap:calendars"]["maxExpandedQueryDuration"],
        "P52W1D"
    );

    let other = other_client(dav);
    let name = unique("differential");
    let href = scratch_calendar(&other, &name).await;
    let calendar = synced(&backend, &alice, &name).await;

    // Push: Calendar and CalendarEvent states went out.
    let mut types_pushed: Vec<String> = Vec::new();
    while let Ok(Some(change)) =
        tokio::time::timeout(Duration::from_millis(200), pushes.next()).await
    {
        let nebula_http::PushEvent::State(change) = change else {
            continue;
        };
        let value = serde_json::to_value(&change).unwrap();
        for types in value["changed"].as_object().unwrap().values() {
            types_pushed.extend(types.as_object().unwrap().keys().cloned());
        }
    }
    assert!(
        types_pushed.iter().any(|t| t == "CalendarEvent"),
        "{types_pushed:?}"
    );
    assert!(
        types_pushed.iter().any(|t| t == "Calendar"),
        "{types_pushed:?}"
    );

    // listCalendars, listOwnAddresses / me.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Calendar/get", {"accountId": id}, "0"],
            ["ParticipantIdentity/get", {"accountId": id}, "1"],
            ["Calendar/changes", {"accountId": id, "sinceState": "0"}, "2"],
            ["CalendarEvent/changes", {"accountId": id, "sinceState": "0", "maxChanges": 1000}, "3"]
        ]),
    )
    .await;
    let mine = responses[0][1]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == calendar.as_str())
        .unwrap()
        .clone();
    assert_eq!(mine["name"], name.as_str());
    assert_eq!(mine["myRights"]["mayReadItems"], true);
    let identities = responses[1][1]["list"].as_array().unwrap();
    assert_eq!(
        identities[0]["calendarAddress"],
        "mailto:alice@nebulamail.test"
    );
    assert_eq!(identities[0]["isDefault"], true);
    assert!(responses[1][1]["state"].is_string());
    assert!(
        responses[2][1]["created"]
            .as_array()
            .unwrap()
            .contains(&json!(calendar))
    );
    assert!(responses[3][1]["created"].as_array().unwrap().len() >= stored_corpus().len());

    // The web's week (Madrid), every occurrence with its series.
    let bridge_run = async |calls: Value| call(&backend, &alice, calls).await;
    let window = (
        "2026-11-02T00:00:00",
        "2026-11-09T00:00:00",
        "Europe/Madrid",
    );
    let (occurrences, series) = week(bridge_run, &id, None, window, OCCURRENCE_PROPERTIES).await;
    let ours: Vec<&Value> = occurrences
        .iter()
        .filter(|o| o["calendarIds"].get(&calendar).is_some())
        .collect();
    assert!(!ours.is_empty());
    for occurrence in &ours {
        // Every occurrence has its series; an event that doesn't repeat
        // comes as itself, under its own id.
        let id = occurrence["id"].as_str().unwrap();
        match occurrence["baseEventId"].as_str() {
            Some(base) => {
                assert!(series.iter().any(|s| s["id"] == base), "{occurrence}");
                assert!(id.contains('_'), "{occurrence}");
            }
            None => assert!(!id.contains('_'), "{occurrence}"),
        }
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        );
        assert!(occurrence["utcStart"].as_str().unwrap().ends_with('Z'));
    }
    let moved = ours
        .iter()
        .find(|o| o["title"] == "Reunión semanal (movida)")
        .expect("the moved occurrence");
    assert_eq!(moved["recurrenceId"], "2026-11-02T10:00:00");
    assert_eq!(moved["start"], "2026-11-02T12:00:00");
    assert_eq!(moved["utcStart"], "2026-11-02T11:00:00Z");

    // getEvent: an occurrence and its series.
    let occurrence = moved["id"].as_str().unwrap();
    let base = moved["baseEventId"].as_str().unwrap();
    let detail: Vec<&str> = OCCURRENCE_PROPERTIES
        .iter()
        .chain(DETAIL_EXTRA)
        .copied()
        .collect();
    let responses = call(
        &backend,
        &alice,
        json!([["CalendarEvent/get", {"accountId": id, "ids": [occurrence, base],
            "properties": detail, "timeZone": "Europe/Madrid"}, "0"]]),
    )
    .await;
    let list = responses[0][1]["list"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{}", responses[0]);
    assert_eq!(list[0]["recurrenceRule"], Value::Null);
    assert_eq!(list[1]["recurrenceRule"]["frequency"], "weekly");
    assert_eq!(list[1]["baseEventId"], Value::Null);

    // findByUid (invitations.ts): query by uid, then get by reference.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["CalendarEvent/query", {"accountId": id, "filter": {"uid": "nebula-corpus-attendees-invited"}}, "q"],
            ["CalendarEvent/get", {"accountId": id,
                "#ids": {"resultOf": "q", "name": "CalendarEvent/query", "path": "/ids"},
                "properties": ["uid", "participants", "status", "sequence", "start", "duration", "timeZone"]}, "g"]
        ]),
    )
    .await;
    let found = responses[1][1]["list"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{}", responses[1]);
    assert_eq!(found[0]["sequence"], 1);
    assert_eq!(found[0]["duration"], "PT1H30M");

    // loadInvitation: CalendarEvent/parse of an uploaded .ics.
    let (_, reply) = corpus()
        .into_iter()
        .find(|(name, _)| name.starts_with("reply-"))
        .unwrap();
    let uploaded = backend
        .upload(
            &alice,
            "text/calendar".parse().unwrap(),
            UploadBody::from_chunks(vec![Bytes::from(reply.clone())], 1_000_000),
        )
        .await
        .unwrap();
    let blob = uploaded.blob_id.as_str().to_owned();
    let responses = call(
        &backend,
        &alice,
        json!([["CalendarEvent/parse", {"accountId": id, "blobIds": [blob, "Gnothing"]}, "0"]]),
    )
    .await;
    let parsed = &responses[0][1];
    assert_eq!(
        parsed["parsed"][&blob][0]["uid"],
        "nebula-corpus-allday-series"
    );
    assert_eq!(parsed["notFound"], json!(["Gnothing"]));

    // What Stalwart refuses too.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["CalendarEvent/query", {"accountId": id, "filter": {"color": "x"}}, "0"],
            ["CalendarEvent/query", {"accountId": id, "sort": [{"property": "created"}]}, "1"],
            ["CalendarEvent/query", {"accountId": id, "expandRecurrences": true,
                "filter": {"after": "2026-01-01T00:00:00", "before": "2027-06-01T00:00:00"}}, "2"],
            ["CalendarEvent/query", {"accountId": id, "expandRecurrences": true,
                "filter": {"after": "2026-01-01T00:00:00"}}, "3"],
            ["CalendarEvent/get", {"accountId": id, "ids": ["v1"], "timeZone": "Mars/Olympus"}, "4"]
        ]),
    )
    .await;
    let kinds: Vec<&str> = responses
        .iter()
        .map(|r| r[1]["type"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(
        kinds,
        [
            "unsupportedFilter",
            "unsupportedSort",
            "expandDurationTooLarge",
            "invalidArguments",
            "invalidArguments"
        ]
    );

    let unexpected = if dav.kind == DavKind::Stalwart {
        differential(dav, &backend, &alice, &calendar, &name).await
    } else {
        0
    };

    // Clean up first, then fail on unexpected differences.
    other.delete(&href, None).await.unwrap();
    assert_eq!(unexpected, 0, "unexpected differences with Stalwart");
    for _ in 0..60 {
        let responses = call(
            &backend,
            &alice,
            json!([["Calendar/get", {"accountId": id, "ids": [calendar]}, "0"]]),
        )
        .await;
        if responses[0][1]["notFound"] == json!([calendar]) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the deleted calendar is still there");
}

/// The same corpus through Stalwart's JMAP and through the bridge: how
/// many differences are not in [`KNOWN`] (all are printed).
async fn differential(
    dav: DavTestServer,
    backend: &JmapBackend,
    alice: &AccountContext,
    calendar: &str,
    name: &str,
) -> usize {
    let stalwart = Stalwart::new(dav).await;
    let theirs = stalwart
        .call(json!([["Calendar/get", {"accountId": stalwart.account}, "0"]]))
        .await;
    let their_calendar = calendar_id(&theirs[0][1], name);
    // Stalwart indexes in the background: wait for it.
    for _ in 0..60 {
        let found = stalwart
            .call(
                json!([["CalendarEvent/query", {"accountId": stalwart.account,
                "filter": {"inCalendar": their_calendar}}, "0"]]),
            )
            .await;
        if found[0][1]["ids"].as_array().map(Vec::len) == Some(stored_corpus().len()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let id = alice.jmap_id.as_str();
    let properties: Vec<&str> = OCCURRENCE_PROPERTIES
        .iter()
        .chain(DETAIL_EXTRA)
        .copied()
        .collect();
    let mut diffs: Vec<(String, String, String)> = Vec::new();
    let windows = [
        (
            "2026-11-02T00:00:00",
            "2026-11-09T00:00:00",
            "Europe/Madrid",
        ),
        (
            "2026-10-19T00:00:00",
            "2026-11-16T00:00:00",
            "America/New_York",
        ),
    ];
    for window in windows {
        let label = format!("{}..{} {}", window.0, window.1, window.2);
        let (their_list, their_series) = week(
            async |calls: Value| stalwart.call(calls).await,
            &stalwart.account,
            Some(&their_calendar),
            window,
            &properties,
        )
        .await;
        let (our_list, our_series) = week(
            async |calls: Value| call(backend, alice, calls).await,
            id,
            Some(calendar),
            window,
            &properties,
        )
        .await;
        compare(
            &label,
            &by_occurrence(&their_list),
            &by_occurrence(&our_list),
            &mut diffs,
        );
        // The series: ask for uid too, to key them.
        let series_ids = |list: &[Value]| -> Vec<String> {
            let mut ids: Vec<String> = list
                .iter()
                .filter_map(|s| s["id"].as_str().map(str::to_owned))
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let (their_ids, our_ids) = (series_ids(&their_series), series_ids(&our_series));
        let mut full: Vec<&str> = SERIES_PROPERTIES.to_vec();
        full.extend(["uid", "start", "duration", "recurrenceOverrides", "alerts"]);
        let theirs = stalwart
            .call(json!([["CalendarEvent/get", {"accountId": stalwart.account, "ids": their_ids, "properties": full}, "0"]]))
            .await;
        let ours = call(
            backend,
            alice,
            json!([["CalendarEvent/get", {"accountId": id, "ids": our_ids, "properties": full}, "0"]]),
        )
        .await;
        compare(
            &format!("series {label}"),
            &by_uid(theirs[0][1]["list"].as_array().unwrap()),
            &by_uid(ours[0][1]["list"].as_array().unwrap()),
            &mut diffs,
        );
    }

    // CalendarEvent/parse of every file, uploaded to each.
    for (file, text) in corpus() {
        let their_blob = stalwart.upload(&text).await;
        let theirs = stalwart
            .call(json!([["CalendarEvent/parse", {"accountId": stalwart.account, "blobIds": [their_blob]}, "0"]]))
            .await;
        let uploaded = backend
            .upload(
                alice,
                "text/calendar".parse().unwrap(),
                UploadBody::from_chunks(vec![Bytes::from(text.clone())], 1_000_000),
            )
            .await
            .unwrap();
        let our_blob = uploaded.blob_id.as_str().to_owned();
        let ours = call(
            backend,
            alice,
            json!([["CalendarEvent/parse", {"accountId": id, "blobIds": [our_blob]}, "0"]]),
        )
        .await;
        let their_events = theirs[0][1]["parsed"][&their_blob]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let our_events = ours[0][1]["parsed"][&our_blob]
            .as_array()
            .cloned()
            .unwrap_or_default();
        compare(
            &format!("parse {file}"),
            &by_uid(&their_events),
            &by_uid(&our_events),
            &mut diffs,
        );
    }

    let unknown: Vec<&(String, String, String)> = diffs
        .iter()
        .filter(|(key, field, _)| {
            !KNOWN
                .iter()
                .any(|(uid, known)| key.contains(uid) && field == known)
        })
        .collect();
    for (key, field, detail) in &diffs {
        let kind = if unknown.iter().any(|(k, f, _)| k == key && f == field) {
            "UNEXPECTED"
        } else {
            "known"
        };
        eprintln!("DIFF ({kind}) {key} {field}: {detail}");
    }
    unknown.len()
}
