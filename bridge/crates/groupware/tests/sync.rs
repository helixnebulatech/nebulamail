// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The read-only sync against a fake CalDAV and
//! CardDAV server (hyper, in memory), with the quirks of real ones:
//! truncated sync reports (507), expired tokens, Stalwart's full listing
//! for a token it does not know, missing etags, no sync-collection, 401,
//! and resources that do not parse. Also the writes' etags: weak ones
//! (never in `If-Match`) and servers that change what they store.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::format_push_string,
    reason = "integration test helpers"
)]

use std::collections::BTreeMap;
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
use nebula_groupware::model::sealed;
use nebula_groupware::{
    Contents, DavStatus, Groupware, GroupwareAccount, GroupwareConfig, GroupwareError,
    GroupwareEvent,
};
use nebula_store::{
    AccountId, ChangesSince, Collection, CollectionKind, DataType, NewAccount, Parsed, Seq, Store,
    UnixTime,
};
use net_guard::{DnsError, DomainName, LookupFuture, Resolver};
use secrecy::{ExposeSecret as _, SecretString};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

// ------------------------------------------------------------ fake server

const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
const CS: &str = "http://calendarserver.org/ns/";

#[derive(Clone)]
struct Item {
    etag: Option<String>,
    data: String,
    version: u64,
}

struct Coll {
    path: String,
    calendar: bool,
    name: String,
    version: u64,
    items: BTreeMap<String, Item>,
    /// Deleted member → version of the deletion.
    deleted: BTreeMap<String, u64>,
}

impl Coll {
    fn token(&self) -> String {
        format!("tok-{}", self.version)
    }
}

/// How the server answers a sync token it does not know.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unknown {
    /// `403` with `DAV:valid-sync-token` (RFC 6578, Radicale).
    Refuse,
    /// Every member, no error (Stalwart 0.16).
    Everything,
}

struct DavState {
    colls: Vec<Coll>,
    unknown: Unknown,
    /// Tokens older than this are forgotten.
    oldest_token: u64,
    /// Entries per sync answer before a 507.
    page: Option<usize>,
    etags_in_sync: bool,
    etags_in_multiget: bool,
    sync_supported: bool,
    unauthorized: bool,
    requests: Vec<String>,
    /// The address book lists vCard 4.0 in `supported-address-data`.
    vcard4: bool,
    /// Card name → its text as 4.0, sent when a multiget asks for 4.0
    /// (Nextcloud keeps cards as 3.0 and converts on request).
    as_vcard4: BTreeMap<String, String>,
    /// Bodies of the address book multigets.
    card_multigets: Vec<String>,
    /// Member name → an error status the server gives for it: in sync
    /// reports and listings as the response's status, in multigets for
    /// its data.
    failing: BTreeMap<String, u16>,
    /// Bodies of the calendar multigets.
    calendar_multigets: Vec<String>,
    /// Member name → how a multiget answers it instead of with its data:
    /// `None` leaves it out, `Some(status)` gives that status.
    multiget_answers: BTreeMap<String, Option<u16>>,
    /// The only `Authorization` accepted (others get 401), if set.
    accepted: Option<String>,
    /// The `Authorization` of every request.
    authorizations: Vec<Option<String>>,
    /// How long each answer takes.
    delay: Option<Duration>,
    /// New etags are weak (`W/"…"`), as Apache's `mod_dav` gives in the
    /// second a file changed.
    weak: bool,
    /// A PUT stores the text with a line of the server's own (sabre adds
    /// a `PRODID`, say).
    changes_on_put: bool,
    /// The `If-Match` of every PUT and DELETE of a member.
    if_matches: Vec<Option<String>>,
}

impl DavState {
    fn new() -> Self {
        Self {
            colls: vec![
                Coll {
                    path: "/dav/cal/alice/work/".to_owned(),
                    calendar: true,
                    name: "Work".to_owned(),
                    version: 0,
                    items: BTreeMap::new(),
                    deleted: BTreeMap::new(),
                },
                Coll {
                    path: "/dav/card/alice/book/".to_owned(),
                    calendar: false,
                    name: "Contacts".to_owned(),
                    version: 0,
                    items: BTreeMap::new(),
                    deleted: BTreeMap::new(),
                },
            ],
            unknown: Unknown::Refuse,
            oldest_token: 0,
            page: None,
            etags_in_sync: true,
            etags_in_multiget: true,
            sync_supported: true,
            unauthorized: false,
            requests: Vec::new(),
            vcard4: false,
            as_vcard4: BTreeMap::new(),
            card_multigets: Vec::new(),
            failing: BTreeMap::new(),
            calendar_multigets: Vec::new(),
            multiget_answers: BTreeMap::new(),
            accepted: None,
            authorizations: Vec::new(),
            delay: None,
            weak: false,
            changes_on_put: false,
            if_matches: Vec::new(),
        }
    }

    fn coll(&mut self, calendar: bool) -> &mut Coll {
        self.colls
            .iter_mut()
            .find(|c| c.calendar == calendar)
            .unwrap()
    }

    /// Creates or changes a member (a new etag each time, if `etag`).
    fn put(&mut self, calendar: bool, name: &str, data: &str, etag: bool) {
        let weak = if self.weak { "W/" } else { "" };
        let coll = self.coll(calendar);
        coll.version += 1;
        let version = coll.version;
        coll.deleted.remove(name);
        coll.items.insert(
            name.to_owned(),
            Item {
                etag: etag.then(|| format!("{weak}\"{name}-{version}\"")),
                data: data.to_owned(),
                version,
            },
        );
    }

    fn delete(&mut self, calendar: bool, name: &str) {
        let coll = self.coll(calendar);
        coll.version += 1;
        coll.items.remove(name);
        coll.deleted.insert(name.to_owned(), coll.version);
    }

    /// Forgets every token issued so far.
    fn expire_tokens(&mut self) {
        let newest = self.colls.iter().map(|c| c.version).max().unwrap_or(0);
        self.oldest_token = newest + 1;
        for coll in &mut self.colls {
            coll.version = coll.version.max(newest + 1);
        }
    }

