// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Principals and free-busy (libre/ocupado) against a fake CalDAV server
//! (hyper, in memory) shaped like the real ones: Stalwart (outbox, a
//! principal listing, only `displayname` searchable), a server that
//! searches by address (SabreDAV-like, with substring matches) and
//! Radicale (no scheduling, no listing).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::format_push_string,
    reason = "integration test helpers"
)]

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::Response;
use http_body_util::{BodyExt as _, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use nebula_crypto::{AccountId as KeyId, AccountKey, DataKey};
use nebula_dav::DavServer;
use nebula_dav::proto::CalendarAddress;
use nebula_groupware::model::freebusy::BusyStatus;
use nebula_groupware::principals::{Principal, PrincipalKind};
use nebula_groupware::{DavStatus, Groupware, GroupwareAccount, GroupwareConfig};
use nebula_store::{AccountId, NewAccount, Store, UnixTime};
use net_guard::{DnsError, DomainName, LookupFuture, Resolver};
use secrecy::SecretString;
use tokio::net::TcpListener;

type Body = Full<Bytes>;

/// How the fake server behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Outbox, listing, `displayname` searchable only.
    Stalwart,
    /// Outbox, `calendar-user-address-set` searchable, listing refused.
    Sabre,
    /// No outbox, no scheduling, listing refused.
    Radicale,
}

struct State {
    kind: Kind,
    requests: Vec<String>,
    /// More principals in the Stalwart listing (`user{i}`).
    extra: usize,
}

impl State {
    fn count(&self, needle: &str) -> usize {
        self.requests.iter().filter(|r| r.contains(needle)).count()
    }
}

fn reply(status: u16, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .header(
            "dav",
            "1, 2, 3, access-control, calendar-access, calendar-auto-schedule",
        )
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap()
}

fn multistatus(inner: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{inner}</d:multistatus>"#
    )
}

