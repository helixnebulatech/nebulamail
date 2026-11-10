// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendars and address books on two configured servers (`url` and
//! `contacts_url`, as Fastmail's `caldav.fastmail.com` and
//! `carddav.fastmail.com`), against three fake servers (hyper, in
//! memory):
//!
//! - A, the calendar server (`url`);
//! - B, the contacts server (`contacts_url`);
//! - C, a third origin both servers name (homes, principals), in the
//!   admin's `allowed_hosts`.
//!
//! The login goes to A and B, each with its own kind of collection, and
//! never to C; an address book A names (or a calendar B names) is not
//! used.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
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
use nebula_dav::proto::ResourceName;
use nebula_dav::{AllowedOrigin, DavServer, Origin};
use nebula_groupware::{DavStatus, Groupware, GroupwareAccount, GroupwareConfig};
use nebula_store::{AccountId, Collection, CollectionKind, NewAccount, Store, UnixTime};
use net_guard::{DnsError, DomainName, LookupFuture, NetworkAccess, Resolver};
use secrecy::SecretString;
use tokio::net::TcpListener;

// ------------------------------------------------------------ fake servers

const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
const CS: &str = "http://calendarserver.org/ns/";

/// The Basic login of `alice` / `password`.
const BASIC: &str = "Basic YWxpY2U6cGFzc3dvcmQ=";

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;
type Handler = Arc<dyn Fn(&str, &str, &str) -> Response<Body> + Send + Sync>;

/// One request seen by a fake server.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    listener: Option<TcpListener>,
}