    fn count(&self, needle: &str) -> usize {
        self.requests.iter().filter(|r| r.contains(needle)).count()
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;")
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

fn not_found(href: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
    )
}

fn etag_prop(etag: Option<&String>) -> String {
    etag.map_or_else(String::new, |etag| format!("<d:getetag>{etag}</d:getetag>"))
}

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn reply(status: u16, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .header("dav", "1, 3, access-control, calendar-access, addressbook")
        .body(Full::new(Bytes::from(body)).boxed())
        .unwrap()
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Vec<&'a str> {
    text.split(start)
        .skip(1)
        .filter_map(|rest| rest.split(end).next())
        .collect()
}

fn home(state: &DavState, calendar: bool) -> String {
    let (path, kind) = if calendar {
        ("/dav/cal/alice/", "<c:calendar/>")
    } else {
        ("/dav/card/alice/", "<r:addressbook/>")
    };
    let mut out = ok(path, "<d:resourcetype><d:collection/></d:resourcetype>");
    for coll in state.colls.iter().filter(|c| c.calendar == calendar) {
        let reports = if state.sync_supported {
            "<d:supported-report-set><d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report></d:supported-report-set>"
        } else {
            ""
        };
        let mut token = if state.sync_supported {
            format!("<d:sync-token>{}</d:sync-token>", coll.token())
        } else {
            String::new()
        };
        if !calendar {
            // Every book lists 3.0 (and jCard, which does not count).
            token.push_str(r#"<r:supported-address-data><r:address-data-type content-type="text/vcard" version="3.0"/><r:address-data-type content-type="application/vcard+json" version="4.0"/>"#);
            if state.vcard4 {
                token.push_str(r#"<r:address-data-type content-type="text/vcard" version="4.0"/>"#);
            }
            token.push_str("</r:supported-address-data>");
        }
        out.push_str(&ok(
            &coll.path,
            &format!(
                "<d:resourcetype><d:collection/>{kind}</d:resourcetype><d:displayname>{}</d:displayname>{token}<cs:getctag>ctag-{}</cs:getctag>{reports}",
                coll.name, coll.version
            ),
        ));
    }
    multistatus(&out)
}

fn sync(state: &DavState, coll: &Coll, body: &str) -> Response<Body> {
    let token = between(body, "<D:sync-token>", "</D:sync-token>")
        .first()
        .copied()
        .unwrap_or_default();
    // (version, href, change)
    let mut entries: Vec<(u64, String, Option<&Item>)> = Vec::new();
    let since = if token.is_empty() {
        None
    } else {
        match token
            .strip_prefix("tok-")
            .and_then(|n| n.parse::<u64>().ok())
        {
            Some(n) if n >= state.oldest_token && n <= coll.version => Some(n),
            _ if state.unknown == Unknown::Refuse => {
                return reply(
                    403,
                    r#"<?xml version="1.0"?><d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#
                        .to_owned(),
                );
            }
            _ => None,
        }
    };
    for (name, item) in &coll.items {
        if since.is_none_or(|n| item.version > n) {
            entries.push((item.version, format!("{}{name}", coll.path), Some(item)));
        }
    }
    if let Some(n) = since {
        for (name, version) in &coll.deleted {
            if *version > n {
                entries.push((*version, format!("{}{name}", coll.path), None));
            }
        }
    }
    entries.sort_by_key(|(version, ..)| *version);
    let mut new_token = coll.token();
    let mut truncated = false;
    if let Some(page) = state.page
        && entries.len() > page
    {
        entries.truncate(page);
        new_token = format!("tok-{}", entries.last().unwrap().0);
        truncated = true;
    }
    let mut out = String::new();
    for (_, href, item) in &entries {
        if let Some(status) = href
            .rsplit('/')
            .next()
            .and_then(|name| state.failing.get(name))
        {
            out.push_str(&failed(href, *status));
            continue;
        }
        match item {
            Some(item) => {
                let etag = if state.etags_in_sync {
                    etag_prop(item.etag.as_ref())
                } else {
                    String::new()
                };
                out.push_str(&ok(href, &etag));
            }
            None => out.push_str(&not_found(href)),
        }
    }
    if truncated {
        out.push_str(&format!(
            "<d:response><d:href>{}</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>",
            coll.path
        ));
    }
    out.push_str(&format!("<d:sync-token>{new_token}</d:sync-token>"));
    reply(207, multistatus(&out))
}

fn multiget(state: &DavState, coll: &Coll, body: &str) -> Response<Body> {
    let element = if coll.calendar {
        "c:calendar-data"
    } else {
        "r:address-data"
    };
    let wants_vcard4 = !coll.calendar && body.contains(r#"version="4.0""#);
    let mut out = String::new();
    for href in between(body, "<D:href>", "</D:href>") {
        let name = href.strip_prefix(&coll.path).unwrap_or_default();
        if let Some(status) = state.failing.get(name) {
            out.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>\"x\"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><{element}/></d:prop><d:status>HTTP/1.1 {status} Failed</d:status></d:propstat></d:response>"
            ));
            continue;
        }
        match state.multiget_answers.get(name) {
            Some(None) => continue,
            Some(Some(status)) => {
                out.push_str(&format!(
                    "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 {status} X</d:status></d:response>"
                ));
                continue;
            }
            None => {}
        }
        match coll.items.get(name) {
            Some(item) => {
                let etag = if state.etags_in_multiget {
                    etag_prop(item.etag.as_ref())
                } else {
                    String::new()
                };
                let data = state
                    .as_vcard4
                    .get(name)
                    .filter(|_| wants_vcard4)
                    .unwrap_or(&item.data);
                out.push_str(&ok(
                    href,
                    &format!("{etag}<{element}>{}</{element}>", escape(data)),
                ));
            }
            None => out.push_str(&not_found(href)),
        }
    }
    reply(207, multistatus(&out))
}

fn failed(href: &str, status: u16) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 {status} Failed</d:status></d:response>"
    )
}

fn members(state: &DavState, coll: &Coll) -> String {
    let mut out = ok(
        &coll.path,
        "<d:resourcetype><d:collection/></d:resourcetype>",
    );
    for (name, item) in &coll.items {
        if let Some(status) = state.failing.get(name) {
            out.push_str(&failed(&format!("{}{name}", coll.path), *status));
            continue;
        }
        out.push_str(&ok(
            &format!("{}{name}", coll.path),
            &format!("<d:resourcetype/>{}", etag_prop(item.etag.as_ref())),
        ));
    }
    multistatus(&out)
}

/// The conditional headers of a request.
#[derive(Default)]
struct Conditions {
    if_match: Option<String>,
    if_none_match_any: bool,
}

