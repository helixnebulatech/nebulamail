// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! iCloud's per-account hosts, with
//! fake servers. The real names (`caldav.icloud.com` →
//! `p42-caldav.icloud.com`, https on 443) can't be served locally, so the
//! servers use a test rule of the same shape (`p<digits>-caldav.icloud.test`
//! on a loopback port), only available with the `test-account-hosts`
//! feature of `nebula-dav` that this crate's dev-dependencies enable:
//!
//! - E, the configured calendar server (`url`), and F, the configured
//!   contacts server (`contacts_url`);
//! - P, "Apple's partitions": one port for every `*.icloud.test` name,
//!   telling hosts apart by the `Host` header. It answers anything, so
//!   that a request that reached a host it shouldn't would show.
//!
//! The fake DNS records every name it is asked: a refused host is refused
//! before it is even resolved.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "integration test helpers"
)]

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};
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
use nebula_dav::{AccountHosts, AllowedOrigin, DavServer, Origin, Scheme};
use nebula_groupware::{DavStatus, Groupware, GroupwareAccount, GroupwareConfig};
use nebula_store::{AccountId, CollectionKind, NewAccount, Store, UnixTime};
use net_guard::{DnsError, DomainName, LookupFuture, NetworkAccess, Port, Resolver};
use secrecy::SecretString;
use tokio::net::TcpListener;

// ------------------------------------------------------------ fake servers

const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
const CS: &str = "http://calendarserver.org/ns/";

/// The Basic login of `alice@icloud.com` / `app-password`.
const BASIC: &str = "Basic YWxpY2VAaWNsb3VkLmNvbTphcHAtcGFzc3dvcmQ=";

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;
/// `(host, method, path, body)` → answer.
type Handler = Arc<dyn Fn(&str, &str, &str, &str) -> Response<Body> + Send + Sync>;

#[derive(Debug, Clone)]
struct Seen {
    host: String,
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

    /// `http://<name>:<port>`: a name of P.
    fn named(&self, name: &str) -> String {
        format!("http://{name}:{}", self.port)
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
                                let host = parts
                                    .headers
                                    .get("host")
                                    .map(|v| v.to_str().unwrap().to_owned())
                                    .unwrap_or_default();
                                // Without the port: P has only one.
                                let host = host.split(':').next().unwrap_or_default().to_owned();
                                // The clients here only use 127.0.0.1 or the
                                // .icloud.test names. A `localhost` request
                                // comes from outside the test (e.g. an IDE
                                // that probes every new listening port), so it
                                // is not recorded.
                                if host != "localhost" {
                                    seen.lock().unwrap().push(Seen {
                                        host: host.clone(),
                                        method: parts.method.as_str().to_owned(),
                                        path: parts.uri.path().to_owned(),
                                        authorization: parts
                                            .headers
                                            .get("authorization")
                                            .map(|v| v.to_str().unwrap().to_owned()),
                                    });
                                }
                                Ok::<_, Infallible>(handler(
                                    &host,
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

    fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.seen().into_iter().map(|s| s.host).collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }
}

/// Every `*.icloud.test` name is 127.0.0.1 (where P listens); every name
/// asked is recorded.
#[derive(Default)]
struct FakeDns {
    asked: Mutex<Vec<String>>,
}

impl Resolver for FakeDns {
    fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(name.as_str().to_owned());
            if name.as_str().ends_with(".icloud.test") {
                Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
            } else {
                Err(DnsError)
            }
        })
    }
}

