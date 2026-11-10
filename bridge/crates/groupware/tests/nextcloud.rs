// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Nextcloud's calendar extras: publishing,
//! subscriptions and the trash bin, against a fake server (hyper, in
//! memory) that answers as Nextcloud 34 did live (2026-10-30), and the
//! same calls against a server without them (Radicale-like): nothing of
//! it is offered there, and Nextcloud's headers are never sent to it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::format_push_string,
    clippy::struct_excessive_bools,
    clippy::needless_pass_by_value,
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
use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{ExternalUrl, ResourceName};
use nebula_groupware::{
    DavStatus, Groupware, GroupwareAccount, GroupwareConfig, GroupwareError, TrashItem, TrashKind,
};
use nebula_store::{AccountId, Collection, DataType, NewAccount, Rights, Store, UnixTime};
use net_guard::{DnsError, DomainName, LookupFuture, Resolver};
use secrecy::SecretString;
use tokio::net::TcpListener;

type Body = Full<Bytes>;

/// One request as the fake saw it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
    webcal: bool,
    no_trashbin: bool,
    destination: Option<String>,
}

struct State {
    nextcloud: bool,
    published: bool,
    /// Subscriptions: path and source.
    subscriptions: Vec<(String, String)>,
    /// What is in the trash bin: calendar `old`, event `51.ics`.
    old_calendar: bool,
    trashed_event: bool,
    requests: Vec<Seen>,
}

fn reply(nextcloud: bool, status: u16, body: &str) -> Response<Body> {
    let dav = if nextcloud {
        "1, 3, extended-mkcol, access-control, calendarserver-principal-property-search, oc-resource-sharing, calendar-access, calendar-proxy, calendar-auto-schedule, calendar-availability, nc-calendar-trashbin, nc-calendar-webcal-cache, calendarserver-subscribed, oc-calendar-publishing, calendarserver-sharing, nc-paginate, 2"
    } else {
        "1, 2, 3, calendar-access, extended-mkcol"
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .header("dav", dav)
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap()
}

fn multistatus(inner: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/" xmlns:O="http://owncloud.org/ns" xmlns:NC="http://nextcloud.com/ns">{inner}</D:multistatus>"#
    )
}

fn response(href: &str, props: &str) -> String {
    format!(
        "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{props}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
    )
}

const ALL: &str = "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set>";
/// A cached subscription's privileges, as Nextcloud gives them.
const CACHED: &str = "<D:current-user-privilege-set><D:privilege><D:write-properties/></D:privilege><D:privilege><C:read-free-busy/></D:privilege><D:privilege><D:read/></D:privilege></D:current-user-privilege-set>";

/// The calendar home: the calendar `work`, the deleted `old` and the
/// subscriptions, as calendars with the webcal header and as
/// `CS:subscribed` with their source without it.
fn home(state: &State, webcal: bool) -> String {
    let mut out = response(
        "/dav/cal/alice/",
        "<D:resourcetype><D:collection/></D:resourcetype>",
    );
    let publish = if state.published {
        "<CS:publish-url><D:href>https://cloud.example.com/remote.php/dav/public-calendars/SECRET</D:href></CS:publish-url>"
    } else {
        ""
    };
    let invite = if state.nextcloud { "<O:invite/>" } else { "" };
    out.push_str(&response(
        "/dav/cal/alice/work/",
        &format!(
            "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
             <D:displayname>Work</D:displayname><D:sync-token>t1</D:sync-token>\
             <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}{invite}{publish}"
        ),
    ));
    if !state.nextcloud {
        return out;
    }
    if state.old_calendar {
        out.push_str(&response(
            "/dav/cal/alice/old/",
            "<D:resourcetype><D:collection/><NC:deleted-calendar/></D:resourcetype>\
             <D:displayname>Old</D:displayname><D:sync-token>t3</D:sync-token>\
             <NC:deleted-at>2026-10-28T09:31:37+00:00</NC:deleted-at>",
        ));
    }
    for (path, source) in &state.subscriptions {
        if webcal {
            out.push_str(&response(
                path,
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Holidays</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{CACHED}"
                ),
            ));
        } else {
            out.push_str(&response(
                path,
                &format!(
                    "<D:resourcetype><D:collection/><CS:subscribed/></D:resourcetype>\
                     <CS:source><D:href>{source}</D:href></CS:source>"
                ),
            ));
        }
    }
    out
}