/// GET, PUT and DELETE of a member, with RFC 9110's strong comparison for
/// `If-Match` (§13.1.1: a weak tag on either side never matches).
fn member(
    state: &mut DavState,
    method: &str,
    path: &str,
    body: &str,
    conditions: &Conditions,
) -> Option<Response<Body>> {
    let (dir, name) = path.rsplit_once('/')?;
    let dir = format!("{dir}/");
    let calendar = state.colls.iter().find(|c| c.path == dir)?.calendar;
    let current = state
        .colls
        .iter()
        .find(|c| c.path == dir)?
        .items
        .get(name)
        .cloned();
    let content_type = if calendar {
        "text/calendar"
    } else {
        "text/vcard"
    };
    let with_etag = |status: u16, etag: Option<&String>, body: String| {
        let mut builder = Response::builder()
            .status(status)
            .header("content-type", content_type);
        if let Some(etag) = etag {
            builder = builder.header("etag", etag);
        }
        builder.body(Full::new(Bytes::from(body)).boxed()).unwrap()
    };
    let matches = |current: Option<&Item>| match (&conditions.if_match, current) {
        (None, _) => true,
        (Some(wanted), Some(item)) => {
            !wanted.starts_with("W/")
                && item
                    .etag
                    .as_deref()
                    .is_some_and(|etag| !etag.starts_with("W/") && etag == wanted)
        }
        (Some(_), None) => false,
    };
    Some(match method {
        "GET" => match current {
            Some(item) => with_etag(200, item.etag.as_ref(), item.data),
            None => reply(404, String::new()),
        },
        "PUT" => {
            state.if_matches.push(conditions.if_match.clone());
            if (conditions.if_none_match_any && current.is_some()) || !matches(current.as_ref()) {
                return Some(reply(412, String::new()));
            }
            let stored = if state.changes_on_put {
                body.replacen("\r\nEND:", "\r\nX-SERVER:mine\r\nEND:", 1)
            } else {
                body.to_owned()
            };
            state.put(calendar, name, &stored, true);
            let etag = state
                .coll(calendar)
                .items
                .get(name)
                .and_then(|i| i.etag.clone());
            with_etag(
                if current.is_some() { 204 } else { 201 },
                etag.as_ref(),
                String::new(),
            )
        }
        "DELETE" => {
            state.if_matches.push(conditions.if_match.clone());
            if current.is_none() {
                return Some(reply(404, String::new()));
            }
            if !matches(current.as_ref()) {
                return Some(reply(412, String::new()));
            }
            state.delete(calendar, name);
            reply(204, String::new())
        }
        _ => return None,
    })
}

fn handle(
    state: &mut DavState,
    method: &str,
    path: &str,
    body: &str,
    conditions: &Conditions,
) -> Response<Body> {
    state.requests.push(format!("{method} {path}"));
    if state.unauthorized {
        return reply(401, String::new());
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
                "<c:calendar-home-set><d:href>/dav/cal/alice/</d:href></c:calendar-home-set>\
                 <r:addressbook-home-set><d:href>/dav/card/alice/</d:href></r:addressbook-home-set>\
                 <c:calendar-user-address-set><d:href>mailto:alice@example.com</d:href></c:calendar-user-address-set>",
            )),
        ),
        ("PROPFIND", "/dav/cal/alice/") => reply(207, home(state, true)),
        ("PROPFIND", "/dav/card/alice/") => reply(207, home(state, false)),
        ("MKCALENDAR" | "MKCOL", path) => {
            let calendar = method == "MKCALENDAR";
            let home = if calendar {
                "/dav/cal/alice/"
            } else {
                "/dav/card/alice/"
            };
            if !path.starts_with(home) || state.colls.iter().any(|c| c.path == path) {
                return reply(405, String::new());
            }
            let name = between(body, "<D:displayname>", "</D:displayname>")
                .first()
                .map_or_else(String::new, |name| (*name).to_owned());
            state.colls.push(Coll {
                path: path.to_owned(),
                calendar,
                name,
                version: 0,
                items: BTreeMap::new(),
                deleted: BTreeMap::new(),
            });
            reply(201, String::new())
        }
        ("DELETE", path) if state.colls.iter().any(|c| c.path == path) => {
            state.colls.retain(|c| c.path != path);
            reply(204, String::new())
        }
        (method, path) => {
            let Some(coll) = state.colls.iter().find(|c| c.path == path) else {
                return member(state, method, path, body, conditions)
                    .unwrap_or_else(|| reply(404, String::new()));
            };
            match method {
                "PROPFIND" => reply(207, members(state, coll)),
                "REPORT" if body.contains("sync-collection") => sync(state, coll, body),
                "REPORT" => {
                    if coll.calendar {
                        state.calendar_multigets.push(body.to_owned());
                    } else {
                        state.card_multigets.push(body.to_owned());
                    }
                    let coll = state.colls.iter().find(|c| c.path == path).unwrap();
                    multiget(state, coll, body)
                }
                _ => reply(405, String::new()),
            }
        }
    }
}

struct Fake {
    port: u16,
    state: Arc<Mutex<DavState>>,
}

impl Fake {
    async fn start(state: DavState) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(state));
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
                                    return Ok::<_, Infallible>(reply(404, String::new()));
                                }
                                let authorization = parts
                                    .headers
                                    .get("authorization")
                                    .and_then(|v| v.to_str().ok())
                                    .map(str::to_owned);
                                let delay = {
                                    let mut state = state.lock().unwrap();
                                    state.authorizations.push(authorization.clone());
                                    state.delay
                                };
                                if let Some(delay) = delay {
                                    tokio::time::sleep(delay).await;
                                }
                                let mut state = state.lock().unwrap();
                                if state.accepted.is_some() && state.accepted != authorization {
                                    return Ok(reply(401, String::new()));
                                }
                                let header = |name: &str| {
                                    parts
                                        .headers
                                        .get(name)
                                        .and_then(|v| v.to_str().ok())
                                        .map(str::to_owned)
                                };
                                let conditions = Conditions {
                                    if_match: header("if-match"),
                                    if_none_match_any: header("if-none-match").as_deref()
                                        == Some("*"),
                                };
                                Ok::<_, Infallible>(handle(
                                    &mut state,
                                    parts.method.as_str(),
                                    parts.uri.path(),
                                    &body,
                                    &conditions,
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

    fn with<T>(&self, f: impl FnOnce(&mut DavState) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }
}

/// No DNS: the fake server is an IP address.
struct NoDns;

impl Resolver for NoDns {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async { Err(DnsError) })
    }
}

// ------------------------------------------------------------ helpers

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\n\
         DTSTAMP:20261027T120000Z\r\nDTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\n\
         SUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn card(uid: &str, family: &str, given: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:{given} {family}\r\nN:{family};{given};;;\r\nEND:VCARD\r\n"
    )
}

struct Setup {
    fake: Fake,
    store: Store,
    groupware: Groupware,
    account: Arc<GroupwareAccount>,
    id: AccountId,
    events: broadcast::Receiver<GroupwareEvent>,
    _dir: tempfile::TempDir,
}

fn key() -> AccountKey {
    AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    )
}

async fn setup(state: DavState) -> Setup {
    let fake = Fake::start(state).await;
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
    let server = DavServer::new(&format!("http://127.0.0.1:{}/dav/", fake.port))
        .unwrap()
        .with_private_network(true);
    let mut config = GroupwareConfig::new(server);
    // Rounds only when the test asks (or kicks).
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    config.first_retry = Duration::from_millis(50);
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
    let events = groupware.subscribe();
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key())
        .unwrap();
    Setup {
        fake,
        store,
        groupware,
        account,
        id,
        events,
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

async fn ready(setup: &Setup) {
    until("the first round", || {
        setup.account.status() == DavStatus::Ready
    })
    .await;
}

async fn collection(setup: &Setup, kind: CollectionKind) -> Collection {
    setup
        .store
        .dav_collections(setup.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.props.kind == kind && c.orphaned_at.is_none())
        .unwrap()
}

/// Visible resources: href path → id.
async fn visible(setup: &Setup, kind: CollectionKind) -> BTreeMap<String, i64> {
    let coll = collection(setup, kind).await;
    setup
        .store
        .dav_resources(setup.id, coll.id)
        .await
        .unwrap()
        .into_iter()
        .filter(nebula_store::StoredResource::is_visible)
        .map(|r| {
            let path = r.href.as_str().rsplit('/').next().unwrap().to_owned();
            (path, r.id.get())
        })
        .collect()
}

async fn state(setup: &Setup, data_type: DataType) -> Seq {
    setup.store.state(setup.id, data_type).await.unwrap()
}

/// (created, updated, destroyed) since `from`.
async fn delta(setup: &Setup, data_type: DataType, from: Seq) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
    match setup
        .store
        .changes_since(setup.id, data_type, from, 1000)
        .await
        .unwrap()
    {
        ChangesSince::Changes(c) => (c.created, c.updated, c.destroyed),
        ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!("cannot calculate"),
    }
}