fn principal(path: &str, name: &str, address: &str, kind: &str) -> String {
    format!(
        "<d:response><d:href>{path}</d:href><d:propstat><d:prop>\
         <d:resourcetype><d:principal/><d:collection/></d:resourcetype>\
         <d:displayname>{name}</d:displayname>\
         <c:calendar-user-address-set><d:href>mailto:{address}</d:href></c:calendar-user-address-set>\
         <c:calendar-user-type>{kind}</c:calendar-user-type>\
         </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

const FREE_BUSY: &str = "BEGIN:VCALENDAR\nMETHOD:REPLY\nBEGIN:VFREEBUSY\n\
    FREEBUSY;FBTYPE=BUSY-TENTATIVE:20261028T100000Z/20261028T110000Z\n\
    END:VFREEBUSY\nEND:VCALENDAR\n";

fn handle(state: &mut State, method: &str, path: &str, body: &str) -> Response<Body> {
    state.requests.push(format!("{method} {path} {body}"));
    let kind = state.kind;
    match (method, path) {
        ("OPTIONS", _) => {
            let mut response = reply(200, "");
            if kind == Kind::Radicale {
                response.headers_mut().insert(
                    "dav",
                    http::HeaderValue::from_static("1, 2, 3, calendar-access"),
                );
            }
            response
        }
        ("PROPFIND", "/dav/") => reply(
            207,
            &multistatus(
                "<d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal>\
                 <d:href>/dav/principals/alice/</d:href></d:current-user-principal></d:prop>\
                 <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            ),
        ),
        ("PROPFIND", "/dav/principals/alice/") => {
            let outbox = if kind == Kind::Radicale {
                ""
            } else {
                "<c:schedule-outbox-URL><d:href>/dav/outbox/alice/</d:href></c:schedule-outbox-URL>"
            };
            reply(
                207,
                &multistatus(&format!(
                    "<d:response><d:href>/dav/principals/alice/</d:href><d:propstat><d:prop>\
                     <c:calendar-home-set><d:href>/dav/cal/alice/</d:href></c:calendar-home-set>\
                     <c:calendar-user-address-set><d:href>mailto:alice@example.com</d:href>\
                     <d:href>/dav/principals/alice/</d:href></c:calendar-user-address-set>\
                     <d:principal-collection-set><d:href>/dav/principals/</d:href></d:principal-collection-set>\
                     <d:displayname>Alice A.</d:displayname>{outbox}\
                     </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
                )),
            )
        }
        ("PROPFIND", "/dav/cal/alice/") => reply(
            207,
            &multistatus(
                "<d:response><d:href>/dav/cal/alice/</d:href><d:propstat><d:prop>\
                 <d:resourcetype><d:collection/></d:resourcetype></d:prop>\
                 <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>\
                 <d:response><d:href>/dav/cal/alice/work/</d:href><d:propstat><d:prop>\
                 <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>\
                 <d:displayname>Work</d:displayname><d:sync-token>t1</d:sync-token></d:prop>\
                 <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            ),
        ),
        ("REPORT", "/dav/cal/alice/work/") => {
            reply(207, &multistatus("<d:sync-token>t1</d:sync-token>"))
        }
        ("PROPFIND", "/dav/principals/") if kind == Kind::Stalwart => reply(
            207,
            &multistatus(&format!(
                "<d:response><d:href>/dav/principals/</d:href><d:propstat><d:prop>\
                 <d:resourcetype><d:collection/></d:resourcetype></d:prop>\
                 <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>{}{}{}{}",
                principal(
                    "/dav/principals/alice/",
                    "Alice A.",
                    "alice@example.com",
                    "INDIVIDUAL"
                ),
                principal(
                    "/dav/principals/bob/",
                    "Bob B.",
                    "bob@example.com",
                    "INDIVIDUAL"
                ),
                principal(
                    "/dav/principals/blue/",
                    "Blue room",
                    "blue@example.com",
                    "ROOM"
                ),
                (0..state.extra)
                    .map(|i| principal(
                        &format!("/dav/principals/user{i}/"),
                        &format!("User {i}"),
                        &format!("user{i}@example.com"),
                        "INDIVIDUAL"
                    ))
                    .collect::<String>(),
            )),
        ),
        ("PROPFIND", "/dav/principals/") => reply(403, ""),
        ("REPORT", "/dav/principals/") if body.contains("principal-search-property-set") => {
            match kind {
                Kind::Stalwart => reply(
                    200,
                    r#"<d:principal-search-property-set xmlns:d="DAV:"><d:principal-search-property><d:prop><d:displayname/></d:prop></d:principal-search-property></d:principal-search-property-set>"#,
                ),
                Kind::Sabre => reply(
                    200,
                    r#"<d:principal-search-property-set xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:principal-search-property><d:prop><c:calendar-user-address-set/></d:prop></d:principal-search-property></d:principal-search-property-set>"#,
                ),
                Kind::Radicale => reply(207, r#"<multistatus xmlns="DAV:"/>"#),
            }
        }
        ("REPORT", "/dav/principals/") if kind == Kind::Sabre => {
            // Substring matching: "bob@example.com" also finds bobby.
            let mut found = String::new();
            if body.contains("bob@example.com") {
                found.push_str(&principal(
                    "/dav/principals/users/bobby/",
                    "Bobby",
                    "bobby.bob@example.com",
                    "INDIVIDUAL",
                ));
                found.push_str(&principal(
                    "/dav/principals/users/bob/",
                    "Bob B.",
                    "bob@example.com",
                    "INDIVIDUAL",
                ));
            }
            reply(207, &multistatus(&found))
        }
        ("POST", "/dav/outbox/alice/") if kind != Kind::Radicale => {
            let mut answer = String::new();
            for line in body.lines().filter(|l| l.starts_with("ATTENDEE:")) {
                let address = line.trim_start_matches("ATTENDEE:").trim();
                if address.eq_ignore_ascii_case("mailto:bob@example.com") {
                    answer.push_str(&format!(
                        "<c:response><c:recipient><d:href>{address}</d:href></c:recipient>\
                         <c:request-status>2.0;Success</c:request-status>\
                         <c:calendar-data>{FREE_BUSY}</c:calendar-data></c:response>"
                    ));
                } else {
                    answer.push_str(&format!(
                        "<c:response><c:recipient><d:href>{address}</d:href></c:recipient>\
                         <c:request-status>3.7;Invalid calendar user</c:request-status></c:response>"
                    ));
                }
            }
            reply(
                200,
                &format!(
                    r#"<c:schedule-response xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{answer}</c:schedule-response>"#
                ),
            )
        }
        ("MKCALENDAR" | "MKCOL", _) => reply(201, ""),
        _ => reply(404, ""),
    }
}

struct Fake {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Fake {
    async fn start(kind: Kind) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            kind,
            requests: Vec::new(),
            extra: 0,
        }));
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let state = Arc::clone(&state);
                            async move {
                                let (parts, body) = request.into_parts();
                                let body = body.collect().await.unwrap().to_bytes();
                                let body = String::from_utf8_lossy(&body).into_owned();
                                // The bridge here only uses 127.0.0.1. A
                                // `localhost` request comes from outside the
                                // test (e.g. an IDE that probes every new
                                // listening port): not recorded.
                                if parts
                                    .headers
                                    .get("host")
                                    .and_then(|v| v.to_str().ok())
                                    .is_some_and(|h| h.split(':').next() == Some("localhost"))
                                {
                                    return Ok::<_, Infallible>(reply(404, ""));
                                }
                                let mut state = state.lock().unwrap();
                                Ok::<_, Infallible>(handle(
                                    &mut state,
                                    parts.method.as_str(),
                                    parts.uri.path(),
                                    &body,
                                ))
                            }
                        });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self { port, state }
    }

    fn count(&self, needle: &str) -> usize {
        self.state.lock().unwrap().count(needle)
    }
}