fn handle(state: &mut State, seen: Seen) -> Response<Body> {
    state.requests.push(seen.clone());
    let nc = state.nextcloud;
    let ok = |body: &str| reply(nc, 207, &multistatus(body));
    match (seen.method.as_str(), seen.path.as_str()) {
        ("OPTIONS", _) => reply(nc, 200, ""),
        ("PROPFIND", "/dav/") => ok(&response(
            "/dav/",
            "<D:current-user-principal><D:href>/dav/principals/alice/</D:href></D:current-user-principal>",
        )),
        ("PROPFIND", "/dav/principals/alice/") => ok(&response(
            "/dav/principals/alice/",
            "<C:calendar-home-set><D:href>/dav/cal/alice/</D:href></C:calendar-home-set>\
             <D:displayname>Alice</D:displayname>",
        )),
        ("PROPFIND", "/dav/cal/alice/") => ok(&home(state, seen.webcal)),
        ("PROPFIND", "/dav/cal/alice/trashbin/") if nc => ok(&response(
            "/dav/cal/alice/trashbin/",
            "<NC:trash-bin-retention-duration>2592000</NC:trash-bin-retention-duration>",
        )),
        ("REPORT", "/dav/cal/alice/trashbin/objects/") if nc => {
            let mut out = String::new();
            if state.trashed_event {
                out.push_str(&response(
                    "/dav/cal/alice/trashbin/objects/51.ics",
                    "<NC:deleted-at>2026-10-30T03:36:50+00:00</NC:deleted-at>\
                     <NC:calendar-uri>work</NC:calendar-uri><D:getetag>\"e1\"</D:getetag>\
                     <C:calendar-data>BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//EN\r\nBEGIN:VEVENT\r\n\
                     UID:lunch\r\nDTSTAMP:20261030T000000Z\r\nDTSTART:20261101T100000Z\r\n\
                     DTEND:20261101T110000Z\r\nSUMMARY:Lunch &amp; co\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n</C:calendar-data>",
                ));
            }
            ok(&out)
        }
        ("POST", "/dav/cal/alice/work/") if nc => {
            state.published = seen.body.contains("<CS:publish-calendar");
            reply(nc, 202, "")
        }
        ("MKCOL", path) if nc && path.starts_with("/dav/cal/alice/") => {
            let source = seen
                .body
                .split("<CS:source><D:href>")
                .nth(1)
                .and_then(|s| s.split("</D:href>").next())
                .unwrap_or_default()
                .to_owned();
            state.subscriptions.push((path.to_owned(), source));
            reply(nc, 201, "")
        }
        ("MOVE", "/dav/cal/alice/trashbin/objects/51.ics") if nc && state.trashed_event => {
            state.trashed_event = false;
            reply(nc, 201, "")
        }
        ("DELETE", "/dav/cal/alice/old/") if nc && state.old_calendar => {
            if seen.no_trashbin {
                state.old_calendar = false;
            }
            reply(nc, 204, "")
        }
        ("REPORT", _) => ok("<D:sync-token>t1</D:sync-token>"),
        _ => reply(nc, 404, ""),
    }
}

struct Fake {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Fake {
    async fn start(nextcloud: bool) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            nextcloud,
            published: false,
            subscriptions: vec![(
                "/dav/cal/alice/sub1/".to_owned(),
                "https://example.com/holidays.ics".to_owned(),
            )],
            old_calendar: true,
            trashed_event: true,
            requests: Vec::new(),
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
                                let header = |name: &str| {
                                    parts
                                        .headers
                                        .get(name)
                                        .and_then(|v| v.to_str().ok())
                                        .map(str::to_owned)
                                };
                                let seen = Seen {
                                    method: parts.method.as_str().to_owned(),
                                    path: parts.uri.path().to_owned(),
                                    body: String::from_utf8_lossy(&body).into_owned(),
                                    webcal: header("x-nc-caldav-webcal-caching").as_deref()
                                        == Some("On"),
                                    no_trashbin: header("x-nc-caldav-no-trashbin").as_deref()
                                        == Some("1"),
                                    destination: header("destination"),
                                };
                                let mut state = state.lock().unwrap();
                                if header("host").is_some_and(|h| h.starts_with("localhost")) {
                                    return Ok::<_, Infallible>(reply(state.nextcloud, 404, ""));
                                }
                                Ok::<_, Infallible>(handle(&mut state, seen))
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