async fn text_of(setup: &Setup, id: i64, key: Option<&AccountKey>) -> Option<String> {
    let id = nebula_store::ResourceId::from_raw(id).unwrap();
    let sealed = setup.store.dav_resource_data(setup.id, id).await.unwrap()?;
    let resource = setup.store.dav_resource(setup.id, id).await.unwrap()?;
    let key = key?;
    let text = sealed::open(key, resource.collection, &resource.href, &sealed).ok()?;
    Some(String::from_utf8(text.expose_secret().to_vec()).unwrap())
}

fn drain(events: &mut broadcast::Receiver<GroupwareEvent>) -> Vec<GroupwareEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

fn pushed(events: &[GroupwareEvent], data_type: DataType) -> bool {
    events.iter().any(|event| {
        matches!(event, GroupwareEvent::StateChanged(changed) if changed.data_types.contains(&data_type))
    })
}

fn initial() -> DavState {
    let mut state = DavState::new();
    state.put(true, "1.ics", &event("u1", "One"), true);
    state.put(true, "2.ics", &event("u2", "Two"), true);
    state.put(false, "a.vcf", &card("ca", "Zapata", "Ana"), true);
    state
}

// ------------------------------------------------------------ tests

#[tokio::test]
async fn creates_changes_and_deletes_are_seen_next_round() {
    let mut s = setup(initial()).await;
    ready(&s).await;
    let events = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(events.keys().collect::<Vec<_>>(), ["1.ics", "2.ics"]);
    let cards = visible(&s, CollectionKind::AddressBook).await;
    assert_eq!(cards.len(), 1);
    // Indexes in clear, the text sealed.
    let book = collection(&s, CollectionKind::AddressBook).await;
    let stored = s.store.dav_resources(s.id, book.id).await.unwrap();
    match &stored[0].parsed {
        Parsed::Valid(index) => {
            assert_eq!(index.uid.as_deref(), Some("ca"));
            assert_eq!(index.sort_name.as_deref(), Some("zapata ana"));
        }
        Parsed::Invalid => panic!("the card parses"),
    }
    let work = collection(&s, CollectionKind::Calendar).await;
    assert_eq!(work.props.name.as_deref(), Some("Work"));
    assert!(work.is_default && work.props.supports_sync);
    let first = drain(&mut s.events);
    assert!(pushed(&first, DataType::Calendar) && pushed(&first, DataType::CalendarEvent));
    assert!(pushed(&first, DataType::AddressBook) && pushed(&first, DataType::ContactCard));
    let discovered = s.store.dav_account(s.id).await.unwrap().unwrap();
    assert_eq!(discovered.user_addresses, ["mailto:alice@example.com"]);
    assert!(s.account.availability().calendars && s.account.availability().contacts);

    let before = state(&s, DataType::CalendarEvent).await;
    let cards_before = state(&s, DataType::ContactCard).await;
    s.fake.with(|state| {
        state.put(true, "3.ics", &event("u3", "Three"), true);
        state.put(true, "1.ics", &event("u1", "One, moved"), true);
        state.delete(true, "2.ics");
    });
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    let now = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(created, [now["3.ics"]]);
    assert_eq!(updated, [events["1.ics"]]);
    assert_eq!(destroyed, [events["2.ics"]]);
    // The address book did not change: no sync of it, no new state.
    assert_eq!(state(&s, DataType::ContactCard).await, cards_before);
    assert!(pushed(&drain(&mut s.events), DataType::CalendarEvent));
    // The local copy is the new text, sealed with the account's key.
    let wrong_key = key();
    assert_eq!(text_of(&s, events["1.ics"], Some(&wrong_key)).await, None);

    // The homes had a calendar and an address book: none was created.
    assert_eq!(s.fake.with(|state| state.count("MKC")), 0);

    // Nothing changed: only the homes are listed, nothing recorded.
    let reports = s.fake.with(|state| state.count("REPORT"));
    let recorded = s.account.sync_round(false).await.unwrap();
    assert!(recorded.is_empty());
    assert_eq!(s.fake.with(|state| state.count("REPORT")), reports);
    assert!(drain(&mut s.events).is_empty());
    s.groupware.close(s.id).await;
}

#[tokio::test]
async fn truncated_sync_reports_are_continued() {
    let mut first = initial();
    first.page = Some(2);
    for n in 0..5 {
        first.put(
            true,
            &format!("p{n}.ics"),
            &event(&format!("p{n}"), "Page"),
            true,
        );
    }
    let s = setup(first).await;
    ready(&s).await;
    assert_eq!(visible(&s, CollectionKind::Calendar).await.len(), 7);
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| {
        for n in 0..3 {
            state.put(
                true,
                &format!("q{n}.ics"),
                &event(&format!("q{n}"), "More"),
                true,
            );
        }
        state.delete(true, "p0.ics");
    });
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!((created.len(), updated.len(), destroyed.len()), (3, 0, 1));
}

#[tokio::test]
async fn an_expired_token_resyncs_without_spurious_changes() {
    let s = setup(initial()).await;
    ready(&s).await;
    let ids = visible(&s, CollectionKind::Calendar).await;
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| {
        state.delete(true, "2.ics");
        state.expire_tokens();
    });
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert!(created.is_empty() && updated.is_empty());
    assert_eq!(destroyed, [ids["2.ics"]]);
    // It started over without a token after the 403.
    assert!(
        s.fake
            .with(|state| state.count("REPORT /dav/cal/alice/work/"))
            >= 3
    );
}