impl FakeDns {
    fn asked(&self) -> Vec<String> {
        let mut asked = self.asked.lock().unwrap().clone();
        asked.sort();
        asked.dedup();
        asked
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

fn redirect(location: &str) -> Response<Body> {
    Response::builder()
        .status(301)
        .header("location", location)
        .body(Full::new(Bytes::new()).boxed())
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
const PLAIN: &str = "<d:resourcetype><d:collection/></d:resourcetype>";

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//test//EN\r\nBEGIN:VEVENT\r\nUID:e1\r\n\
DTSTAMP:20261027T120000Z\r\nDTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\n\
SUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const CARD: &str =
    "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nFN:Bob Builder\r\nN:Builder;Bob;;;\r\nEND:VCARD\r\n";

/// A home with one collection of one member: listing, member listing and
/// multiget.
fn home_with_one(
    method: &str,
    path: &str,
    body: &str,
    home: &str,
    kind: &str,
    element: &str,
    data: &str,
) -> Option<Response<Body>> {
    let coll = format!("{home}main/");
    let member = format!("{coll}one");
    match (method, path) {
        ("PROPFIND", p) if p == home => Some(reply(
            207,
            multistatus(&format!(
                "{}{}",
                ok(home, PLAIN),
                ok(
                    &coll,
                    &format!(
                        "{kind}<d:displayname>Main</d:displayname><cs:getctag>ctag-1</cs:getctag>"
                    ),
                ),
            )),
        )),
        ("PROPFIND", p) if p == coll => Some(reply(
            207,
            multistatus(&format!(
                "{}{}",
                ok(&coll, PLAIN),
                ok(&member, "<d:resourcetype/><d:getetag>\"one-1\"</d:getetag>"),
            )),
        )),
        ("REPORT", p) if p == coll && body.contains(&member) => Some(reply(
            207,
            multistatus(&ok(
                &member,
                &format!(
                    "<d:getetag>\"one-1\"</d:getetag><{element}>{}</{element}>",
                    escape(data)
                ),
            )),
        )),
        ("PUT", p) if p.starts_with(&coll) => Some(created("\"new\"")),
        _ => None,
    }
}

/// P: alice's calendars on `p42-caldav`, her address books on
/// `p42-contacts`, and on `p42-caldav` also a principal (for the redirect
/// case). Any other name answers anything.
fn partitions() -> Handler {
    Arc::new(|host, method, path, body| {
        if method == "OPTIONS" {
            return reply(200, String::new());
        }
        let found = match host {
            "p42-caldav.icloud.test" => match (method, path) {
                ("PROPFIND", "/") => Some(reply(
                    207,
                    multistatus(&ok(
                        "/",
                        "<d:current-user-principal><d:href>/1234/principal/</d:href></d:current-user-principal>",
                    )),
                )),
                ("PROPFIND", "/1234/principal/") => Some(reply(
                    207,
                    multistatus(&ok(
                        "/1234/principal/",
                        "<c:calendar-home-set><d:href>/1234/calendars/</d:href></c:calendar-home-set>",
                    )),
                )),
                _ => home_with_one(
                    method,
                    path,
                    body,
                    "/1234/calendars/",
                    CALENDAR,
                    "c:calendar-data",
                    EVENT,
                ),
            },
            "p42-contacts.icloud.test" => home_with_one(
                method,
                path,
                body,
                "/1234/carddavhome/",
                BOOK,
                "r:address-data",
                CARD,
            ),
            _ => None,
        };
        found.unwrap_or_else(|| {
            reply(
                207,
                multistatus(&format!(
                    "{}{}",
                    ok(path, PLAIN),
                    ok(
                        &format!("{path}x/"),
                        &format!("{CALENDAR}<d:displayname>Elsewhere</d:displayname>")
                    ),
                )),
            )
        })
    })
}

/// E or F, the configured server: its principal names `homes`.
fn entry(homes: String) -> Handler {
    Arc::new(move |_host, method, path, _body| match (method, path) {
        ("OPTIONS", _) => reply(200, String::new()),
        ("PROPFIND", "/.well-known/caldav" | "/.well-known/carddav" | "/") => reply(
            207,
            multistatus(&ok(
                path,
                "<d:current-user-principal><d:href>/1234/principal/</d:href></d:current-user-principal>",
            )),
        ),
        ("PROPFIND", "/1234/principal/") => {
            reply(207, multistatus(&ok("/1234/principal/", &homes)))
        }
        _ => reply(404, String::new()),
    })
}

/// E, redirecting every discovery place to `location`.
fn redirecting(location: String) -> Handler {
    Arc::new(move |_host, method, path, _body| match (method, path) {
        ("PROPFIND", "/.well-known/caldav" | "/.well-known/carddav" | "/") => redirect(&location),
        _ => reply(404, String::new()),
    })
}

/// E, naming a principal at `base` (another host).
fn principal_at(base: String) -> Handler {
    Arc::new(move |_host, method, path, _body| match method {
        "PROPFIND" => reply(
            207,
            multistatus(&ok(
                path,
                &format!(
                    "<d:current-user-principal><d:href>{base}1234/principal/</d:href></d:current-user-principal>"
                ),
            )),
        ),
        _ => reply(404, String::new()),
    })
}

// ------------------------------------------------------------ helpers

fn key() -> AccountKey {
    AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    )
}

/// The configured server at `url`, with the test rule of `service` on P.
fn server(url: &str, service: &'static str, p: &Fake) -> DavServer {
    DavServer::new(url)
        .unwrap()
        .with_private_network(true)
        .with_test_account_hosts(AccountHosts::for_tests(
            service,
            "icloud.test",
            Scheme::Http,
            Port::try_from(p.port).unwrap(),
            NetworkAccess::PrivateAllowed,
        ))
}

struct Run {
    store: Store,
    id: AccountId,
    account: Arc<GroupwareAccount>,
    dns: Arc<FakeDns>,
    _groupware: Groupware,
    _dir: tempfile::TempDir,
}

async fn open(config: GroupwareConfig) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
        .get_or_create_account(
            NewAccount {
                login: "alice@icloud.com",
                email: "alice@icloud.com",
                wrapped_dek: b"k",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let dns = Arc::new(FakeDns::default());
    let resolver: Arc<dyn Resolver> = Arc::<FakeDns>::clone(&dns);
    let groupware = Groupware::new(store.clone(), config, resolver);
    let account = groupware
        .open(
            id,
            "alice@icloud.com",
            SecretString::from("app-password"),
            key(),
        )
        .unwrap();
    Run {
        store,
        id,
        account,
        dns,
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

fn quick(mut config: GroupwareConfig) -> GroupwareConfig {
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    config.first_retry = Duration::from_hours(1);
    config
}

// ------------------------------------------------------------ tests

/// The iCloud setup of docs/bridge.md: `url` (calendars) and
/// `contacts_url` (contacts), each principal naming homes on Apple hosts.
/// Each client uses its own kind of host, with the login; every other
/// host is refused before it is even resolved, except an allowed host,
/// which never gets the login.
#[tokio::test]
async fn each_client_uses_its_account_host_with_the_login() {
    let mut p = Fake::bind().await;
    p.serve(partitions());
    let (mut e, mut f) = (Fake::bind().await, Fake::bind().await);
    let calendar_homes = format!(
        "<c:calendar-home-set>\
           <d:href>{}/1234/calendars/</d:href>\
           <d:href>{}/1234/calendars/</d:href>\
           <d:href>{}/1234/carddavhome/</d:href>\
           <d:href>{}/1234/calendars/</d:href>\
           <d:href>{}/x/</d:href>\
           <d:href>{}/1234/calendars/</d:href>\
         </c:calendar-home-set>\
         <c:schedule-outbox-URL><d:href>{}/1234/calendars/outbox/</d:href></c:schedule-outbox-URL>",
        p.named("p42-caldav.icloud.test"),
        // A second partition after the first: not the account's.
        p.named("p43-caldav.icloud.test"),
        // The contacts pattern on the calendar client.
        p.named("p42-contacts.icloud.test"),
        // In the domain, outside the pattern, but an allowed host.
        p.named("blog.icloud.test"),
        // Lookalikes.
        p.named("p42-caldav.icloud.test.evil.test"),
        p.named("xp42-caldav.icloud.test"),
        p.named("p42-caldav.icloud.test"),
    );
    let book_homes = format!(
        "<r:addressbook-home-set>\
           <d:href>{}/1234/carddavhome/</d:href>\
           <d:href>{}/1234/calendars/</d:href>\
         </r:addressbook-home-set>",
        p.named("p42-contacts.icloud.test"),
        // The calendar pattern on the contacts client.
        p.named("p42-caldav.icloud.test"),
    );
    e.serve(entry(calendar_homes));
    f.serve(entry(book_homes));
    let blog = AllowedOrigin {
        origin: Origin::parse(&p.named("blog.icloud.test")).unwrap(),
        network: NetworkAccess::PrivateAllowed,
    };
    let config = GroupwareConfig::new(
        server(&format!("{}/", e.origin()), "caldav", &p).with_allowed_origin(blog.clone()),
    )
    .with_contacts_server(
        server(&format!("{}/", f.origin()), "contacts", &p).with_allowed_origin(blog),
    );
    let run = open(quick(config)).await;
    until("the first round", || {
        run.account.status() == DavStatus::Ready
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // One calendar on p42-caldav, one address book on p42-contacts.
    let collections = run.store.dav_collections(run.id).await.unwrap();
    let mut found: Vec<(CollectionKind, String)> = collections
        .iter()
        .filter(|c| c.orphaned_at.is_none())
        .map(|c| (c.props.kind, c.props.href.as_str().to_owned()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            (
                CollectionKind::Calendar,
                format!("{}/1234/calendars/main/", p.named("p42-caldav.icloud.test"))
            ),
            (
                CollectionKind::AddressBook,
                format!(
                    "{}/1234/carddavhome/main/",
                    p.named("p42-contacts.icloud.test")
                )
            ),
        ]
    );
    for coll in &collections {
        assert_eq!(
            run.store
                .dav_resources(run.id, coll.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    // Writes go to the account hosts, with the login.
    let id_of = |kind| {
        collections
            .iter()
            .find(|c| c.props.kind == kind)
            .unwrap()
            .id
    };
    run.account
        .create_resource(
            id_of(CollectionKind::Calendar),
            &ResourceName::try_from("new.ics").unwrap(),
            EVENT.replace("UID:e1", "UID:e2"),
        )
        .await
        .unwrap();
    run.account
        .create_resource(
            id_of(CollectionKind::AddressBook),
            &ResourceName::try_from("new.vcf").unwrap(),
            CARD.replace("UID:c1", "UID:c2"),
        )
        .await
        .unwrap();
    let puts: Vec<(String, String)> = p
        .seen()
        .into_iter()
        .filter(|s| s.method == "PUT")
        .map(|s| (s.host, s.path))
        .collect();
    assert_eq!(
        puts,
        vec![
            (
                "p42-caldav.icloud.test".to_owned(),
                "/1234/calendars/main/new.ics".to_owned()
            ),
            (
                "p42-contacts.icloud.test".to_owned(),
                "/1234/carddavhome/main/new.vcf".to_owned()
            ),
        ]
    );

    // The login went to the configured servers and the two account hosts
    // with every request…
    for seen in e.seen().iter().chain(&f.seen()) {
        assert_eq!(seen.authorization.as_deref(), Some(BASIC), "{seen:?}");
    }
    let seen = p.seen();
    for s in seen.iter().filter(|s| s.host != "blog.icloud.test") {
        assert_eq!(s.authorization.as_deref(), Some(BASIC), "{s:?}");
    }
    // …the allowed host only got an OPTIONS, without it…
    assert!(
        seen.iter()
            .filter(|s| s.host == "blog.icloud.test")
            .all(|s| s.method == "OPTIONS" && s.authorization.is_none()),
        "{seen:?}"
    );
    // …and nothing else was asked, or even resolved. The calendar client
    // never touched p42-contacts' paths, nor the contacts client
    // p42-caldav's (each host only saw its own kind).
    assert_eq!(
        p.hosts(),
        vec![
            "blog.icloud.test",
            "p42-caldav.icloud.test",
            "p42-contacts.icloud.test"
        ]
    );
    assert!(
        seen.iter()
            .filter(|s| s.host == "p42-caldav.icloud.test")
            .all(|s| s.path.starts_with("/1234/calendars/")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .filter(|s| s.host == "p42-contacts.icloud.test")
            .all(|s| s.path.starts_with("/1234/carddavhome/")),
        "{seen:?}"
    );
    assert_eq!(
        run.dns.asked(),
        vec![
            "blog.icloud.test",
            "p42-caldav.icloud.test",
            "p42-contacts.icloud.test"
        ]
    );
}

/// `.well-known` redirects to the account host, where the principal is:
/// the login follows (iCloud may answer either way).
#[tokio::test]
async fn a_redirect_to_the_account_host_gets_the_login() {
    let mut p = Fake::bind().await;
    p.serve(partitions());
    let mut e = Fake::bind().await;
    e.serve(redirecting(format!(
        "{}/",
        p.named("p42-caldav.icloud.test")
    )));
    let mut config = GroupwareConfig::new(server(&format!("{}/", e.origin()), "caldav", &p));
    config.contacts = false;
    let run = open(quick(config)).await;
    until("the first round", || {
        run.account.status() == DavStatus::Ready
    })
    .await;

    let collections = run.store.dav_collections(run.id).await.unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(
        collections[0].props.href.as_str(),
        format!("{}/1234/calendars/main/", p.named("p42-caldav.icloud.test"))
    );
    let seen = p.seen();
    assert!(seen.iter().any(|s| s.method == "PROPFIND" && s.path == "/"));
    assert!(
        seen.iter().all(
            |s| s.host == "p42-caldav.icloud.test" && s.authorization.as_deref() == Some(BASIC)
        ),
        "{seen:?}"
    );
}

/// Redirects (or a principal) elsewhere are refused before anything is
/// resolved or sent: other names in the domain, the contacts pattern on
/// the calendar client, lookalikes, another port or scheme. Discovery
/// fails and nothing reaches P.
#[tokio::test]
async fn other_hosts_are_refused_before_connecting() {
    let mut p = Fake::bind().await;
    p.serve(partitions());
    let other_port = Fake::bind().await;
    for target in [
        format!("{}/", p.named("blog.icloud.test")),
        format!("{}/", p.named("p42-contacts.icloud.test")),
        format!("{}/", p.named("p42-caldav.icloud.test.evil.test")),
        format!("{}/", p.named("p42-caldav.icloud.test.")),
        format!("{}/", p.named("p12345-caldav.icloud.test")),
        format!("{}/", other_port.named("p42-caldav.icloud.test")),
        format!("https://p42-caldav.icloud.test:{}/", p.port),
    ] {
        for how in ["redirect", "principal"] {
            let mut e = Fake::bind().await;
            e.serve(if how == "redirect" {
                redirecting(target.clone())
            } else {
                principal_at(target.clone())
            });
            let mut config =
                GroupwareConfig::new(server(&format!("{}/", e.origin()), "caldav", &p));
            config.contacts = false;
            let run = open(quick(config)).await;
            until("the failed discovery", || {
                run.account.status() == DavStatus::Failing
            })
            .await;
            assert!(p.seen().is_empty(), "{how} {target}: {:?}", p.seen());
            assert!(
                run.dns.asked().is_empty(),
                "{how} {target}: {:?}",
                run.dns.asked()
            );
            assert!(
                run.store.dav_collections(run.id).await.unwrap().is_empty(),
                "{how} {target}"
            );
            assert!(
                e.seen()
                    .iter()
                    .all(|s| s.authorization.as_deref() == Some(BASIC))
            );
        }
    }
}