struct NoDns;

impl Resolver for NoDns {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async { Err(DnsError) })
    }
}

struct Setup {
    fake: Fake,
    account: Arc<GroupwareAccount>,
    _groupware: Groupware,
    _dir: tempfile::TempDir,
}

async fn setup(kind: Kind) -> Setup {
    let fake = Fake::start(kind).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id: AccountId = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "alice@example.com",
                wrapped_dek: b"k",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let server = DavServer::new(&format!("http://127.0.0.1:{}/dav/", fake.port))
        .unwrap()
        .with_private_network(true);
    let mut config = GroupwareConfig::new(server);
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    let groupware = Groupware::new(store, config, Arc::new(NoDns));
    let key = AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    );
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key)
        .unwrap();
    for _ in 0..500 {
        if account.status() == DavStatus::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(account.status(), DavStatus::Ready);
    Setup {
        fake,
        account,
        _groupware: groupware,
        _dir: dir,
    }
}

fn address(text: &str) -> CalendarAddress {
    CalendarAddress::parse(text).unwrap()
}

const FROM: i64 = 1_793_178_000; // 2026-10-28T09:00:00Z
const TO: i64 = 1_793_192_400; // 2026-10-28T13:00:00Z

/// Stalwart: the own principal from discovery (its path in the address
/// set is not an address), others from one listing kept for a while, a
/// room as `location`, free-busy through the outbox, and nothing asked
/// for an address the server did not show.
#[tokio::test]
async fn stalwart_like_listing_and_outbox() {
    let s = setup(Kind::Stalwart).await;
    let own = s.account.own_principal().unwrap();
    assert!(own.own);
    assert_eq!(own.name.as_deref(), Some("Alice A."));
    assert_eq!(own.addresses, [address("alice@example.com")]);
    assert!(s.account.supports_free_busy());

    // Own address: no request.
    let before = s.fake.count("PROPFIND /dav/principals/ ");
    let found = s
        .account
        .principal_for_address(&address("ALICE@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert!(found.own);
    assert_eq!(s.fake.count("PROPFIND /dav/principals/ "), before);

    // Bob: not searchable by address here, so one listing; the second
    // lookup and the full list use it.
    let bob = s
        .account
        .principal_for_address(&address("bob@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob.name.as_deref(), Some("Bob B."));
    assert!(!bob.own);
    assert_eq!(s.fake.count("principal-search-property-set"), 1);
    assert_eq!(s.fake.count("principal-property-search"), 0);
    assert_eq!(s.fake.count("PROPFIND /dav/principals/ "), 1);
    assert!(
        s.account
            .principal_for_address(&address("nobody@example.com"))
            .await
            .unwrap()
            .is_none()
    );
    let all = s.account.principals().await.unwrap();
    let names: Vec<Option<&str>> = all.iter().map(|p| p.name.as_deref()).collect();
    assert_eq!(names, [Some("Alice A."), Some("Bob B."), Some("Blue room")]);
    assert_eq!(all[2].kind, PrincipalKind::Location);
    assert_eq!(s.fake.count("PROPFIND /dav/principals/ "), 1);
    assert_eq!(s.fake.count("principal-search-property-set"), 1);
    assert_eq!(
        s.account.known_principal(bob.number()).unwrap().href,
        bob.href
    );

    // Free-busy: the outbox, with alice as organizer and only bob asked.
    let periods = s
        .account
        .free_busy(
            &address("alice@example.com"),
            &address("bob@example.com"),
            FROM,
            TO,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].status, BusyStatus::Tentative);
    assert_eq!(periods[0].end - periods[0].start, 3600);
    let posts: Vec<String> = s
        .fake
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.starts_with("POST "))
        .cloned()
        .collect();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].contains("ORGANIZER:mailto:alice@example.com\r\n"));
    assert!(posts[0].contains("ATTENDEE:mailto:bob@example.com\r\n"));
    assert_eq!(posts[0].matches("ATTENDEE:").count(), 1);
    // Someone the server answers 3.7 for: no data.
    assert!(
        s.account
            .free_busy(
                &address("alice@example.com"),
                &address("blue@example.com"),
                FROM,
                TO
            )
            .await
            .unwrap()
            .is_none()
    );
}