/// Stalwart 0.16 answers a token it does not know with every member and no
/// error. The etags avoid spurious changes and a full listing finds the
/// deletions. (If every remaining member changed too, the answer can't be
/// told from a normal one: the daily full comparison finds the deletions.)
#[tokio::test]
async fn stalwart_full_answers_to_unknown_tokens() {
    let mut first = initial();
    first.put(true, "3.ics", &event("u3", "Unchanged"), true);
    first.unknown = Unknown::Everything;
    let s = setup(first).await;
    ready(&s).await;
    let ids = visible(&s, CollectionKind::Calendar).await;
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| {
        state.delete(true, "2.ics");
        state.put(true, "1.ics", &event("u1", "Changed"), true);
        state.expire_tokens();
    });
    let listings = s
        .fake
        .with(|state| state.count("PROPFIND /dav/cal/alice/work/"));
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert!(created.is_empty());
    assert_eq!(updated, [ids["1.ics"]]);
    assert_eq!(destroyed, [ids["2.ics"]]);
    assert_eq!(
        s.fake
            .with(|state| state.count("PROPFIND /dav/cal/alice/work/")),
        listings + 1
    );

    // Only unknown tokens look like that: a normal round is incremental.
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake
        .with(|state| state.put(true, "4.ics", &event("u4", "Four"), true));
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!((created.len(), updated.len(), destroyed.len()), (1, 0, 0));
    assert_eq!(
        s.fake
            .with(|state| state.count("PROPFIND /dav/cal/alice/work/")),
        listings + 1
    );
}

/// Without etags the text is compared: the same text is no change, even
/// under a new etag.
#[tokio::test]
async fn resources_without_etags_are_compared_by_text() {
    let mut first = initial();
    first.etags_in_sync = false;
    first.etags_in_multiget = false;
    let s = setup(first).await;
    ready(&s).await;
    let ids = visible(&s, CollectionKind::Calendar).await;
    let before = state(&s, DataType::CalendarEvent).await;
    // Touched, same text and no etag.
    s.fake
        .with(|state| state.put(true, "1.ics", &event("u1", "One"), false));
    s.account.sync_round(false).await.unwrap();
    assert_eq!(state(&s, DataType::CalendarEvent).await, before);
    // Same text, a new etag this time.
    s.fake.with(|state| {
        state.etags_in_multiget = true;
        state.put(true, "1.ics", &event("u1", "One"), true);
    });
    s.account.sync_round(false).await.unwrap();
    assert_eq!(state(&s, DataType::CalendarEvent).await, before);
    // Really changed.
    s.fake
        .with(|state| state.put(true, "1.ics", &event("u1", "Other"), false));
    s.account.sync_round(false).await.unwrap();
    let (_, updated, _) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!(updated, [ids["1.ics"]]);
}

#[tokio::test]
async fn without_sync_collection_the_ctag_decides() {
    let mut first = initial();
    first.sync_supported = false;
    let s = setup(first).await;
    ready(&s).await;
    assert!(
        !collection(&s, CollectionKind::Calendar)
            .await
            .props
            .supports_sync
    );
    assert_eq!(s.fake.with(|state| state.count("sync")), 0);
    let listings = s
        .fake
        .with(|state| state.count("PROPFIND /dav/cal/alice/work/"));
    // Same ctag: no listing.
    s.account.sync_round(false).await.unwrap();
    assert_eq!(
        s.fake
            .with(|state| state.count("PROPFIND /dav/cal/alice/work/")),
        listings
    );
    let ids = visible(&s, CollectionKind::Calendar).await;
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| {
        state.delete(true, "1.ics");
        state.put(true, "2.ics", &event("u2", "Changed"), true);
    });
    s.account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert!(created.is_empty());
    assert_eq!(updated, [ids["2.ics"]]);
    assert_eq!(destroyed, [ids["1.ics"]]);
}

#[tokio::test]
async fn a_refused_login_turns_off_only_dav() {
    let mut first = initial();
    first.unauthorized = true;
    let mut s = setup(first).await;
    until("the 401", || s.account.status() == DavStatus::Unauthorized).await;
    let event = s.events.recv().await.unwrap();
    assert_eq!(event, GroupwareEvent::Unauthorized(s.id));
    assert_eq!(
        s.account.availability(),
        nebula_groupware::Availability::default()
    );
    // Not retried by itself.
    let requests = s.fake.with(|state| state.requests.len());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.fake.with(|state| state.requests.len()), requests);
    // A new login starts it again.
    s.fake.with(|state| state.unauthorized = false);
    let again = s
        .groupware
        .open(s.id, "alice", SecretString::from("new password"), key())
        .unwrap();
    until("the new login", || again.status() == DavStatus::Ready).await;

    // A 401 later on (password changed on the server) stops it too, and
    // keeps what was synced.
    s.fake.with(|state| state.unauthorized = true);
    again.kick();
    until("the later 401", || {
        again.status() == DavStatus::Unauthorized
    })
    .await;
    assert_eq!(visible(&s, CollectionKind::Calendar).await.len(), 2);
}

#[tokio::test]
async fn a_resource_that_does_not_parse_is_kept_but_hidden() {
    let mut first = initial();
    first.put(true, "bad.ics", "BEGIN:VCALENDAR\r\nthis is not\r\n", true);
    first.put(false, "bad.vcf", "BEGIN:VCARD\r\nVERSION:4.0\r\n", true);
    let s = setup(first).await;
    ready(&s).await;
    assert_eq!(visible(&s, CollectionKind::Calendar).await.len(), 2);
    let work = collection(&s, CollectionKind::Calendar).await;
    let all = s.store.dav_resources(s.id, work.id).await.unwrap();
    let bad = all
        .iter()
        .find(|r| r.href.as_str().ends_with("bad.ics"))
        .unwrap();
    assert_eq!(bad.parsed, Parsed::Invalid);
    assert!(bad.etag.is_some());
    // Created only for what parsed.
    let (created, ..) = delta(&s, DataType::CalendarEvent, Seq::ZERO).await;
    assert_eq!(created.len(), 2);
    // Fixed on the server: now it is created.
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake
        .with(|state| state.put(true, "bad.ics", &event("fixed", "Fixed"), true));
    s.account.sync_round(false).await.unwrap();
    let (created, ..) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!(created, [bad.id.get()]);
}

#[tokio::test]
async fn a_kick_and_a_watcher_wake_the_task() {
    let s = setup(initial()).await;
    ready(&s).await;
    let rounds = s.fake.with(|state| state.count("PROPFIND /dav/cal/alice/"));
    s.fake
        .with(|state| state.put(true, "k.ics", &event("k", "Kicked"), true));
    s.groupware.kick(s.id);
    until("the kicked round", || {
        s.fake.with(|state| state.count("PROPFIND /dav/cal/alice/")) > rounds
    })
    .await;
    // A watcher only shortens the wait (one hour here): no round now.
    let rounds = s.fake.with(|state| state.count("PROPFIND /dav/cal/alice/"));
    let watch = s.groupware.watch(s.id);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        s.fake.with(|state| state.count("PROPFIND /dav/cal/alice/")),
        rounds
    );
    drop(watch);
    s.groupware.close(s.id).await;
}