    fn requests(&self) -> Vec<Seen> {
        self.state.lock().unwrap().requests.clone()
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
    store: Store,
    id: AccountId,
    account: Arc<GroupwareAccount>,
    _groupware: Groupware,
    _dir: tempfile::TempDir,
}

impl Setup {
    async fn collections(&self) -> Vec<Collection> {
        self.store
            .dav_collections(self.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.orphaned_at.is_none())
            .collect()
    }

    async fn named(&self, name: &str) -> Collection {
        self.collections()
            .await
            .into_iter()
            .find(|c| c.props.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no collection {name}"))
    }
}

async fn setup(nextcloud: bool) -> Setup {
    let fake = Fake::start(nextcloud).await;
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
    config.contacts = false;
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
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
    for _ in 0..500 {
        if !store.dav_collections(id).await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Setup {
        fake,
        store,
        id,
        account,
        _groupware: groupware,
        _dir: dir,
    }
}

#[tokio::test]
async fn nextcloud_publishing_subscriptions_and_trash() {
    let s = setup(true).await;
    let support = s.account.nextcloud_support();
    assert!(support.publishing && support.subscriptions && support.trash_bin);

    // The listing: the calendar, the subscription (read-only, with its
    // source), never the deleted calendar.
    let work = s.named("Work").await;
    assert!(work.props.extras.publishable);
    assert!(work.props.extras.publish_url.is_none());
    assert!(!work.props.extras.is_subscription);
    let holidays = s.named("Holidays").await;
    assert!(holidays.props.extras.is_subscription);
    assert!(!holidays.props.extras.publishable);
    assert_eq!(
        s.account
            .calendar_links(&holidays)
            .subscription_source
            .as_ref()
            .map(nebula_store::ExternalUrl::as_str),
        Some("https://example.com/holidays.ics")
    );
    // Stored only sealed, and a round with nothing new records nothing
    // (the sealed bytes are kept).
    let sealed = holidays.props.extras.subscription_source.clone().unwrap();
    assert!(
        !sealed
            .as_bytes()
            .windows(b"holidays.ics".len())
            .any(|w| w == b"holidays.ics")
    );
    let before = s.store.state(s.id, DataType::Calendar).await.unwrap();
    s.account.sync_round(false).await.unwrap();
    assert_eq!(
        s.store.state(s.id, DataType::Calendar).await.unwrap(),
        before
    );
    assert_eq!(
        s.named("Holidays").await.props.extras.subscription_source,
        Some(sealed)
    );
    let rights = holidays.props.rights.unwrap();
    assert!(rights.contains(Rights::READ) && rights.contains(Rights::UNBIND));
    assert!(!rights.contains(Rights::WRITE_CONTENT) && !rights.contains(Rights::BIND));
    assert_eq!(holidays.props.sharing.mode, None);
    assert!(
        s.collections()
            .await
            .iter()
            .all(|c| c.props.name.as_deref() != Some("Old"))
    );
    // The listing went with the header, the second one without it.
    let listings: Vec<Seen> = s
        .fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "PROPFIND" && r.path == "/dav/cal/alice/")
        .collect();
    assert!(listings.iter().any(|r| r.webcal));
    assert!(
        listings
            .iter()
            .any(|r| !r.webcal && r.body.contains("<CS:source/>"))
    );

    // Publishing: the link comes in the next listing.
    s.account.set_published(&work, true).await.unwrap();
    let post = s
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "POST")
        .unwrap();
    assert!(post.body.contains("<CS:publish-calendar "));
    let work = s.named("Work").await;
    assert_eq!(
        s.account
            .calendar_links(&work)
            .publish_url
            .as_ref()
            .map(nebula_store::ExternalUrl::as_str),
        Some("https://cloud.example.com/remote.php/dav/public-calendars/SECRET")
    );
    s.account.set_published(&work, false).await.unwrap();
    assert!(s.named("Work").await.props.extras.publish_url.is_none());
    // Someone else's or a subscription: not publishable.
    assert!(matches!(
        s.account.set_published(&holidays, true).await,
        Err(GroupwareError::Unsupported)
    ));