/// A server that searches by address: the search is used (not the
/// listing), and only the exact address counts (the server matches
/// substrings).
#[tokio::test]
async fn address_search_with_substring_matches() {
    let s = setup(Kind::Sabre).await;
    let bob = s
        .account
        .principal_for_address(&address("bob@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob.href.path(), "/dav/principals/users/bob/");
    assert_eq!(s.fake.count("principal-property-search"), 1);
    assert_eq!(s.fake.count("PROPFIND /dav/principals/ "), 0);
    // Found ones are remembered.
    s.account
        .principal_for_address(&address("bob@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.fake.count("principal-property-search"), 1);
    assert!(s.account.known_principal(bob.number()).is_some());
    assert!(
        s.account
            .principal_for_address(&address("carol@example.com"))
            .await
            .unwrap()
            .is_none()
    );
    // The listing is refused (403): just the own principal.
    let all = s.account.principals().await.unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].own);
}

/// Radicale: no scheduling and no outbox, so no free-busy (nothing is
/// sent); its address set is a path, so the own principal has no
/// address; no listing and no search.
#[tokio::test]
async fn radicale_like_without_scheduling() {
    let s = setup(Kind::Radicale).await;
    assert!(!s.account.supports_free_busy());
    assert!(
        s.account
            .free_busy(
                &address("alice@example.com"),
                &address("bob@example.com"),
                FROM,
                TO
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(s.fake.count("POST "), 0);
    assert!(
        s.account
            .principal_for_address(&address("bob@example.com"))
            .await
            .unwrap()
            .is_none()
    );
    let all = s.account.principals().await.unwrap();
    assert_eq!(all.len(), 1);
}

/// A big listing: `known_principal` finds one at the end through the
/// index, and the lists keep their order and have no duplicates.
#[tokio::test]
async fn known_principal_in_a_big_listing() {
    let s = setup(Kind::Stalwart).await;
    s.fake.state.lock().unwrap().extra = 2000;
    let all = s.account.principals().await.unwrap();
    assert_eq!(all.len(), 2003);
    assert!(all[0].own);
    let last = all.last().unwrap();
    assert_eq!(last.name.as_deref(), Some("User 1999"));
    let found = s.account.known_principal(last.number()).unwrap();
    assert_eq!(found.href, last.href);
    let own = s.account.own_principal().unwrap();
    assert!(s.account.known_principal(own.number()).unwrap().own);
    assert!(s.account.known_principal(12345).is_none());
    let known = s.account.known_principals();
    assert_eq!(
        known.iter().map(Principal::number).collect::<Vec<_>>(),
        all.iter().map(Principal::number).collect::<Vec<_>>()
    );
}