#[tokio::test]
async fn empty_homes_get_a_calendar_and_an_address_book() {
    // A new user of Radicale or Baïkal: homes without collections.
    let mut state = DavState::new();
    state.colls.clear();
    let s = setup(state).await;
    ready(&s).await;
    let calendar = collection(&s, CollectionKind::Calendar).await;
    let book = collection(&s, CollectionKind::AddressBook).await;
    assert_eq!(calendar.props.name.as_deref(), Some("Calendar"));
    assert_eq!(book.props.name.as_deref(), Some("Contacts"));
    assert!(calendar.is_default && book.is_default);
    assert_eq!(
        s.fake
            .with(|state| state.count("MKCALENDAR /dav/cal/alice/calendar/")),
        1
    );
    assert_eq!(
        s.fake
            .with(|state| state.count("MKCOL /dav/card/alice/contacts/")),
        1
    );
    // And a list for tasks (the calendar above takes only events).
    assert_eq!(
        s.fake
            .with(|state| state.count("MKCALENDAR /dav/cal/alice/tasks/")),
        1
    );
    // Only once per login: later rounds don't make more.
    s.account.sync_round(false).await.unwrap();
    assert_eq!(s.fake.with(|state| state.count("MKC")), 3);
    s.groupware.close(s.id).await;
}