impl Fake {
    async fn bind() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        Self {
            port: listener.local_addr().unwrap().port(),
            seen: Arc::new(Mutex::new(Vec::new())),
            listener: Some(listener),
        }
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn serve(&mut self, handler: Handler) {
        let listener = self.listener.take().unwrap();
        let seen = Arc::clone(&self.seen);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (seen, handler) = (Arc::clone(&seen), Arc::clone(&handler));
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let (seen, handler) = (Arc::clone(&seen), Arc::clone(&handler));
                            async move {
                                let (parts, body) = request.into_parts();
                                let body = body.collect().await.unwrap().to_bytes();
                                let body = String::from_utf8_lossy(&body).into_owned();
                                // The clients here only use 127.0.0.1. A
                                // `localhost` request comes from outside the
                                // test (e.g. an IDE that probes every new
                                // listening port), so it is not recorded.
                                let outside = parts
                                    .headers
                                    .get("host")
                                    .and_then(|v| v.to_str().ok())
                                    .is_some_and(|h| h.split(':').next() == Some("localhost"));
                                if !outside {
                                    seen.lock().unwrap().push(Seen {
                                        method: parts.method.as_str().to_owned(),
                                        path: parts.uri.path().to_owned(),
                                        authorization: parts
                                            .headers
                                            .get("authorization")
                                            .map(|v| v.to_str().unwrap().to_owned()),
                                    });
                                }
                                Ok::<_, Infallible>(handler(
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
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Whether anything under `path` was listed, read or written
    /// (discovery's `OPTIONS` on a named home only reads its `DAV:`
    /// header).
    fn used(&self, path: &str) -> bool {
        self.seen()
            .iter()
            .any(|s| s.method != "OPTIONS" && s.path.starts_with(path))
    }
}

/// No DNS: every fake server is an IP address.
struct NoDns;

impl Resolver for NoDns {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async { Err(DnsError) })
    }
}

fn multistatus(inner: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:c="{CALDAV}" xmlns:r="{CARDDAV}" xmlns:cs="{CS}">{inner}</d:multistatus>"#
    )
}

fn ok(href: &str, props: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

fn reply(status: u16, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .header("dav", "1, 3, calendar-access, addressbook")
        .body(Full::new(Bytes::from(body)).boxed())
        .unwrap()
}

fn created(etag: &str) -> Response<Body> {
    Response::builder()
        .status(201)
        .header("etag", etag)
        .body(Full::new(Bytes::new()).boxed())
        .unwrap()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;")
}

const CALENDAR: &str = "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>";
const BOOK: &str = "<d:resourcetype><d:collection/><r:addressbook/></d:resourcetype>";

fn collection(href: &str, kind: &str, name: &str) -> String {
    ok(
        href,
        &format!("{kind}<d:displayname>{name}</d:displayname><cs:getctag>ctag-1</cs:getctag>"),
    )
}

/// A collection with one member: its Depth 1 listing and its multiget.
fn one_member(
    method: &str,
    path: &str,
    body: &str,
    coll: &str,
    name: &str,
    element: &str,
    data: &str,
) -> Option<Response<Body>> {
    let member = format!("{coll}{name}");
    match (method, path) {
        ("PROPFIND", p) if p == coll => Some(reply(
            207,
            multistatus(&format!(
                "{}{}",
                ok(coll, "<d:resourcetype><d:collection/></d:resourcetype>"),
                ok(
                    &member,
                    &format!("<d:resourcetype/><d:getetag>\"{name}-1\"</d:getetag>"),
                ),
            )),
        )),
        ("REPORT", p) if p == coll && body.contains(&member) => Some(reply(
            207,
            multistatus(&ok(
                &member,
                &format!(
                    "<d:getetag>\"{name}-1\"</d:getetag><{element}>{}</{element}>",
                    escape(data)
                ),
            )),
        )),
        _ => None,
    }
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//test//EN\r\nBEGIN:VEVENT\r\nUID:e1\r\n\
DTSTAMP:20261027T120000Z\r\nDTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\n\
SUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const CARD: &str =
    "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nFN:Bob Builder\r\nN:Builder;Bob;;;\r\nEND:VCARD\r\n";

/// A, the calendar server at `/dav/`. Its principal also names an address
/// book home of its own and homes on B and C; its calendar home lists an
/// address book too. None of those may be used with `contacts_url` set.
fn calendar_server(b: String, c: String) -> Handler {
    Arc::new(move |method, path, body| {
        if let Some(found) = one_member(
            method,
            path,
            body,
            "/dav/cal/alice/work/",
            "e1.ics",
            "c:calendar-data",
            EVENT,
        ) {
            return found;
        }
        match (method, path) {
            ("OPTIONS", _) => reply(200, String::new()),
            ("PROPFIND", "/dav/") => reply(
                207,
                multistatus(&ok(
                    "/dav/",
                    "<d:current-user-principal><d:href>/dav/principals/alice/</d:href></d:current-user-principal>",
                )),
            ),
            ("PROPFIND", "/dav/principals/alice/") => reply(
                207,
                multistatus(&ok(
                    "/dav/principals/alice/",
                    &format!(
                        "<c:calendar-home-set><d:href>/dav/cal/alice/</d:href><d:href>{c}/cal/alice/</d:href><d:href>{b}/dav.php/calendars/alice/</d:href></c:calendar-home-set>\
                         <r:addressbook-home-set><d:href>/dav/card/alice/</d:href><d:href>{c}/card/alice/</d:href><d:href>{b}/dav.php/addressbooks/alice/</d:href></r:addressbook-home-set>\
                         <c:calendar-user-address-set><d:href>mailto:alice@example.com</d:href></c:calendar-user-address-set>\
                         <d:principal-collection-set><d:href>/dav/principals/</d:href></d:principal-collection-set>"
                    ),
                )),
            ),
            ("PROPFIND", "/dav/cal/alice/") => reply(
                207,
                multistatus(&format!(
                    "{}{}{}",
                    ok(
                        "/dav/cal/alice/",
                        "<d:resourcetype><d:collection/></d:resourcetype>"
                    ),
                    collection("/dav/cal/alice/work/", CALENDAR, "Work"),
                    collection("/dav/cal/alice/sneaky/", BOOK, "Sneaky book"),
                )),
            ),
            ("PROPFIND", "/dav/card/alice/") => reply(
                207,
                multistatus(&format!(
                    "{}{}",
                    ok(
                        "/dav/card/alice/",
                        "<d:resourcetype><d:collection/></d:resourcetype>"
                    ),
                    collection("/dav/card/alice/old/", BOOK, "Old book"),
                )),
            ),
            ("PUT", p) if p.starts_with("/dav/cal/alice/work/") => created("\"new-event\""),
            _ => reply(404, String::new()),
        }
    })
}

/// B, the contacts server at `/dav.php/` (as Baïkal). Its principal names
/// a calendar home of its own and an address book home on A; its address
/// book home lists a calendar too. Only its address books may be used.
fn contacts_server(a: String) -> Handler {
    Arc::new(move |method, path, body| {
        if let Some(found) = one_member(
            method,
            path,
            body,
            "/dav.php/addressbooks/alice/contacts/",
            "c1.vcf",
            "r:address-data",
            CARD,
        ) {
            return found;
        }
        match (method, path) {
            ("OPTIONS", _) => reply(200, String::new()),
            ("PROPFIND", "/dav.php/") => reply(
                207,
                multistatus(&ok(
                    "/dav.php/",
                    "<d:current-user-principal><d:href>/dav.php/principals/alice/</d:href></d:current-user-principal>",
                )),
            ),
            ("PROPFIND", "/dav.php/principals/alice/") => reply(
                207,
                multistatus(&ok(
                    "/dav.php/principals/alice/",
                    &format!(
                        "<c:calendar-home-set><d:href>/dav.php/calendars/alice/</d:href></c:calendar-home-set>\
                         <r:addressbook-home-set><d:href>/dav.php/addressbooks/alice/</d:href><d:href>{a}/dav/card/alice/</d:href></r:addressbook-home-set>"
                    ),
                )),
            ),
            ("PROPFIND", "/dav.php/addressbooks/alice/") => reply(
                207,
                multistatus(&format!(
                    "{}{}{}",
                    ok(
                        "/dav.php/addressbooks/alice/",
                        "<d:resourcetype><d:collection/></d:resourcetype>"
                    ),
                    // Its owner is alice's principal on B (not A's).
                    ok(
                        "/dav.php/addressbooks/alice/contacts/",
                        &format!(
                            "{BOOK}<d:displayname>Contacts</d:displayname><cs:getctag>ctag-1</cs:getctag>\
                             <d:owner><d:href>/dav.php/principals/alice/</d:href></d:owner>"
                        ),
                    ),
                    collection("/dav.php/addressbooks/alice/odd/", CALENDAR, "Odd calendar"),
                )),
            ),
            ("PUT", p) if p.starts_with("/dav.php/addressbooks/alice/contacts/") => {
                created("\"new-card\"")
            }
            _ => reply(404, String::new()),
        }
    })
}

/// C, a third origin the admin allowed: answers anything, so that a
/// request that reached it would show.
fn third_server() -> Handler {
    Arc::new(|method, path, _body| match method {
        "OPTIONS" => reply(200, String::new()),
        _ => reply(
            207,
            multistatus(&format!(
                "{}{}",
                ok(path, "<d:resourcetype><d:collection/></d:resourcetype>"),
                collection(&format!("{path}x/"), BOOK, "Elsewhere"),
            )),
        ),
    })
}

// ------------------------------------------------------------ helpers

struct Setup {
    a: Fake,
    b: Fake,
    c: Fake,
    store: Store,
    account: Arc<GroupwareAccount>,
    id: AccountId,
    _groupware: Groupware,
    _dir: tempfile::TempDir,
}

fn key() -> AccountKey {
    AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    )
}

fn server(url: &str, allowed: &str) -> DavServer {
    DavServer::new(url)
        .unwrap()
        .with_private_network(true)
        .with_allowed_origin(AllowedOrigin {
            origin: Origin::parse(allowed).unwrap(),
            network: NetworkAccess::PrivateAllowed,
        })
}

async fn setup() -> Setup {
    let (mut a, mut b, mut c) = (Fake::bind().await, Fake::bind().await, Fake::bind().await);
    a.serve(calendar_server(b.origin(), c.origin()));
    b.serve(contacts_server(a.origin()));
    c.serve(third_server());

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
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
    let mut config = GroupwareConfig::new(server(&format!("{}/dav/", a.origin()), &c.origin()))
        .with_contacts_server(server(&format!("{}/dav.php/", b.origin()), &c.origin()));
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    config.first_retry = Duration::from_millis(50);
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key())
        .unwrap();
    Setup {
        a,
        b,
        c,
        store,
        account,
        id,
        _groupware: groupware,
        _dir: dir,
    }
}

async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn live(setup: &Setup) -> Vec<Collection> {
    setup
        .store
        .dav_collections(setup.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|c| c.orphaned_at.is_none())
        .collect()
}

async fn synced(setup: &Setup) {
    until("the first round", || {
        setup.account.status() == DavStatus::Ready
    })
    .await;
    // The default collections are made after the first round: wait until
    // nothing is created any more (none should be: both kinds exist).
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ------------------------------------------------------------ tests

#[tokio::test]
async fn each_kind_from_its_own_server_and_the_login_only_there() {
    let s = setup().await;
    synced(&s).await;

    // One calendar from A, one address book from B; nothing else.
    let collections = live(&s).await;
    let mut found: Vec<(CollectionKind, String)> = collections
        .iter()
        .map(|c| (c.props.kind, c.props.href.as_str().to_owned()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            (
                CollectionKind::Calendar,
                format!("{}/dav/cal/alice/work/", s.a.origin())
            ),
            (
                CollectionKind::AddressBook,
                format!("{}/dav.php/addressbooks/alice/contacts/", s.b.origin())
            ),
        ]
    );
    let available = s.account.availability();
    assert!(available.calendars && available.contacts);
    // Whose the address book is, is decided against alice's principal on
    // B: it is hers, not someone else's shared with her.
    assert!(
        collections.iter().all(|c| c.props.sharing.owner.is_none()),
        "{:?}",
        collections
            .iter()
            .map(|c| &c.props.sharing.owner)
            .collect::<Vec<_>>()
    );
    for coll in &collections {
        let resources = s.store.dav_resources(s.id, coll.id).await.unwrap();
        assert_eq!(resources.len(), 1, "{:?}", coll.props.kind);
    }

    // The login went with every request to A and B (each its own
    // configured server)…
    for (name, fake) in [("A", &s.a), ("B", &s.b)] {
        let seen = fake.seen();
        assert!(!seen.is_empty(), "{name} was asked");
        assert!(
            seen.iter()
                .all(|r| r.authorization.as_deref() == Some(BASIC)),
            "{name}: {seen:?}"
        );
    }
    // …and never to C. Discovery may ask C's homes for their `DAV:`
    // header (an allowed host, without the login), but nothing on C is
    // listed or used.
    assert!(
        s.c.seen()
            .iter()
            .all(|r| r.authorization.is_none() && r.method == "OPTIONS"),
        "{:?}",
        s.c.seen()
    );

    // A's address books (its own home, the one in its calendar home) and
    // B's calendars were never used.
    assert!(!s.a.used("/dav/card/"));
    assert!(!s.a.used("/dav/cal/alice/sneaky/"));
    assert!(!s.b.used("/dav.php/calendars/"));
    assert!(!s.b.used("/dav.php/addressbooks/alice/odd/"));
    // B never got anything of A's paths, nor A of B's.
    assert!(!s.b.used("/dav/"));
    assert!(!s.a.used("/dav.php/"));

    // Writes go to the server of their kind, with the login.
    let calendar = collections
        .iter()
        .find(|c| c.props.kind == CollectionKind::Calendar)
        .unwrap();
    let book = collections
        .iter()
        .find(|c| c.props.kind == CollectionKind::AddressBook)
        .unwrap();
    let card = s
        .account
        .create_resource(
            book.id,
            &ResourceName::try_from("new.vcf").unwrap(),
            CARD.replace("UID:c1", "UID:c2"),
        )
        .await
        .unwrap();
    assert!(card.href.as_str().starts_with(&s.b.origin()));
    let event = s
        .account
        .create_resource(
            calendar.id,
            &ResourceName::try_from("new.ics").unwrap(),
            EVENT.replace("UID:e1", "UID:e2"),
        )
        .await
        .unwrap();
    assert!(event.href.as_str().starts_with(&s.a.origin()));
    let puts = |fake: &Fake| -> Vec<Seen> {
        fake.seen()
            .into_iter()
            .filter(|r| r.method == "PUT")
            .collect()
    };
    let (a_puts, b_puts) = (puts(&s.a), puts(&s.b));
    assert_eq!(a_puts.len(), 1);
    assert_eq!(a_puts[0].path, "/dav/cal/alice/work/new.ics");
    assert_eq!(b_puts.len(), 1);
    assert_eq!(
        b_puts[0].path,
        "/dav.php/addressbooks/alice/contacts/new.vcf"
    );
    assert!(
        a_puts
            .iter()
            .chain(&b_puts)
            .all(|r| r.authorization.as_deref() == Some(BASIC))
    );
    assert!(s.c.seen().iter().all(|r| r.method == "OPTIONS"));
}

#[tokio::test]
async fn a_refused_login_on_the_contacts_server_stops_both() {
    let (mut a, mut b) = (Fake::bind().await, Fake::bind().await);
    a.serve(calendar_server(b.origin(), "http://127.0.0.1:9".to_owned()));
    b.serve(Arc::new(|_, _, _| reply(401, String::new())));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
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
    let config = GroupwareConfig::new(
        DavServer::new(&format!("{}/dav/", a.origin()))
            .unwrap()
            .with_private_network(true),
    )
    .with_contacts_server(
        DavServer::new(&format!("{}/dav.php/", b.origin()))
            .unwrap()
            .with_private_network(true),
    );
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key())
        .unwrap();
    until("the refused login", || {
        account.status() == DavStatus::Unauthorized
    })
    .await;
    let available = account.availability();
    assert!(!available.calendars && !available.contacts);
    assert!(store.dav_collections(id).await.unwrap().is_empty());
}

#[tokio::test]
async fn contacts_off_never_asks_the_contacts_server() {
    let (mut a, mut b) = (Fake::bind().await, Fake::bind().await);
    a.serve(calendar_server(b.origin(), "http://127.0.0.1:9".to_owned()));
    b.serve(contacts_server(a.origin()));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
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
    let mut config = GroupwareConfig::new(
        DavServer::new(&format!("{}/dav/", a.origin()))
            .unwrap()
            .with_private_network(true),
    )
    .with_contacts_server(
        DavServer::new(&format!("{}/dav.php/", b.origin()))
            .unwrap()
            .with_private_network(true),
    );
    config.contacts = false;
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key())
        .unwrap();
    until("the first round", || account.status() == DavStatus::Ready).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(b.seen().is_empty());
    let kinds: Vec<CollectionKind> = store
        .dav_collections(id)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.props.kind)
        .collect();
    assert_eq!(kinds, vec![CollectionKind::Calendar]);
}