    // A new subscription: the server gets the source, the bridge never
    // asks for it.
    let source = ExternalUrl::subscription("webcal://example.com/team.ics?a=1&b=2").unwrap();
    let name = ResourceName::try_from("team-sub").unwrap();
    let made = s
        .account
        .subscribe(&name, &source, &[PropValue::DisplayName("Team".to_owned())])
        .await
        .unwrap();
    assert!(made.props.extras.is_subscription);
    let mkcol = s
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "MKCOL")
        .unwrap();
    assert_eq!(mkcol.path, "/dav/cal/alice/team-sub/");
    assert!(mkcol.body.contains("<CS:subscribed/>"));
    assert!(mkcol.body.contains(
        "<CS:source><D:href>webcal://example.com/team.ics?a=1&amp;b=2</D:href></CS:source>"
    ));
    assert!(
        s.fake
            .requests()
            .iter()
            .all(|r| !r.path.contains("team.ics") && !r.path.contains("holidays.ics"))
    );

    // The trash bin.
    let trash = s.account.trash().await.unwrap();
    assert_eq!(trash.retention_secs, Some(2_592_000));
    let [old] = trash.calendars.as_slice() else {
        panic!("one deleted calendar");
    };
    assert_eq!(old.name.as_deref(), Some("Old"));
    assert_eq!(old.deleted_at.map(UnixTime::secs), Some(1_793_179_897));
    let [lunch] = trash.events.as_slice() else {
        panic!("one deleted event");
    };
    assert_eq!(lunch.title.as_deref(), Some("Lunch & co"));
    assert_eq!(lunch.calendar, Some(work.id));
    assert_eq!(lunch.utc_start.map(UnixTime::secs), Some(1_793_527_200));

    let event = TrashItem {
        kind: TrashKind::Event,
        number: lunch.number,
    };
    let missing = TrashItem {
        kind: TrashKind::Calendar,
        number: lunch.number,
    };
    let results = s.account.restore_trashed(&[event, missing]).await.unwrap();
    assert!(results[0].is_ok());
    assert!(matches!(results[1], Err(GroupwareError::NotFound)));
    // Showing the bin asks for the events' data; finding what to restore
    // only for hrefs, so it never depends on how big the events are.
    let reports: Vec<bool> = s
        .fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "REPORT" && r.path == "/dav/cal/alice/trashbin/objects/")
        .map(|r| r.body.contains("calendar-data"))
        .collect();
    assert_eq!(reports, [true, false]);
    let moved = s
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "MOVE")
        .unwrap();
    assert_eq!(
        moved.destination.as_deref(),
        Some(
            format!(
                "http://127.0.0.1:{}/dav/cal/alice/trashbin/restore/51.ics",
                s.fake.port
            )
            .as_str()
        )
    );

    let calendar = TrashItem {
        kind: TrashKind::Calendar,
        number: old.number,
    };
    let results = s.account.delete_trashed(&[calendar]).await.unwrap();
    assert!(results[0].is_ok());
    let deleted = s
        .fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "DELETE")
        .unwrap();
    assert!(deleted.no_trashbin);
    let trash = s.account.trash().await.unwrap();
    assert!(trash.calendars.is_empty() && trash.events.is_empty());
}

#[tokio::test]
async fn elsewhere_nothing_and_no_nextcloud_headers() {
    let s = setup(false).await;
    assert_eq!(
        s.account.nextcloud_support(),
        nebula_groupware::NextcloudSupport::default()
    );
    let work = s.named("Work").await;
    assert!(!work.props.extras.publishable);
    assert!(matches!(
        s.account.set_published(&work, true).await,
        Err(GroupwareError::Unsupported)
    ));
    assert!(matches!(
        s.account.trash().await,
        Err(GroupwareError::Unsupported)
    ));
    let source = ExternalUrl::subscription("https://example.com/x.ics").unwrap();
    let name = ResourceName::try_from("x").unwrap();
    assert!(matches!(
        s.account.subscribe(&name, &source, &[]).await,
        Err(GroupwareError::Unsupported)
    ));
    let requests = s.fake.requests();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|r| !r.webcal && !r.no_trashbin));
    assert!(
        requests
            .iter()
            .all(|r| r.method != "POST" && r.method != "MKCOL")
    );
}