/// A Nextcloud-like book keeps a 4.0 group as 3.0 (`X-ADDRESSBOOKSERVER-*`)
/// and sends 4.0 when asked: the multiget asks for 4.0 only when the book
/// lists `text/vcard` 4.0, and the group is a group either way.
#[tokio::test]
async fn cards_are_asked_for_as_vcard4_when_the_book_takes_it() {
    const STORED: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:urn:uuid:g1\r\nFN:Equipo\r\n\
        X-ADDRESSBOOKSERVER-KIND:GROUP\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:x1\r\nEND:VCARD\r\n";
    const AS_4: &str = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:uuid:g1\r\nFN:Equipo\r\n\
        KIND:group\r\nMEMBER:urn:uuid:x1\r\nEND:VCARD\r\n";
    for vcard4 in [true, false] {
        let mut first = DavState::new();
        first.vcard4 = vcard4;
        first.put(false, "g.vcf", STORED, true);
        first.as_vcard4.insert("g.vcf".to_owned(), AS_4.to_owned());
        let s = setup(first).await;
        ready(&s).await;
        let bodies = s.fake.with(|state| state.card_multigets.clone());
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        if vcard4 {
            assert!(
                bodies[0].contains(r#"<CR:address-data content-type="text/vcard" version="4.0"/>"#),
                "{}",
                bodies[0]
            );
        } else {
            assert!(bodies[0].contains("<CR:address-data/>"), "{}", bodies[0]);
            assert!(!bodies[0].contains(r#"version="4.0""#), "{}", bodies[0]);
        }
        let book = collection(&s, CollectionKind::AddressBook).await;
        let stored = s.store.dav_resources(s.id, book.id).await.unwrap();
        assert_eq!(stored.len(), 1);
        // Same etag and href either way.
        assert_eq!(stored[0].etag.as_deref(), Some("\"g.vcf-1\""));
        assert!(
            stored[0]
                .href
                .as_str()
                .ends_with("/dav/card/alice/book/g.vcf")
        );
        let text = s
            .account
            .resource_text(stored[0].id)
            .await
            .unwrap()
            .unwrap()
            .text;
        assert_eq!(text, if vcard4 { AS_4 } else { STORED });
        let json = nebula_groupware::model::convert::vcard_to_jscontact(text.as_bytes()).unwrap();
        assert_eq!(json["kind"], "group");
        assert_eq!(json["members"]["urn:uuid:x1"], true);
        s.groupware.close(s.id).await;
    }
}

/// Deleting a collection that must be empty asks the server first: a card
/// the local copy doesn't know yet keeps the book; with `Remove` it goes.
#[tokio::test]
async fn a_collection_that_must_be_empty_is_listed_before_the_delete() {
    let s = setup(DavState::new()).await;
    ready(&s).await;
    let book = collection(&s, CollectionKind::AddressBook).await;
    assert!(visible(&s, CollectionKind::AddressBook).await.is_empty());
    s.fake
        .with(|state| state.put(false, "new.vcf", &card("cn", "Nuevo", "Ana"), true));
    let error = s
        .account
        .delete_collection(book.id, Contents::MustBeEmpty)
        .await
        .unwrap_err();
    assert!(matches!(error, GroupwareError::HasContents), "{error:?}");
    let there =
        |path: &'static str| move |state: &mut DavState| state.colls.iter().any(|c| c.path == path);
    assert!(s.fake.with(there("/dav/card/alice/book/")));
    assert_eq!(s.fake.with(|state| state.count("DELETE")), 0);
    s.account
        .delete_collection(book.id, Contents::Remove)
        .await
        .unwrap();
    assert!(!s.fake.with(there("/dav/card/alice/book/")));
    // An empty one goes with `MustBeEmpty`.
    let work = collection(&s, CollectionKind::Calendar).await;
    s.account
        .delete_collection(work.id, Contents::MustBeEmpty)
        .await
        .unwrap();
    assert!(!s.fake.with(there("/dav/cal/alice/work/")));
}

/// A resource the server cannot give now (500 in the listing or for its
/// data) or one over the size limit is not taken as deleted: the round
/// works, the others are stored, and the stored copy stays visible (it
/// is asked for again).
#[tokio::test]
async fn resources_the_server_cannot_give_are_not_deleted() {
    let s = setup(initial()).await;
    ready(&s).await;
    let first = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(first.keys().collect::<Vec<_>>(), ["1.ics", "2.ics"]);
    let before = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| {
        state.put(true, "2.ics", &event("u2", "Two, changed"), true);
        state.failing.insert("2.ics".to_owned(), 500);
        let big = event("big", &"x".repeat(1024 * 1024 + 10));
        state.put(true, "big.ics", &big, true);
        state.put(true, "3.ics", &event("u3", "Three"), true);
    });
    s.account.sync_round(false).await.unwrap();
    let now = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(now.keys().collect::<Vec<_>>(), ["1.ics", "2.ics", "3.ics"]);
    let (created, _, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!(created, [now["3.ics"]]);
    assert!(destroyed.is_empty(), "{destroyed:?}");
    // The 500 is not resolved: the token waits for it and the next round
    // asks for it again (after three rounds without an answer it is kept
    // as a marker).
    // A full listing with the 500 still shows it: kept.
    s.account.sync_round(true).await.unwrap();
    let after = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        ["1.ics", "2.ics", "3.ics"]
    );
    assert_eq!(after["2.ics"], first["2.ics"]);
    s.groupware.close(s.id).await;
}

/// A resource indexed wrongly before (an endless series with an EXDATE
/// had an end) is indexed again when it is fetched with the same text:
/// the migration forgets its etag so that happens once.
#[tokio::test]
async fn the_same_text_is_indexed_again() {
    let endless = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula//test//EN\r\nBEGIN:VEVENT\r\n\
         UID:u9\r\nDTSTAMP:20261027T120000Z\r\nDTSTART:20261028T100000Z\r\nDURATION:PT1H\r\n\
         RRULE:FREQ=DAILY\r\nEXDATE:20261029T100000Z\r\nSUMMARY:Daily\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let mut first = initial();
    first.put(true, "9.ics", endless, true);
    let s = setup(first).await;
    ready(&s).await;
    let coll = collection(&s, CollectionKind::Calendar).await;
    let stored = s
        .store
        .dav_resources(s.id, coll.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.href.as_str().ends_with("/9.ics"))
        .unwrap();
    let Parsed::Valid(index) = &stored.parsed else {
        panic!("not parsed");
    };
    assert_eq!(index.utc_end, None);
    // As an older bridge indexed it: with an end.
    let sealed = s
        .store
        .dav_resource_data(s.id, stored.id)
        .await
        .unwrap()
        .unwrap();
    let mut wrong = index.clone();
    wrong.utc_end = Some(UnixTime::from_secs(1_793_000_000));
    s.store
        .apply_resources(
            s.id,
            coll.id,
            vec![nebula_store::ResourceWrite {
                href: stored.href.clone(),
                etag: None,
                sealed,
                size: stored.size,
                parsed: Parsed::Valid(wrong),
            }],
            Vec::new(),
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap();
    let before = state(&s, DataType::CalendarEvent).await;
    s.account.sync_round(true).await.unwrap();
    let again = s
        .store
        .dav_resource(s.id, stored.id)
        .await
        .unwrap()
        .unwrap();
    let Parsed::Valid(index) = again.parsed else {
        panic!("not parsed");
    };
    assert_eq!(index.utc_end, None);
    assert!(again.etag.is_some());
    let (_, updated, _) = delta(&s, DataType::CalendarEvent, before).await;
    assert_eq!(updated, [stored.id.get()]);
    // Indexed right: the same text again is no change.
    let now = state(&s, DataType::CalendarEvent).await;
    s.fake.with(|state| state.put(true, "9.ics", endless, true));
    s.account.sync_round(false).await.unwrap();
    assert_eq!(state(&s, DataType::CalendarEvent).await, now);
}

/// The calendar resource stored under the member `name`.
async fn stored(setup: &Setup, name: &str) -> Option<nebula_store::StoredResource> {
    let coll = collection(setup, CollectionKind::Calendar).await;
    setup
        .store
        .dav_resources(setup.id, coll.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.href.as_str().ends_with(&format!("/{name}")))
}

#[tokio::test]
async fn a_member_left_out_holds_the_token_until_it_comes() {
    let mut first = initial();
    first.put(true, "3.ics", &event("u3", "Three"), true);
    first.multiget_answers.insert("3.ics".to_owned(), None);
    let s = setup(first).await;
    ready(&s).await;
    // The others are applied, but the token waits.
    let events = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(events.keys().collect::<Vec<_>>(), ["1.ics", "2.ics"]);
    let coll = collection(&s, CollectionKind::Calendar).await;
    assert_eq!(coll.sync.sync_token, None);
    assert_eq!(coll.sync.synced_at, None);
    // The server gives it now: the token moves.
    s.fake.with(|state| state.multiget_answers.clear());
    s.account.sync_round(false).await.unwrap();
    let events = visible(&s, CollectionKind::Calendar).await;
    assert_eq!(
        events.keys().collect::<Vec<_>>(),
        ["1.ics", "2.ics", "3.ics"]
    );
    let coll = collection(&s, CollectionKind::Calendar).await;
    assert!(coll.sync.sync_token.is_some());
    // Only the missing one was asked for again.
    let last = s
        .fake
        .with(|state| state.calendar_multigets.last().cloned());
    let last = last.unwrap();
    assert!(last.contains("3.ics") && !last.contains("1.ics"));
}

#[tokio::test]
async fn a_member_never_answered_is_kept_unreadable_after_some_rounds() {
    let mut first = initial();
    first.put(true, "3.ics", &event("u3", "Three"), true);
    first.multiget_answers.insert("3.ics".to_owned(), None);
    let s = setup(first).await;
    ready(&s).await;
    s.account.sync_round(false).await.unwrap();
    assert_eq!(
        collection(&s, CollectionKind::Calendar)
            .await
            .sync
            .sync_token,
        None
    );
    assert!(stored(&s, "3.ics").await.is_none());
    // Third round in a row: given up on, the token moves.
    s.account.sync_round(false).await.unwrap();
    let marker = stored(&s, "3.ics").await.unwrap();
    assert_eq!(marker.parsed, Parsed::Invalid);
    assert!(marker.etag.is_some());
    assert!(
        collection(&s, CollectionKind::Calendar)
            .await
            .sync
            .sync_token
            .is_some()
    );
    // Not asked for again while its etag stays.
    let multigets = s.fake.with(|state| state.calendar_multigets.len());
    s.fake
        .with(|state| state.put(true, "4.ics", &event("u4", "Four"), true));
    s.account.sync_round(false).await.unwrap();
    let last = s.fake.with(|state| state.calendar_multigets.clone());
    assert_eq!(last.len(), multigets + 1);
    assert!(!last.last().unwrap().contains("3.ics"));
    // Changed on the server and answered: it shows.
    s.fake.with(|state| {
        state.multiget_answers.clear();
        state.put(true, "3.ics", &event("u3", "Three again"), true);
    });
    s.account.sync_round(false).await.unwrap();
    assert!(stored(&s, "3.ics").await.unwrap().is_visible());
}

#[tokio::test]
async fn statuses_of_members_are_told_apart() {
    let mut first = initial();
    for name in ["big.ics", "locked.ics", "flaky.ics"] {
        first.put(true, name, &event(name, "Odd"), true);
    }
    first
        .multiget_answers
        .insert("big.ics".to_owned(), Some(507));
    first
        .multiget_answers
        .insert("locked.ics".to_owned(), Some(403));
    first
        .multiget_answers
        .insert("flaky.ics".to_owned(), Some(500));
    let s = setup(first).await;
    ready(&s).await;
    // Too large and forbidden: kept unreadable with their etag at once.
    for name in ["big.ics", "locked.ics"] {
        let marker = stored(&s, name).await.unwrap();
        assert_eq!(marker.parsed, Parsed::Invalid, "{name}");
        assert!(marker.etag.is_some(), "{name}");
    }
    // A 500 may pass: not stored, and the token waits.
    assert!(stored(&s, "flaky.ics").await.is_none());
    assert_eq!(
        collection(&s, CollectionKind::Calendar)
            .await
            .sync
            .sync_token,
        None
    );
    s.fake.with(|state| {
        state.multiget_answers.remove("flaky.ics");
    });
    s.account.sync_round(false).await.unwrap();
    assert!(stored(&s, "flaky.ics").await.unwrap().is_visible());
    assert!(
        collection(&s, CollectionKind::Calendar)
            .await
            .sync
            .sync_token
            .is_some()
    );
    let last = s
        .fake
        .with(|state| state.calendar_multigets.last().cloned());
    let last = last.unwrap();
    assert!(!last.contains("big.ics") && !last.contains("locked.ics"));

    // A member changed on the server that fails with 500 is not deleted:
    // still visible, no destroyed change. A 410 is.
    let before = state(&s, DataType::CalendarEvent).await;
    let one = stored(&s, "1.ics").await.unwrap();
    s.fake.with(|state| {
        state.put(true, "1.ics", &event("u1", "One changed"), true);
        state.put(true, "2.ics", &event("u2", "Two changed"), true);
        state.multiget_answers.insert("1.ics".to_owned(), Some(500));
        state.multiget_answers.insert("2.ics".to_owned(), Some(410));
    });
    s.account.sync_round(false).await.unwrap();
    assert!(stored(&s, "1.ics").await.unwrap().is_visible());
    assert!(!stored(&s, "2.ics").await.unwrap().is_visible());
    let (_, _, destroyed) = delta(&s, DataType::CalendarEvent, before).await;
    assert!(!destroyed.contains(&one.id.get()));
    assert_eq!(destroyed.len(), 1);
}

/// Logging in again with a new password changes it in place: the same
/// running account, which sends the new one from then on.
#[tokio::test]
async fn a_new_password_is_used_at_once() {
    const OLD: &str = "Basic YWxpY2U6cGFzc3dvcmQ=";
    const NEW: &str = "Basic YWxpY2U6bmV3IHBhc3N3b3Jk";
    let s = setup(initial()).await;
    ready(&s).await;
    assert_eq!(
        s.fake
            .with(|state| state.authorizations.last().cloned())
            .flatten()
            .as_deref(),
        Some(OLD)
    );
    // The password changes on the server, then the user logs in again.
    s.fake.with(|state| state.accepted = Some(NEW.to_owned()));
    let again = s
        .groupware
        .open(s.id, "alice", SecretString::from("new password"), key())
        .unwrap();
    assert!(Arc::ptr_eq(&again, &s.account));
    s.fake
        .with(|state| state.put(true, "3.ics", &event("u3", "Three"), true));
    s.account.sync_round(false).await.unwrap();
    assert_eq!(s.account.status(), DavStatus::Ready);
    assert!(
        visible(&s, CollectionKind::Calendar)
            .await
            .contains_key("3.ics")
    );
    assert_eq!(
        s.fake
            .with(|state| state.authorizations.last().cloned())
            .flatten()
            .as_deref(),
        Some(NEW)
    );
}

/// Closing an account does not wait for a server that takes a minute to
/// answer, and nothing it sends after that carries the password.
#[tokio::test]
async fn close_does_not_wait_for_a_slow_server() {
    let mut first = initial();
    first.delay = Some(Duration::from_mins(1));
    let s = setup(first).await;
    // Discovery is waiting for its answer.
    until("the first request", || {
        s.fake.with(|state| !state.authorizations.is_empty())
    })
    .await;
    tokio::time::timeout(Duration::from_secs(2), s.groupware.close(s.id))
        .await
        .unwrap();
    let sent = s.fake.with(|state| state.authorizations.len());
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.fake.with(|state| {
        assert!(
            state.authorizations[sent..].iter().all(Option::is_none),
            "{:?}",
            state.authorizations
        );
    });
}

// ------------------------------------------------------------ writes and etags

/// Every resource on the server has a weak etag: `If-Match` never takes
/// one (RFC 9110 §13.1.1, it would never match), and without a strong
/// etag even after a GET nothing is written.
#[tokio::test]
async fn weak_etags_are_never_sent_in_if_match() {
    let mut first = DavState::new();
    first.weak = true;
    first.put(true, "1.ics", &event("u1", "One"), true);
    let s = setup(first).await;
    ready(&s).await;
    let one = stored(&s, "1.ics").await.unwrap();
    assert!(one.etag.as_deref().unwrap().starts_with("W/"));
    let error = s
        .account
        .replace_resource(&one, event("u1", "Changed"))
        .await
        .unwrap_err();
    assert!(matches!(error, GroupwareError::NoEtag), "{error:?}");
    let error = s.account.delete_resource(&one).await.unwrap_err();
    assert!(matches!(error, GroupwareError::NoEtag), "{error:?}");
    s.fake.with(|state| {
        assert!(state.if_matches.is_empty(), "{:?}", state.if_matches);
        assert_eq!(state.count("PUT "), 0);
    });
}

/// A weak etag stored, a strong one on the server now for the same text
/// (Apache's `mod_dav`, a second later): the write uses the strong one. Its
/// answer has a weak etag and the server changed the text: what is stored
/// is what a GET gives (RFC 4791 §5.3.4), never our own bytes.
#[tokio::test]
async fn a_weak_etag_is_read_again_before_and_after_a_write() {
    let mut first = DavState::new();
    first.weak = true;
    first.put(true, "1.ics", &event("u1", "One"), true);
    let s = setup(first).await;
    ready(&s).await;
    let one = stored(&s, "1.ics").await.unwrap();
    let strong = s.fake.with(|state| {
        state.weak = false;
        state.put(true, "1.ics", &event("u1", "One"), true);
        state.weak = true;
        state.changes_on_put = true;
        state.coll(true).items["1.ics"].etag.clone().unwrap()
    });
    assert!(strong.starts_with('"'));
    let written = s
        .account
        .replace_resource(&one, event("u1", "Changed"))
        .await
        .unwrap();
    s.fake
        .with(|state| assert_eq!(state.if_matches, [Some(strong.clone())]));
    let local = s.account.resource_text(written.id).await.unwrap().unwrap();
    assert!(local.text.contains("SUMMARY:Changed"), "{}", local.text);
    assert!(local.text.contains("X-SERVER:mine"), "{}", local.text);
    assert!(written.etag.as_deref().unwrap().starts_with("W/"));
}

/// With a weak etag stored, a resource that changed on the server since
/// the local copy is a conflict (the caller patches it again), not a write
/// over someone else's change.
#[tokio::test]
async fn a_weak_etag_and_a_changed_resource_is_a_conflict() {
    let mut first = DavState::new();
    first.weak = true;
    first.put(true, "1.ics", &event("u1", "One"), true);
    let s = setup(first).await;
    ready(&s).await;
    let one = stored(&s, "1.ics").await.unwrap();
    s.fake.with(|state| {
        state.weak = false;
        state.put(true, "1.ics", &event("u1", "Someone else"), true);
    });
    let error = s
        .account
        .replace_resource(&one, event("u1", "Mine"))
        .await
        .unwrap_err();
    assert!(error.is_conflict(), "{error:?}");
    s.fake.with(|state| assert!(state.if_matches.is_empty()));
    // The copy read meanwhile is the server's: a retry works on it.
    let fresh = s.account.resource_text(one.id).await.unwrap().unwrap();
    assert!(fresh.text.contains("Someone else"));
    s.account
        .replace_resource(&fresh.resource, event("u1", "Mine"))
        .await
        .unwrap();
    s.fake.with(|state| {
        assert_eq!(state.if_matches.len(), 1);
        assert!(!state.if_matches[0].as_deref().unwrap().starts_with("W/"));
    });
}
