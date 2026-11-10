// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The DAV client against hostile servers (fake ones, with hyper): the
//! `fetch` vectors of `security/ssrf-vectors.json`
//! and the DAV attacks (DTDs, nesting, huge or slow
//! bodies, gzip bombs, redirects elsewhere, 412, 507, expired tokens,
//! missing etags).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::fmt::Write as _;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use flate2::Compression;
use flate2::write::GzEncoder;
use futures::stream;
use http::Response;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use nebula_dav::proto::{
    Ace, CalendarAddress, CalendarUserType, ETag, FreeBusyRequest, Href, MAX_FREEBUSY_ATTENDEES,
    OcPrincipal, ParseError, Privilege, Property, ResourceName, ShareAccess, Sharee, SyncToken,
};
use nebula_dav::proto::{Status, VcardRequest};
use nebula_dav::{
    AllowedOrigin, Credentials, DavClient, DavError, DavServer, Limits, MAX_FREEBUSY_RESPONSE,
    Origin, PutCondition, RedirectError, ResourceKind, TrashContents,
};
use net_guard::{BlockReason, DnsError, DomainName, LookupFuture, NetworkAccess, Resolver};
use secrecy::SecretString;
use serde::Deserialize;
use tokio::net::TcpListener;

type Body = BoxBody<Bytes, Infallible>;

/// What a fake server saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    accept: Option<String>,
    body: String,
}

/// What a fake server answers: maybe after a pause.
struct Script {
    delay: Option<Duration>,
    response: Response<Body>,
}

impl From<Response<Body>> for Script {
    fn from(response: Response<Body>) -> Self {
        Self {
            delay: None,
            response,
        }
    }
}

type Handler = dyn Fn(&Seen, usize) -> Script + Send + Sync;

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Connections accepted.
    connections: Arc<AtomicUsize>,
}

impl Fake {
    async fn start(handler: impl Fn(&Seen, usize) -> Script + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let log = Arc::clone(&seen);
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::clone(&connections);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let handler = Arc::clone(&handler);
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let handler = Arc::clone(&handler);
                            let log = Arc::clone(&log);
                            async move {
                                let (parts, body) = request.into_parts();
                                // The client here only uses 127.0.0.1. A
                                // `localhost` request comes from outside the
                                // test (e.g. an IDE that probes every new
                                // listening port): not recorded or counted.
                                if parts
                                    .headers
                                    .get("host")
                                    .and_then(|v| v.to_str().ok())
                                    .is_some_and(|h| h.split(':').next() == Some("localhost"))
                                {
                                    return Ok::<_, Infallible>(empty(404));
                                }
                                let body = body
                                    .collect()
                                    .await
                                    .map(http_body_util::Collected::to_bytes)
                                    .unwrap_or_default();
                                let seen = Seen {
                                    method: parts.method.to_string(),
                                    path: parts.uri.path().to_owned(),
                                    authorization: parts
                                        .headers
                                        .get("authorization")
                                        .map(|v| v.to_str().unwrap().to_owned()),
                                    accept: parts
                                        .headers
                                        .get("accept")
                                        .map(|v| v.to_str().unwrap().to_owned()),
                                    body: String::from_utf8_lossy(&body).into_owned(),
                                };
                                let index = {
                                    let mut log = log.lock().unwrap();
                                    log.push(seen.clone());
                                    log.len() - 1
                                };
                                let script = handler(&seen, index);
                                if let Some(delay) = script.delay {
                                    tokio::time::sleep(delay).await;
                                }
                                Ok::<_, Infallible>(script.response)
                            }
                        });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self {
            port,
            seen,
            connections,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).boxed()
}

fn xml(status: u16, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .body(full(body.to_owned()))
        .unwrap()
}

fn redirect(status: u16, location: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("location", location)
        .body(full(""))
        .unwrap()
}

fn empty(status: u16) -> Response<Body> {
    Response::builder().status(status).body(full("")).unwrap()
}

/// A body sent in pieces with a pause before each one.
fn trickle(content_type: &str, pieces: Vec<Bytes>, pause: Duration) -> Response<Body> {
    let frames = stream::unfold(pieces.into_iter(), move |mut rest| async move {
        let piece = rest.next()?;
        tokio::time::sleep(pause).await;
        Some((Ok::<_, Infallible>(Frame::data(piece)), rest))
    });
    Response::builder()
        .status(207)
        .header("content-type", content_type)
        .body(BodyExt::boxed(StreamBody::new(frames)))
        .unwrap()
}

/// A fake DNS: these names, these addresses; anything else fails.
struct Dns(HashMap<String, Vec<IpAddr>>);

impl Dns {
    fn new(entries: &[(&str, &str)]) -> Arc<Self> {
        let mut map: HashMap<String, Vec<IpAddr>> = HashMap::new();
        for (name, ip) in entries {
            map.entry((*name).to_owned())
                .or_default()
                .push(ip.parse().unwrap());
        }
        Arc::new(Self(map))
    }
}

impl Resolver for Dns {
    fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async move { self.0.get(name.as_str()).cloned().ok_or(DnsError) })
    }
}

const PASSWORD: &str = "s3cret-password";

fn fast_limits() -> Limits {
    let mut limits = Limits::default();
    limits.request_timeout = Duration::from_millis(1500);
    limits.idle_timeout = Duration::from_millis(400);
    limits.max_multistatus = 64 * 1024;
    limits.max_icalendar = 16 * 1024;
    limits
}

fn config(fake: &Fake) -> DavServer {
    DavServer::new(&fake.url("/dav/"))
        .unwrap()
        .with_private_network(true)
        .with_limits(fast_limits())
}

fn client_for(server: DavServer, dns: Arc<Dns>) -> DavClient {
    let credentials = Credentials::new("alice", SecretString::from(PASSWORD)).unwrap();
    DavClient::with_resolver(server, Some(credentials), dns).unwrap()
}

fn client(fake: &Fake) -> DavClient {
    client_for(config(fake), Dns::new(&[]))
}

fn href(client: &DavClient, path: &str) -> Href {
    Href::resolve(path, &client.server().base).unwrap()
}

fn allowed(origin: &str, network: NetworkAccess) -> AllowedOrigin {
    AllowedOrigin {
        origin: Origin::parse(origin).unwrap(),
        network,
    }
}

const EMPTY_MULTISTATUS: &str = r#"<?xml version="1.0"?><multistatus xmlns="DAV:"/>"#;

fn principal_answer(principal: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/dav/</href><propstat><prop><current-user-principal><href>{principal}</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#
    )
}

fn expect_reason(result: Result<impl std::fmt::Debug, DavError>, reason: BlockReason) {
    match result {
        Err(error) => assert_eq!(error.block_reason(), Some(reason), "{error:?}"),
        Ok(value) => panic!("expected {reason}, got {value:?}"),
    }
}

// --- security/ssrf-vectors.json, `fetch` ------------------------------

/// `redirect-to-private`: a redirect to a private address is never
/// followed. To a host that is not allowed, nothing is even resolved; to
/// an allowed name that resolves to a private address (or to link-local,
/// even with `private_network`), net-guard refuses it.
#[tokio::test]
async fn redirect_to_private() {
    let target = Fake::start(|_, _| xml(207, EMPTY_MULTISTATUS).into()).await;
    let target_port = target.port;
    let fake = Fake::start(move |seen, _| {
        match seen.path.as_str() {
            "/dav/a" => redirect(302, "http://10.0.0.1/"),
            "/dav/b" => redirect(302, &format!("http://internal.test:{target_port}/")),
            "/dav/c" => redirect(307, "http://169.254.169.254/latest/meta-data/"),
            _ => empty(404),
        }
        .into()
    })
    .await;

    let plain = client(&fake);
    let result = plain.options(&href(&plain, "/dav/a")).await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );

    let server = config(&fake)
        .with_allowed_origin(allowed(
            &format!("http://internal.test:{target_port}"),
            NetworkAccess::PublicOnly,
        ))
        .with_allowed_origin(allowed(
            "http://169.254.169.254",
            NetworkAccess::PrivateAllowed,
        ));
    let dns = Dns::new(&[("internal.test", "127.0.0.1")]);
    let client = client_for(server, dns);
    expect_reason(
        client.options(&href(&client, "/dav/b")).await,
        BlockReason::PrivateIp,
    );
    expect_reason(
        client.options(&href(&client, "/dav/c")).await,
        BlockReason::PrivateIp,
    );
    assert!(target.seen().is_empty(), "the private target was reached");
}

/// `redirect-to-metadata-name`: a name of the cloud metadata service,
/// even if the admin allowed it, is refused: by name (`.internal`, `dns`)
/// when public only, before resolving.
#[tokio::test]
async fn redirect_to_metadata_name() {
    let fake = Fake::start(|_, _| {
        redirect(301, "http://metadata.google.internal/computeMetadata/v1/").into()
    })
    .await;
    let plain = client(&fake);
    let result = plain.options(&href(&plain, "/dav/")).await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );

    let server = config(&fake).with_allowed_origin(allowed(
        "http://metadata.google.internal",
        NetworkAccess::PublicOnly,
    ));
    let client = client_for(
        server,
        Dns::new(&[("metadata.google.internal", "169.254.169.254")]),
    );
    expect_reason(
        client.options(&href(&client, "/dav/")).await,
        BlockReason::Dns,
    );
}

/// `redirect-scheme-change`: `file:` and `gopher:` are refused.
#[tokio::test]
async fn redirect_scheme_change() {
    let fake = Fake::start(|seen, _| {
        match seen.path.as_str() {
            "/dav/file" => redirect(302, "file:///etc/passwd"),
            "/dav/gopher" => redirect(307, "gopher://127.0.0.1:6379/_FLUSHALL"),
            _ => redirect(308, "javascript:alert(1)"),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    for path in ["/dav/file", "/dav/gopher", "/dav/js"] {
        expect_reason(
            client.options(&href(&client, path)).await,
            BlockReason::Scheme,
        );
    }
}

/// `redirect-loop`: at most 5 redirects are followed.
#[tokio::test]
async fn redirect_loop() {
    let fake = Fake::start(|_, index| redirect(302, &format!("/dav/loop/{index}")).into()).await;
    let looping = client(&fake);
    let result = looping.options(&href(&looping, "/dav/")).await;
    assert!(matches!(
        result,
        Err(DavError::Redirect(RedirectError::TooMany))
    ));
    expect_reason(result, BlockReason::TooManyRedirects);
    assert_eq!(fake.seen().len(), 6, "the request and 5 redirects");

    // Five are fine.
    let five = Fake::start(|_, index| {
        if index < 5 {
            redirect(307, &format!("/dav/hop/{index}/")).into()
        } else {
            xml(207, &principal_answer("/dav/principal/")).into()
        }
    })
    .await;
    let client = client(&five);
    client
        .options(&href(&client, "/dav/"))
        .await
        .unwrap_or_default();
    assert_eq!(five.seen().len(), 6);
}

/// `not-an-image`, for DAV: a multistatus that is HTML, and an event that
/// is not `text/calendar`, are refused (`content-type`).
#[tokio::test]
async fn wrong_content_type() {
    let fake = Fake::start(|seen, _| {
        let content_type = if seen.method == "GET" {
            "text/html"
        } else {
            "text/html; charset=utf-8"
        };
        Response::builder()
            .status(if seen.method == "GET" { 200 } else { 207 })
            .header("content-type", content_type)
            .body(full(EMPTY_MULTISTATUS))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    expect_reason(
        client.list_members(&href(&client, "/dav/cal/")).await,
        BlockReason::ContentType,
    );
    expect_reason(
        client
            .get(&href(&client, "/dav/cal/1.ics"), ResourceKind::Calendar)
            .await,
        BlockReason::ContentType,
    );
}

/// `too-large`: with `Content-Length` (refused before reading) and
/// without (stopped at the limit).
#[tokio::test]
async fn too_large() {
    let big = format!(
        r#"<multistatus xmlns="DAV:">{}</multistatus>"#,
        "<response><href>/dav/cal/x</href><status>HTTP/1.1 404 x</status></response>".repeat(2000)
    );
    let chunks: Vec<Bytes> = big
        .as_bytes()
        .chunks(8 * 1024)
        .map(Bytes::copy_from_slice)
        .collect();
    let fake = Fake::start(move |seen, _| {
        if seen.path.ends_with("/length/") {
            xml(207, &big)
        } else {
            trickle("application/xml", chunks.clone(), Duration::ZERO)
        }
        .into()
    })
    .await;
    let client = client(&fake);
    for path in ["/dav/length/", "/dav/chunked/"] {
        expect_reason(
            client.list_members(&href(&client, path)).await,
            BlockReason::Size,
        );
    }
    // A GET over the iCalendar limit too.
    let event = Fake::start(|_, _| {
        Response::builder()
            .header("content-type", "text/calendar")
            .body(full(vec![b'A'; 20 * 1024]))
            .unwrap()
            .into()
    })
    .await;
    let client = self::client(&event);
    expect_reason(
        client
            .get(&href(&client, "/dav/cal/1.ics"), ResourceKind::Calendar)
            .await,
        BlockReason::Size,
    );
}

/// `gzip-bomb`: we ask for `identity`; a server that compresses anyway is
/// decoded with the same limit.
#[tokio::test]
async fn gzip_bomb() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(br#"<multistatus xmlns="DAV:">"#).unwrap();
    encoder.write_all(&vec![b' '; 50 * 1024 * 1024]).unwrap();
    encoder.write_all(b"</multistatus>").unwrap();
    let bomb = Bytes::from(encoder.finish().unwrap());
    assert!(bomb.len() < 100 * 1024);
    let mut small = GzEncoder::new(Vec::new(), Compression::default());
    small.write_all(EMPTY_MULTISTATUS.as_bytes()).unwrap();
    let small = Bytes::from(small.finish().unwrap());
    let fake = Fake::start(move |seen, _| {
        assert_eq!(seen.method, "PROPFIND");
        let body = if seen.path.ends_with("/bomb/") {
            bomb.clone()
        } else {
            small.clone()
        };
        Response::builder()
            .status(207)
            .header("content-type", "application/xml")
            .header("content-encoding", "gzip")
            .body(full(body))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    expect_reason(
        client.list_members(&href(&client, "/dav/bomb/")).await,
        BlockReason::Size,
    );
    // A small compressed answer is decoded.
    assert!(
        client
            .list_members(&href(&client, "/dav/ok/"))
            .await
            .unwrap()
            .is_empty()
    );
}

/// `slow-loris`: a body that trickles in is cut at the request's deadline;
/// a pause longer than the idle limit too; and headers that never come.
#[tokio::test]
async fn slow_loris() {
    let body: Vec<Bytes> = EMPTY_MULTISTATUS
        .bytes()
        .map(|b| Bytes::copy_from_slice(&[b]))
        .collect();
    let slow = body.clone();
    let fake = Fake::start(move |seen, _| match seen.path.as_str() {
        // One byte every 100 ms: never idle, but past the deadline.
        "/dav/trickle/" => {
            trickle("application/xml", slow.clone(), Duration::from_millis(100)).into()
        }
        // A pause longer than the idle limit.
        "/dav/pause/" => trickle(
            "application/xml",
            vec![
                Bytes::from_static(b"<multistatus"),
                Bytes::from_static(b" xmlns=\"DAV:\"/>"),
            ],
            Duration::from_millis(900),
        )
        .into(),
        // The headers never come.
        _ => Script {
            delay: Some(Duration::from_secs(30)),
            response: xml(207, EMPTY_MULTISTATUS),
        },
    })
    .await;
    let client = client(&fake);
    for path in ["/dav/trickle/", "/dav/pause/", "/dav/silent/"] {
        let started = tokio::time::Instant::now();
        expect_reason(
            client.list_members(&href(&client, path)).await,
            BlockReason::Timeout,
        );
        assert!(started.elapsed() < Duration::from_secs(3), "{path}");
    }
}

/// `env-proxy-ignored` and `rebinding`: covered by construction, see
/// `vectors_are_all_accounted_for`.
const COVERED_ELSEWHERE: &[(&str, &str)] = &[
    (
        "rebinding",
        "net-guard's rebinding_vector: the connection uses the address validated once",
    ),
    (
        "env-proxy-ignored",
        "hyper::client::conn reads no proxy variables and net-guard dials a SocketAddr; \
         setting HTTP_PROXY in a test needs unsafe (Rust 2024)",
    ),
];

const RUN_HERE: &[&str] = &[
    "redirect-to-private",
    "redirect-to-metadata-name",
    "redirect-scheme-change",
    "redirect-loop",
    "not-an-image",
    "too-large",
    "gzip-bomb",
    "slow-loris",
];

#[derive(Deserialize)]
struct Vectors {
    fetch: Vec<FetchVector>,
}

#[derive(Deserialize)]
struct FetchVector {
    id: String,
    reason: Option<String>,
}

/// Every `fetch` vector runs here (with its reason, see `expect_reason`)
/// or is accounted for, so a new vector cannot be forgotten.
#[test]
fn vectors_are_all_accounted_for() {
    let text = include_str!("../../../../security/ssrf-vectors.json");
    let vectors: Vectors = serde_json::from_str(text).unwrap();
    let known: HashSet<&str> = RUN_HERE
        .iter()
        .copied()
        .chain(COVERED_ELSEWHERE.iter().map(|(id, _)| *id))
        .collect();
    for vector in &vectors.fetch {
        assert!(
            known.contains(vector.id.as_str()),
            "fetch vector {} is not run",
            vector.id
        );
        let expected = match vector.id.as_str() {
            "redirect-to-private" => Some(BlockReason::PrivateIp),
            "redirect-to-metadata-name" => Some(BlockReason::Dns),
            "redirect-scheme-change" => Some(BlockReason::Scheme),
            "redirect-loop" => Some(BlockReason::TooManyRedirects),
            "not-an-image" => Some(BlockReason::ContentType),
            "too-large" | "gzip-bomb" => Some(BlockReason::Size),
            "slow-loris" => Some(BlockReason::Timeout),
            _ => None,
        };
        assert_eq!(
            expected.map(BlockReason::as_str),
            vector.reason.as_deref(),
            "{}",
            vector.id
        );
    }
    assert_eq!(known.len(), vectors.fetch.len());
}

// --- Credentials -----------------------------------------------------

/// `set_credentials` changes the login of the next request (a new
/// password after logging in again), and `None` sends none.
#[tokio::test]
async fn credentials_can_be_changed_in_place() {
    let fake = Fake::start(|_, _| xml(207, EMPTY_MULTISTATUS).into()).await;
    let client = client(&fake);
    let collection = href(&client, "/dav/cal/work/");
    client.list_members(&collection).await.unwrap();
    client.set_credentials(Some(
        Credentials::new("alice", SecretString::from("new password")).unwrap(),
    ));
    client.list_members(&collection).await.unwrap();
    client.set_credentials(None);
    client.list_members(&collection).await.unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 3);
    let old = format!("Basic {}", STANDARD.encode(format!("alice:{PASSWORD}")));
    let new = format!("Basic {}", STANDARD.encode("alice:new password"));
    assert_eq!(seen[0].authorization.as_deref(), Some(old.as_str()));
    assert_eq!(seen[1].authorization.as_deref(), Some(new.as_str()));
    assert_eq!(seen[2].authorization, None);
    // Never printed.
    assert!(!format!("{client:?}").contains("new password"));
}

/// `Authorization` goes to the server's own origin (also after a redirect
/// on it), never to another origin, even an allowed one.
#[tokio::test]
async fn credentials_never_leave_the_origin() {
    let other = Fake::start(|_, _| xml(207, EMPTY_MULTISTATUS).into()).await;
    let other_url = other.url("/elsewhere/");
    let fake = Fake::start(move |seen, _| {
        match seen.path.as_str() {
            "/dav/same/" => redirect(301, "/dav/moved/"),
            "/dav/other/" => redirect(307, &other_url),
            _ => xml(207, EMPTY_MULTISTATUS),
        }
        .into()
    })
    .await;
    let server = config(&fake).with_allowed_origin(allowed(
        &format!("http://127.0.0.1:{}", other.port),
        NetworkAccess::PrivateAllowed,
    ));
    let client = client_for(server, Dns::new(&[]));
    // A home listing follows redirects (a member listing would refuse
    // one: its hrefs would all look outside).
    client
        .list_collections(&href(&client, "/dav/same/"))
        .await
        .unwrap();
    client
        .list_collections(&href(&client, "/dav/other/"))
        .await
        .unwrap();

    let expected = format!("Basic {}", STANDARD.encode(format!("alice:{PASSWORD}")));
    let seen = fake.seen();
    assert_eq!(seen.len(), 3);
    assert!(
        seen.iter()
            .all(|s| s.authorization.as_deref() == Some(expected.as_str()))
    );
    assert_eq!(seen[1].path, "/dav/moved/");
    let elsewhere = other.seen();
    assert_eq!(elsewhere.len(), 1);
    assert_eq!(elsewhere[0].path, "/elsewhere/");
    assert_eq!(elsewhere[0].authorization, None);
    // The request was repeated as it was (307).
    assert_eq!(elsewhere[0].method, "PROPFIND");
    assert!(elsewhere[0].body.contains("resourcetype"));
}

#[tokio::test]
async fn unauthorized_is_typed() {
    let fake = Fake::start(|_, _| {
        Response::builder()
            .status(401)
            .header("www-authenticate", "Basic realm=\"x\"")
            .body(full("no"))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    assert!(matches!(
        client.discover().await,
        Err(DavError::Unauthorized)
    ));
}

// --- XML -------------------------------------------------------------

#[tokio::test]
async fn dtds_and_entities_are_refused() {
    let xxe = r#"<?xml version="1.0"?><!DOCTYPE m [<!ENTITY x SYSTEM "file:///etc/passwd">]><multistatus xmlns="DAV:"><response><href>/dav/cal/&x;</href></response></multistatus>"#;
    let laughs = concat!(
        r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol">"#,
        r#"<!ENTITY lol1 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">"#,
        r#"<!ENTITY lol2 "&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;">"#,
        r#"]><multistatus xmlns="DAV:">&lol2;</multistatus>"#
    );
    let empty_dtd = r#"<!DOCTYPE multistatus><multistatus xmlns="DAV:"/>"#;
    let bodies = [xxe, laughs, empty_dtd];
    let fake = Fake::start(move |_, index| xml(207, bodies[index % 3]).into()).await;
    let client = client(&fake);
    for _ in 0..3 {
        let result = client.list_members(&href(&client, "/dav/cal/")).await;
        assert!(
            matches!(result, Err(DavError::Xml(ParseError::Dtd))),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn deep_nesting_is_refused() {
    let deep = format!(
        r#"<multistatus xmlns="DAV:">{}{}</multistatus>"#,
        "<x>".repeat(5000),
        "</x>".repeat(5000)
    );
    let fake = Fake::start(move |_, _| xml(207, &deep).into()).await;
    let client = client(&fake);
    let result = client.list_members(&href(&client, "/dav/cal/")).await;
    assert!(
        matches!(result, Err(DavError::Xml(ParseError::TooDeep))),
        "{result:?}"
    );
}

#[tokio::test]
async fn huge_headers_are_refused() {
    let fake = Fake::start(|_, _| {
        Response::builder()
            .status(207)
            .header("x-padding", "a".repeat(100 * 1024))
            .body(full(EMPTY_MULTISTATUS))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    let result = client.list_members(&href(&client, "/dav/cal/")).await;
    // hyper refuses it while reading, or our own check right after.
    assert!(
        matches!(result, Err(DavError::Http(_) | DavError::HeadTooLarge)),
        "{result:?}"
    );
}

/// Discovery only follows hrefs on allowed origins, and ignores homes on
/// other ones. Collections outside the home are kept only on its origin,
/// marked as such (the groupware keeps only those of someone else).
#[tokio::test]
async fn hostile_hrefs() {
    let fake = Fake::start(|seen, _| {
        let body = match seen.path.as_str() {
            "/dav/" if seen.body.contains("current-user-principal") => principal_answer("/dav/principals/alice/"),
            "/dav/principals/alice/" => r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:R="urn:ietf:params:xml:ns:carddav"><response><href>/dav/principals/alice/</href><propstat><prop>
                <C:calendar-home-set><href>https://evil.example/cal/</href></C:calendar-home-set>
                <R:addressbook-home-set><href>/dav/card/alice/</href></R:addressbook-home-set>
                </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#.to_owned(),
            "/dav/card/alice/" => r#"<multistatus xmlns="DAV:" xmlns:R="urn:ietf:params:xml:ns:carddav">
                <response><href>/dav/card/alice/</href><propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
                <response><href>/dav/card/alice/book/</href><propstat><prop><resourcetype><collection/><R:addressbook/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
                <response><href>/dav/card/bob/book/</href><propstat><prop><resourcetype><collection/><R:addressbook/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
                <response><href>https://evil.example/dav/card/alice/x/</href><propstat><prop><resourcetype><collection/><R:addressbook/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
                <response><href>/dav/card/alice/a%5Cb/</href><propstat><prop><resourcetype><collection/><R:addressbook/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
                </multistatus>"#.to_owned(),
            "/dav/card/alice/dots/" => r#"<multistatus xmlns="DAV:"><response><href>/dav/card/alice/dots/%2e%2e/%2e%2e/bob/x.vcf</href><status>HTTP/1.1 200 OK</status></response></multistatus>"#.to_owned(),
            _ => return empty(404).into(),
        };
        if seen.method == "OPTIONS" {
            return empty(200).into();
        }
        xml(207, &body).into()
    })
    .await;
    let client = client(&fake);
    let found = client.discover().await.unwrap();
    assert!(found.calendar_homes.is_empty(), "the evil home was dropped");
    assert_eq!(found.addressbook_homes.len(), 1);
    let books = client
        .list_collections(&found.addressbook_homes[0])
        .await
        .unwrap();
    // Alice's direct member, and one elsewhere on the same origin (as
    // Stalwart lists what others share), marked; never the other host.
    assert_eq!(books.len(), 2, "{books:?}");
    assert_eq!(books[0].href.path(), "/dav/card/alice/book/");
    assert!(books[0].in_home);
    assert_eq!(books[1].href.path(), "/dav/card/bob/book/");
    assert!(!books[1].in_home);
    let dots = client
        .list_members(&href(&client, "/dav/card/alice/dots/"))
        .await;
    // The member with dot segments is dropped (never used), the listing
    // still works.
    assert!(dots.unwrap().is_empty());
    // Requests to another origin are refused before connecting.
    let evil = Href::resolve("https://evil.example/cal/", &client.server().base).unwrap();
    assert!(matches!(
        client.list_collections(&evil).await,
        Err(DavError::OriginNotAllowed)
    ));
}

// --- Conditional writes and sync -------------------------------------

#[tokio::test]
async fn precondition_failed_and_missing_etag() {
    let fake = Fake::start(|seen, _| {
        if seen.method == "PUT" && seen.path.ends_with("taken.ics") {
            return empty(412).into();
        }
        // A server that gives no ETag after PUT (libdav: "some servers
        // don't return etags").
        empty(201).into()
    })
    .await;
    let client = client(&fake);
    let collection = href(&client, "/dav/cal/work/");
    let taken = collection
        .member(&ResourceName::try_from("taken.ics").unwrap(), false)
        .unwrap();
    let event = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
    let result = client
        .put(&taken, ResourceKind::Calendar, event, &PutCondition::Create)
        .await;
    assert!(
        matches!(result, Err(DavError::PreconditionFailed)),
        "{result:?}"
    );
    let new = collection
        .member(&ResourceName::try_from("new.ics").unwrap(), false)
        .unwrap();
    let etag = client
        .put(&new, ResourceKind::Calendar, event, &PutCondition::Create)
        .await
        .unwrap();
    assert_eq!(etag, None);
    let seen = fake.seen();
    assert!(seen.iter().all(|s| s.method == "PUT"));
    // Too big to send: refused locally.
    let big = "A".repeat(fast_limits().max_icalendar + 1);
    let result = client
        .put(&new, ResourceKind::Calendar, &big, &PutCondition::Create)
        .await;
    assert!(matches!(result, Err(DavError::TooLarge)));
    assert_eq!(fake.seen().len(), 2);
}

/// vCard 4.0 is asked for only when the caller says so (a book that lists
/// it): `address-data` with its type and version in the multiget,
/// `version=4.0` in the GET's `Accept`. Etags and hrefs come back the same
/// way; a calendar ignores the request.
#[tokio::test]
async fn vcard4_is_asked_for_only_when_wanted() {
    const CARD: &str = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:g\r\nFN:G\r\nKIND:group\r\nEND:VCARD\r\n";
    let fake = Fake::start(|seen, _| {
        if seen.method == "GET" {
            return Response::builder()
                .status(200)
                .header("content-type", "text/vcard; version=4.0; charset=utf-8")
                .header("etag", "\"e1\"")
                .body(full(CARD))
                .unwrap()
                .into();
        }
        xml(
            207,
            &format!(
                r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:r="urn:ietf:params:xml:ns:carddav" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/dav/card/book/g.vcf</d:href><d:propstat><d:prop><d:getetag>"e1"</d:getetag><r:address-data>{CARD}</r:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
            ),
        )
        .into()
    })
    .await;
    let client = client(&fake);
    let book = href(&client, "/dav/card/book/");
    let card = book
        .member(&ResourceName::try_from("g.vcf").unwrap(), false)
        .unwrap();
    for wanted in [VcardRequest::AsStored, VcardRequest::Version4] {
        let fetched = client
            .multiget_as(
                &book,
                ResourceKind::Contact,
                std::slice::from_ref(&card),
                wanted,
            )
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].href, card);
        assert_eq!(fetched[0].etag.as_ref().map(ETag::as_str), Some("\"e1\""));
        assert!(fetched[0].data.as_deref().unwrap().contains("KIND:group"));
        let got = client
            .get_as(&card, ResourceKind::Contact, wanted)
            .await
            .unwrap();
        assert_eq!(got.etag.as_ref().map(ETag::as_str), Some("\"e1\""));
    }
    let plain = client
        .multiget(&book, ResourceKind::Contact, std::slice::from_ref(&card))
        .await
        .unwrap();
    assert_eq!(plain.len(), 1);
    client.get(&card, ResourceKind::Contact).await.unwrap();
    // A calendar never asks for a version.
    let calendar = href(&client, "/dav/cal/work/");
    let _ = client
        .multiget_as(
            &calendar,
            ResourceKind::Calendar,
            &[],
            VcardRequest::Version4,
        )
        .await;

    let seen = fake.seen();
    let reports: Vec<&Seen> = seen.iter().filter(|s| s.method == "REPORT").collect();
    let gets: Vec<&Seen> = seen.iter().filter(|s| s.method == "GET").collect();
    assert_eq!((reports.len(), gets.len()), (3, 3), "{seen:?}");
    assert!(
        reports[0].body.contains("<CR:address-data/>"),
        "{}",
        reports[0].body
    );
    assert!(
        !reports[0].body.contains(r#"version="4.0""#),
        "{}",
        reports[0].body
    );
    assert!(
        reports[1]
            .body
            .contains(r#"<D:prop><D:getetag/><CR:address-data content-type="text/vcard" version="4.0"/></D:prop><D:href>/dav/card/book/g.vcf</D:href>"#),
        "{}",
        reports[1].body
    );
    assert_eq!(reports[2].body, reports[0].body);
    assert_eq!(
        gets[0].accept.as_deref(),
        Some("text/vcard, text/x-vcard;q=0.5")
    );
    assert_eq!(
        gets[1].accept.as_deref(),
        Some("text/vcard; version=4.0, text/vcard;q=0.9, text/x-vcard;q=0.5")
    );
    assert_eq!(gets[2].accept, gets[0].accept);
    // An empty calendar multiget sends nothing at all.
    assert!(seen.iter().all(|s| !s.path.starts_with("/dav/cal/")));
}

fn sync_answer(entries: &str, token: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">{entries}<d:sync-token>{token}</d:sync-token></d:multistatus>"#
    )
}

fn changed(path: &str, etag: Option<&str>) -> String {
    let etag = etag.map_or_else(String::new, |etag| format!("<d:getetag>{etag}</d:getetag>"));
    format!(
        "<d:response><d:href>{path}</d:href><d:propstat><d:prop>{etag}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

fn removed(path: &str) -> String {
    format!(
        "<d:response><d:href>{path}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
    )
}

/// RFC 6578 §3.6: a 507 on the collection means "more to come with the
/// new token"; the client asks again and joins the pages.
#[tokio::test]
async fn sync_continues_after_507() {
    let fake = Fake::start(|seen, _| {
        let body = if seen.body.contains("<D:sync-token>t1</D:sync-token>") {
            sync_answer(&format!("{}{}", changed("/dav/cal/work/2.ics", Some("\"e2\"")), removed("/dav/cal/work/3.ics")), "t2")
        } else {
            sync_answer(
                &format!(
                    "{}{}{}",
                    changed("/dav/cal/work/1.ics", Some("\"e1\"")),
                    changed("/dav/cal/work/2.ics", Some("\"old\"")),
                    "<d:response><d:href>/dav/cal/work/</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>"
                ),
                "t1",
            )
        };
        xml(207, &body).into()
    })
    .await;
    let client = client(&fake);
    let result = client
        .sync(&href(&client, "/dav/cal/work/"), None)
        .await
        .unwrap();
    assert_eq!(result.token.as_str(), "t2");
    assert!(result.full);
    let changed: Vec<(&str, &str)> = result
        .changed
        .iter()
        .map(|m| (m.href.path(), m.etag.as_ref().map_or("", ETag::as_str)))
        .collect();
    assert_eq!(
        changed,
        [
            ("/dav/cal/work/1.ics", "\"e1\""),
            ("/dav/cal/work/2.ics", "\"e2\"")
        ]
    );
    assert_eq!(result.removed.len(), 1);
    let seen = fake.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|s| s.method == "REPORT"));
    assert!(seen[0].body.contains("<D:sync-token></D:sync-token>"));
}

/// An expired token (`DAV:valid-sync-token`, RFC 6578 §3.2) starts over
/// once with no token, and the result says it is complete.
#[tokio::test]
async fn expired_token_starts_over() {
    let fake = Fake::start(|seen, _| {
        if seen.body.contains("<D:sync-token>old</D:sync-token>") {
            return xml(
                403,
                r#"<?xml version="1.0"?><D:error xmlns:D="DAV:"><D:valid-sync-token/></D:error>"#,
            )
            .into();
        }
        // Missing ETag: the member is there, its etag is unknown.
        xml(
            207,
            &sync_answer(&changed("/dav/cal/work/1.ics", None), "fresh"),
        )
        .into()
    })
    .await;
    let client = client(&fake);
    let old = SyncToken::parse("old").unwrap();
    let result = client
        .sync(&href(&client, "/dav/cal/work/"), Some(&old))
        .await
        .unwrap();
    assert!(result.full);
    assert_eq!(result.token.as_str(), "fresh");
    assert_eq!(result.changed.len(), 1);
    assert_eq!(result.changed[0].etag, None);
    assert_eq!(fake.seen().len(), 2);

    // Another 403 is not a token problem: no retry.
    let forbidden = Fake::start(|_, _| empty(403).into()).await;
    let client = self::client(&forbidden);
    let result = client
        .sync(&href(&client, "/dav/cal/work/"), Some(&old))
        .await;
    assert!(
        matches!(result, Err(DavError::Forbidden { precondition: None })),
        "{result:?}"
    );
    assert_eq!(forbidden.seen().len(), 1);
}

/// A sync answer naming resources outside the collection: ignored.
#[tokio::test]
async fn sync_ignores_other_collections() {
    let fake = Fake::start(|_, _| {
        let entries = format!(
            "{}{}{}",
            changed("/dav/cal/work/1.ics", Some("\"a\"")),
            changed("/dav/cal/bob/1.ics", Some("\"b\"")),
            removed("/dav/cal/work/sub/2.ics"),
        );
        xml(207, &sync_answer(&entries, "t")).into()
    })
    .await;
    let client = client(&fake);
    let result = client
        .sync(&href(&client, "/dav/cal/work/"), None)
        .await
        .unwrap();
    assert_eq!(result.changed.len(), 1);
    assert!(result.removed.is_empty());
}

/// 507 outside a sync report, and other statuses, are typed.
#[tokio::test]
async fn statuses_are_typed() {
    let fake = Fake::start(|seen, _| match seen.path.as_str() {
        "/dav/full.ics" => empty(507),
        "/dav/conflict.ics" => xml(409, r#"<error xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:no-uid-conflict><href>/dav/x.ics</href></C:no-uid-conflict></error>"#),
        "/dav/gone.ics" => empty(404),
        _ => empty(500),
    }
    .into())
    .await;
    let client = client(&fake);
    let event = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
    let put = |path: &'static str| {
        let client = &client;
        async move {
            client
                .put(
                    &href(client, path),
                    ResourceKind::Calendar,
                    event,
                    &PutCondition::Create,
                )
                .await
        }
    };
    assert!(matches!(
        put("/dav/full.ics").await,
        Err(DavError::InsufficientStorage)
    ));
    assert!(matches!(
        put("/dav/conflict.ics").await,
        Err(DavError::Conflict {
            precondition: Some(nebula_dav::proto::Precondition::NoUidConflict)
        })
    ));
    assert!(matches!(
        put("/dav/gone.ics").await,
        Err(DavError::NotFound)
    ));
    let error = put("/dav/other.ics").await.unwrap_err();
    assert!(error.is_temporary());
    assert!(!DavError::Unauthorized.is_temporary());
}

fn busy(retry_after: &str) -> Response<Body> {
    Response::builder()
        .status(503)
        .header("retry-after", retry_after)
        .body(full(""))
        .unwrap()
}

fn other_status(result: Result<impl std::fmt::Debug, DavError>) -> u16 {
    match result {
        Err(DavError::Status { status, .. }) => status,
        other => panic!("{other:?}"),
    }
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";

/// A read answered 503 with `Retry-After` is sent again, with its body and
/// its credentials, and the answer after that is the one that counts.
#[tokio::test]
async fn a_busy_server_is_asked_again() {
    let fake = Fake::start(|_, index| {
        if index == 0 {
            return busy("0").into();
        }
        xml(207, &sync_answer("", "token-1")).into()
    })
    .await;
    let client = client(&fake);
    let collection = href(&client, "/dav/cal/work/");
    let page = client.sync(&collection, None).await.unwrap();
    assert_eq!(page.token.as_str(), "token-1");
    let seen = fake.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].method, "REPORT");
    assert_eq!(seen[1].method, "REPORT");
    assert!(!seen[0].body.is_empty());
    assert_eq!(seen[0].body, seen[1].body);
    assert_eq!(seen[0].authorization, seen[1].authorization);
}

/// At most two more tries: a server that is always busy gets three
/// requests, and the error is still temporary.
#[tokio::test]
async fn a_busy_server_is_not_asked_forever() {
    let fake = Fake::start(|_, _| busy("0").into()).await;
    let client = client(&fake);
    let result = client
        .get(&href(&client, "/dav/a.ics"), ResourceKind::Calendar)
        .await;
    let error = result.unwrap_err();
    assert!(error.is_temporary(), "{error:?}");
    assert!(matches!(error, DavError::Status { status: 503, .. }));
    assert_eq!(fake.seen().len(), 3);
}

/// What writes is never repeated, even conditional: the first one may have
/// gone through.
#[tokio::test]
async fn a_write_is_not_repeated() {
    let fake = Fake::start(|_, _| busy("0").into()).await;
    let client = client(&fake);
    let event = href(&client, "/dav/cal/work/a.ics");
    let put = client
        .put(&event, ResourceKind::Calendar, EVENT, &PutCondition::Create)
        .await;
    assert_eq!(other_status(put), 503);
    assert_eq!(other_status(client.delete(&event, None).await), 503);
    let methods: Vec<String> = fake.seen().into_iter().map(|s| s.method).collect();
    assert_eq!(methods, ["PUT", "DELETE"]);
}

/// A 429 was refused without being done, so even a write is sent again.
#[tokio::test]
async fn a_rate_limited_write_is_repeated() {
    let fake = Fake::start(|_, index| {
        if index == 0 {
            return Response::builder()
                .status(429)
                .header("retry-after", "0")
                .body(full(""))
                .unwrap()
                .into();
        }
        empty(201).into()
    })
    .await;
    let client = client(&fake);
    let event = href(&client, "/dav/cal/work/a.ics");
    client
        .put(&event, ResourceKind::Calendar, EVENT, &PutCondition::Create)
        .await
        .unwrap();
    let methods: Vec<String> = fake.seen().into_iter().map(|s| s.method).collect();
    assert_eq!(methods, ["PUT", "PUT"]);
}

/// Without a wait we can honour (none, a date, too long, past the
/// request's time limit) the 503 goes to the caller at once.
#[tokio::test]
async fn a_busy_answer_without_a_usable_wait_is_final() {
    for wait in ["none", "Sat, 31 Oct 2026 10:00:00 GMT", "11", "5", "-1"] {
        let fake = Fake::start(move |_, _| {
            if wait == "none" {
                empty(503).into()
            } else {
                busy(wait).into()
            }
        })
        .await;
        let client = client(&fake);
        let started = std::time::Instant::now();
        let result = client
            .get(&href(&client, "/dav/a.ics"), ResourceKind::Calendar)
            .await;
        assert_eq!(other_status(result), 503, "{wait}");
        assert_eq!(fake.seen().len(), 1, "{wait}");
        // `fast_limits` gives each request 1.5 s, less than the 5 s asked.
        assert!(started.elapsed() < Duration::from_secs(1), "{wait}");
    }
}

/// Other statuses are not retried, even with `Retry-After`.
#[tokio::test]
async fn only_503_and_429_are_retried() {
    let fake = Fake::start(|_, _| {
        Response::builder()
            .status(502)
            .header("retry-after", "0")
            .body(full(""))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    let result = client
        .get(&href(&client, "/dav/a.ics"), ResourceKind::Calendar)
        .await;
    assert_eq!(other_status(result), 502);
    assert_eq!(fake.seen().len(), 1);
}

// --- Free-busy and principals (libre/ocupado) -------------------------

const SCHEDULE_RESPONSE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><A:schedule-response xmlns:D=\"DAV:\" xmlns:A=\"urn:ietf:params:xml:ns:caldav\"><A:response><A:recipient><D:href>mailto:nobody@example.com</D:href></A:recipient><A:request-status>3.7;Invalid calendar user</A:request-status></A:response><A:response><A:recipient><D:href>MAILTO:Bob@Example.com</D:href></A:recipient><A:request-status>2.0;Success</A:request-status><A:calendar-data><![CDATA[BEGIN:VCALENDAR\nMETHOD:REPLY\nBEGIN:VFREEBUSY\nFREEBUSY:20261020T133000Z/20261020T143000Z\nEND:VFREEBUSY\nEND:VCALENDAR\n]]></A:calendar-data></A:response><A:response><A:recipient><D:href>mailto:eve@example.com</D:href></A:recipient><A:request-status>2.0;Success</A:request-status><A:calendar-data>BEGIN:VCALENDAR</A:calendar-data></A:response></A:schedule-response>";

fn free_busy_request(attendees: &[&str]) -> FreeBusyRequest {
    FreeBusyRequest {
        uid: "nebula-fb-test".to_owned(),
        stamp: 1_793_059_199,
        start: 1_792_454_400,
        end: 1_793_664_000,
        organizer: CalendarAddress::parse("alice@example.com").unwrap(),
        attendees: attendees
            .iter()
            .map(|a| CalendarAddress::parse(a).unwrap())
            .collect(),
    }
}

/// The outbox POST (RFC 6638 §5): the VFREEBUSY request goes to the
/// outbox with the credentials; one answer per attendee asked, in order,
/// matched without case; a recipient nobody asked for is dropped.
#[tokio::test]
async fn free_busy_through_the_outbox() {
    let fake = Fake::start(|seen, _| {
        if seen.method == "POST" && seen.path == "/dav/outbox/" {
            xml(200, SCHEDULE_RESPONSE).into()
        } else {
            empty(404).into()
        }
    })
    .await;
    let client = client(&fake);
    let outbox = href(&client, "/dav/outbox/");
    let answers = client
        .free_busy(
            &outbox,
            &free_busy_request(&["bob@example.com", "nobody@example.com", "carol@example.com"]),
        )
        .await
        .unwrap();
    assert_eq!(answers.len(), 3);
    assert_eq!(answers[0].attendee.address(), "bob@example.com");
    assert!(answers[0].has_data());
    assert!(
        answers[0]
            .calendar_data
            .as_deref()
            .unwrap()
            .contains("FREEBUSY:20261020T133000Z/20261020T143000Z\r\n")
    );
    assert_eq!(answers[1].status.as_ref().unwrap().code(), "3.7");
    assert!(!answers[1].has_data());
    // Not in the answer at all.
    assert_eq!(answers[2].status, None);
    assert!(!answers[2].has_data());
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert!(seen[0].authorization.is_some());
    let body = &seen[0].body;
    assert!(body.starts_with("BEGIN:VCALENDAR\r\n"), "{body}");
    assert!(body.contains("\r\nMETHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\n"));
    assert!(body.contains("\r\nORGANIZER:mailto:alice@example.com\r\n"));
    assert!(body.contains("\r\nATTENDEE:mailto:bob@example.com\r\n"));
    assert!(body.contains("\r\nATTENDEE:mailto:carol@example.com\r\n"));
}

/// A hostile outbox: elsewhere (nothing sent), a 302 that would turn the
/// POST into a GET, HTML, a DTD, a body over 1 MiB, the wrong status or
/// root, too many attendees. A 307 is followed with the same body.
#[tokio::test]
async fn hostile_outbox() {
    let big = format!(
        "<c:schedule-response xmlns:c=\"urn:ietf:params:xml:ns:caldav\">{}</c:schedule-response>",
        " ".repeat(MAX_FREEBUSY_RESPONSE + 1)
    );
    let fake = Fake::start(move |seen, _| {
        match seen.path.as_str() {
            "/dav/found/" => redirect(302, "/dav/outbox/"),
            "/dav/temporary/" => redirect(307, "/dav/outbox/"),
            "/dav/html/" => Response::builder()
                .status(200)
                .header("content-type", "text/html")
                .body(full("<html></html>"))
                .unwrap(),
            "/dav/dtd/" => xml(
                200,
                r#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><c:schedule-response xmlns:c="urn:ietf:params:xml:ns:caldav">&e;</c:schedule-response>"#,
            ),
            "/dav/big/" => xml(200, &big),
            "/dav/multistatus/" => xml(207, EMPTY_MULTISTATUS),
            "/dav/root/" => xml(200, EMPTY_MULTISTATUS),
            "/dav/outbox/" if seen.method == "POST" && seen.body.contains("VFREEBUSY") => {
                xml(200, SCHEDULE_RESPONSE)
            }
            _ => empty(400),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let request = free_busy_request(&["bob@example.com"]);

    let elsewhere =
        Href::from_url(&url::Url::parse("https://evil.example/outbox/").unwrap()).unwrap();
    let result = client.free_busy(&elsewhere, &request).await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );
    assert!(fake.seen().is_empty());

    let result = client
        .free_busy(&href(&client, "/dav/found/"), &request)
        .await;
    assert!(
        matches!(result, Err(DavError::Redirect(RedirectError::Method))),
        "{result:?}"
    );
    let followed = client
        .free_busy(&href(&client, "/dav/temporary/"), &request)
        .await
        .unwrap();
    assert!(followed[0].has_data());

    let result = client
        .free_busy(&href(&client, "/dav/html/"), &request)
        .await;
    assert!(matches!(result, Err(DavError::ContentType)), "{result:?}");
    let result = client
        .free_busy(&href(&client, "/dav/dtd/"), &request)
        .await;
    assert!(
        matches!(result, Err(DavError::Xml(ParseError::Dtd))),
        "{result:?}"
    );
    let result = client
        .free_busy(&href(&client, "/dav/big/"), &request)
        .await;
    assert!(matches!(result, Err(DavError::TooLarge)), "{result:?}");
    let result = client
        .free_busy(&href(&client, "/dav/multistatus/"), &request)
        .await;
    assert!(matches!(result, Err(DavError::Protocol(_))), "{result:?}");
    let result = client
        .free_busy(&href(&client, "/dav/root/"), &request)
        .await;
    assert!(matches!(result, Err(DavError::Xml(_))), "{result:?}");

    let before = fake.seen().len();
    let many: Vec<String> = (0..=MAX_FREEBUSY_ATTENDEES)
        .map(|n| format!("u{n}@example.com"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let result = client
        .free_busy(&href(&client, "/dav/outbox/"), &free_busy_request(&many))
        .await;
    assert!(matches!(result, Err(DavError::Encode(_))), "{result:?}");
    assert_eq!(fake.seen().len(), before, "nothing sent");
}

const PRINCIPALS: &str = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:response><d:href>/dav/principals/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/dav/principals/bob/</d:href><d:propstat><d:prop><d:resourcetype><d:principal/><d:collection/></d:resourcetype><d:displayname>Bob</d:displayname><c:calendar-user-address-set><d:href>mailto:bob@example.com</d:href><d:href>/dav/principals/bob/</d:href></c:calendar-user-address-set><c:calendar-user-type>INDIVIDUAL</c:calendar-user-type></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/dav/principals/rooms/blue/</d:href><d:propstat><d:prop><d:displayname>Blue room</d:displayname><c:calendar-user-address-set><d:href>mailto:blue@example.com</d:href></c:calendar-user-address-set><c:calendar-user-type>ROOM</c:calendar-user-type></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/dav/principals/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/dav/other/eve/</d:href><d:propstat><d:prop><d:resourcetype><d:principal/></d:resourcetype><c:calendar-user-address-set><d:href>mailto:bob@example.com</d:href></c:calendar-user-address-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>https://evil.example/dav/principals/mallory/</d:href><d:propstat><d:prop><d:resourcetype><d:principal/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;

/// Principal search and listing only keep principals inside the
/// collection (a search may go deeper, a listing only direct members),
/// never other places or hosts; the searchable properties are read, and
/// Radicale's empty multistatus is an error (the caller treats it as
/// "no search").
#[tokio::test]
async fn principal_search_and_listing() {
    let fake = Fake::start(|seen, _| {
        match (seen.method.as_str(), seen.path.as_str()) {
            ("REPORT", "/dav/principals/")
                if seen.body.contains("principal-search-property-set") =>
            {
                xml(
                    200,
                    r#"<principal-search-property-set xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><principal-search-property><prop><c:calendar-user-address-set/></prop></principal-search-property></principal-search-property-set>"#,
                )
            }
            ("REPORT", "/dav/principals/") if seen.body.contains("principal-property-search") => {
                xml(207, PRINCIPALS)
            }
            ("PROPFIND", "/dav/principals/") => xml(207, PRINCIPALS),
            ("REPORT", "/dav/radicale/") => xml(207, EMPTY_MULTISTATUS),
            _ => empty(405),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let collection = href(&client, "/dav/principals/");
    assert_eq!(
        client
            .principal_search_properties(&collection)
            .await
            .unwrap(),
        [Property::CalendarUserAddressSet]
    );
    let result = client
        .principal_search_properties(&href(&client, "/dav/radicale/"))
        .await;
    assert!(matches!(result, Err(DavError::Xml(_))), "{result:?}");

    let found = client
        .search_principals(
            &collection,
            Property::CalendarUserAddressSet,
            "bob@example.com",
        )
        .await
        .unwrap();
    let paths: Vec<&str> = found.iter().map(|p| p.href.path()).collect();
    assert_eq!(
        paths,
        ["/dav/principals/bob/", "/dav/principals/rooms/blue/"]
    );
    assert_eq!(found[0].display_name.as_deref(), Some("Bob"));
    assert_eq!(
        found[0].addresses,
        ["mailto:bob@example.com", "/dav/principals/bob/"]
    );
    assert_eq!(found[1].kind, Some(CalendarUserType::Room));
    let search = fake
        .seen()
        .into_iter()
        .find(|s| s.body.contains("principal-property-search"))
        .unwrap();
    assert!(search.body.contains("<D:match>bob@example.com</D:match>"));

    let listed = client.list_principals(&collection).await.unwrap();
    let paths: Vec<&str> = listed.iter().map(|p| p.href.path()).collect();
    assert_eq!(paths, ["/dav/principals/bob/"]);

    let result = client.list_principals(&href(&client, "/dav/closed/")).await;
    assert!(
        matches!(result, Err(DavError::MethodNotAllowed)),
        "{result:?}"
    );
    let result = client
        .search_principals(&collection, Property::DisplayName, "")
        .await;
    assert!(matches!(result, Err(DavError::Encode(_))), "{result:?}");
}

/// Sharing: the ACL is read with a PROPFIND Depth 0 and
/// written back with the `ACL` method, editable ACEs only; a principal on
/// another origin is never sent (its path would name someone else here);
/// a 403 keeps the server's precondition.
#[tokio::test]
async fn acl_read_and_write() {
    let fake = Fake::start(|seen, _| {
        match (seen.method.as_str(), seen.path.as_str()) {
            ("PROPFIND", "/dav/cal/alice/work/") => xml(
                207,
                r#"<multistatus xmlns="DAV:"><response><href>/dav/cal/alice/work/</href><propstat><prop><acl>
                <ace><principal><href>/dav/pal/bob/</href></principal><grant><privilege><read/></privilege></grant></ace>
                <ace><principal><href>/dav/pal/alice/</href></principal><grant><privilege><all/></privilege></grant><protected/></ace>
                </acl></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
            ),
            ("ACL", "/dav/cal/alice/work/") => empty(200),
            ("ACL", "/dav/cal/alice/denied/") => {
                xml(403, r#"<error xmlns="DAV:"><grant-only/></error>"#)
            }
            _ => empty(404),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/alice/work/");
    let acl = client.read_acl(&work).await.unwrap();
    assert_eq!(acl.aces.len(), 2);
    assert_eq!(acl.editable().count(), 1);
    let carol = href(&client, "/dav/pal/carol/");
    let mut aces: Vec<Ace> = acl.editable().cloned().collect();
    aces.push(Ace::grant(carol, &[Privilege::Read]));
    client.set_acl(&work, &aces).await.unwrap();
    let sent = fake.seen().pop().unwrap();
    assert_eq!(sent.method, "ACL");
    assert!(sent.body.contains("<D:href>/dav/pal/bob/</D:href>"));
    assert!(sent.body.contains("<D:href>/dav/pal/carol/</D:href>"));
    assert!(
        !sent.body.contains("/dav/pal/alice/"),
        "protected ACEs stay out"
    );
    // Unsharing everything is an empty ACL.
    client.set_acl(&work, &[]).await.unwrap();
    assert!(!fake.seen().pop().unwrap().body.contains("<D:ace>"));
    // A principal of another host is never written as a path here.
    let elsewhere =
        Href::resolve("https://evil.example/dav/pal/bob/", &client.server().base).unwrap();
    let before = fake.seen().len();
    let result = client
        .set_acl(&work, &[Ace::grant(elsewhere, &[Privilege::Read])])
        .await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );
    assert_eq!(fake.seen().len(), before, "nothing sent");
    let denied = client
        .set_acl(&href(&client, "/dav/cal/alice/denied/"), &aces)
        .await;
    assert!(
        matches!(denied, Err(DavError::Forbidden { .. })),
        "{denied:?}"
    );
    // No ACL shown (no read-acl): an error, not an empty ACL.
    let missing = client
        .read_acl(&href(&client, "/dav/cal/alice/none/"))
        .await;
    assert!(missing.is_err());
}

/// Calendar Server sharing: a `CS:share` POST to the calendar; only 307
/// and 308 redirects are followed (a 302 would turn it into a GET).
#[tokio::test]
async fn calendar_server_share() {
    let fake = Fake::start(|seen, _| {
        match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/dav/cal/alice/work/") => empty(200),
            ("POST", "/dav/cal/alice/moved/") => redirect(302, "/dav/cal/alice/work/"),
            _ => empty(404),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/alice/work/");
    let bob = Sharee::Address(CalendarAddress::parse("bob@example.com").unwrap());
    let carol = Sharee::Principal(href(&client, "/dav/pal/carol/"));
    client
        .share(&work, &[(bob.clone(), ShareAccess::Read)], &[carol])
        .await
        .unwrap();
    let sent = fake.seen().pop().unwrap();
    assert!(sent.body.contains("<CS:share"), "{}", sent.body);
    assert!(sent.body.contains(
        "<CS:set><D:href>mailto:bob@example.com</D:href><CS:read/></CS:set><CS:remove><D:href>/dav/pal/carol/</D:href></CS:remove>"
    ));
    let moved = client
        .share(
            &href(&client, "/dav/cal/alice/moved/"),
            &[(bob, ShareAccess::ReadWrite)],
            &[],
        )
        .await;
    assert!(matches!(moved, Err(DavError::Redirect(_))), "{moved:?}");
    let elsewhere = Sharee::Principal(
        Href::resolve("https://evil.example/dav/pal/bob/", &client.server().base).unwrap(),
    );
    let result = client.share(&work, &[], &[elsewhere]).await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );
    let nothing = client.share(&work, &[], &[]).await;
    assert!(matches!(nothing, Err(DavError::Encode(_))), "{nothing:?}");
}

/// Nextcloud sharing: an `O:share` POST to the calendar or address book,
/// naming principals as `principal:` paths below the DAV root; a hostile
/// `O:invite` in a listing never fails it; only 307 and 308 redirects are
/// followed; a collection on another origin is never asked.
#[tokio::test]
async fn nextcloud_share() {
    let fake = Fake::start(|seen, _| {
        match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/remote.php/dav/addressbooks/users/alice/people/") => empty(200),
            ("POST", "/remote.php/dav/calendars/alice/moved/") => {
                redirect(302, "/remote.php/dav/calendars/alice/work/")
            }
            ("PROPFIND", "/remote.php/dav/calendars/alice/") => xml(
                207,
                r#"<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:oc="http://owncloud.org/ns"><d:response><d:href>/remote.php/dav/calendars/alice/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><oc:invite><oc:user><d:href>principal:principals/users/../../x</d:href></oc:user><oc:user><d:href>principal:principals/users/bob</d:href><oc:invite-accepted/><oc:access><oc:read-write/></oc:access></oc:user><oc:user><d:href>https://evil.example/principals/users/eve</d:href></oc:user></oc:invite></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
            ),
            _ => empty(404),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let listed = client
        .list_collections(&href(&client, "/remote.php/dav/calendars/alice/"))
        .await
        .unwrap();
    let invite = listed[0].oc_invite.as_ref().unwrap();
    assert_eq!(invite.users.len(), 1);
    assert_eq!(
        invite.users[0].principal.to_string(),
        "principal:principals/users/bob"
    );
    assert_eq!(invite.users[0].access, ShareAccess::ReadWrite);

    let people = href(&client, "/remote.php/dav/addressbooks/users/alice/people/");
    let bob = OcPrincipal::parse("principal:principals/users/bob").unwrap();
    let carol = OcPrincipal::parse("principal:principals/users/carol").unwrap();
    client
        .oc_share(&people, &[(bob.clone(), ShareAccess::ReadWrite)], &[carol])
        .await
        .unwrap();
    let sent = fake.seen().pop().unwrap();
    assert_eq!(sent.method, "POST");
    assert!(sent.body.contains("<O:share"), "{}", sent.body);
    assert!(sent.body.contains(
        "<O:set><D:href>principal:principals/users/bob</D:href><O:read-write/></O:set><O:remove><D:href>principal:principals/users/carol</D:href></O:remove>"
    ));
    let moved = client
        .oc_share(
            &href(&client, "/remote.php/dav/calendars/alice/moved/"),
            &[(bob.clone(), ShareAccess::Read)],
            &[],
        )
        .await;
    assert!(matches!(moved, Err(DavError::Redirect(_))), "{moved:?}");
    let before = fake.seen().len();
    let elsewhere = Href::resolve(
        "https://evil.example/remote.php/dav/calendars/alice/work/",
        &client.server().base,
    )
    .unwrap();
    let result = client
        .oc_share(&elsewhere, &[(bob, ShareAccess::Read)], &[])
        .await;
    assert!(
        matches!(result, Err(DavError::OriginNotAllowed)),
        "{result:?}"
    );
    assert_eq!(fake.seen().len(), before, "nothing sent");
    let nothing = client.oc_share(&people, &[], &[]).await;
    assert!(matches!(nothing, Err(DavError::Encode(_))), "{nothing:?}");
}

// --- Connections, retries and redirects of writes --------------------

/// A raw HTTP/1.1 server (hyper cannot close a connection without
/// answering): `answer(connection, request)` gives the body of a 207, or
/// `None` to read the request and close without answering.
struct Raw {
    port: u16,
    connections: Arc<AtomicUsize>,
    methods: Arc<Mutex<Vec<String>>>,
}

impl Raw {
    async fn start(
        answer: impl Fn(usize, usize) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let methods = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new(answer);
        let (accepted, log) = (Arc::clone(&connections), Arc::clone(&methods));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let connection = accepted.fetch_add(1, Ordering::SeqCst);
                let (answer, log) = (Arc::clone(&answer), Arc::clone(&log));
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    for request in 0.. {
                        // The head, then the body of its Content-Length.
                        let head_end = loop {
                            if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                                break at + 4;
                            }
                            let mut chunk = [0_u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
                        let length: usize = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().ok())?
                            })
                            .unwrap_or(0);
                        while buffer.len() < head_end + length {
                            let mut chunk = [0_u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        }
                        buffer.drain(..head_end + length);
                        let method = head.split(' ').next().unwrap_or_default().to_owned();
                        log.lock().unwrap().push(method);
                        let Some(body) = answer(connection, request) else {
                            return;
                        };
                        let response = format!(
                            "HTTP/1.1 207 Multi-Status\r\ncontent-type: application/xml\r\ncontent-length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        if stream.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self {
            port,
            connections,
            methods,
        }
    }

    fn client(&self) -> DavClient {
        let base = format!("http://{}:{}/dav/", Ipv4Addr::LOCALHOST, self.port);
        let server = DavServer::new(&base)
            .unwrap()
            .with_private_network(true)
            .with_limits(fast_limits());
        client_for(server, Dns::new(&[]))
    }

    fn methods(&self) -> Vec<String> {
        self.methods.lock().unwrap().clone()
    }
}

/// The second request on the first connection is read and then the
/// connection closes without an answer.
fn dies_on_the_second(connection: usize, request: usize) -> Option<String> {
    (connection > 0 || request == 0).then(|| EMPTY_MULTISTATUS.to_owned())
}

/// A write that was sent on a reused connection that then died may have
/// been done: it is not sent again (a PUT would get a false 412).
#[tokio::test]
async fn a_write_sent_on_a_dead_connection_is_not_repeated() {
    let raw = Raw::start(dies_on_the_second).await;
    let client = raw.client();
    let work = href(&client, "/dav/cal/work/");
    client.list_members(&work).await.unwrap();
    let event = href(&client, "/dav/cal/work/a.ics");
    let put = client
        .put(&event, ResourceKind::Calendar, EVENT, &PutCondition::Create)
        .await;
    assert!(matches!(put, Err(DavError::Http(_))), "{put:?}");
    assert_eq!(raw.methods(), ["PROPFIND", "PUT"]);
    assert_eq!(raw.connections.load(Ordering::SeqCst), 1);
}

/// A read is sent again on a new connection.
#[tokio::test]
async fn a_read_sent_on_a_dead_connection_is_repeated() {
    let raw = Raw::start(dies_on_the_second).await;
    let client = raw.client();
    let work = href(&client, "/dav/cal/work/");
    client.list_members(&work).await.unwrap();
    client.list_members(&work).await.unwrap();
    assert_eq!(raw.methods(), ["PROPFIND", "PROPFIND", "PROPFIND"]);
    assert_eq!(raw.connections.load(Ordering::SeqCst), 2);
}

/// A connection unused for longer than `idle_timeout` is not reused: a
/// NAT may have dropped it without telling anyone.
#[tokio::test]
async fn old_idle_connections_are_not_reused() {
    let fake = Fake::start(|_, _| xml(207, EMPTY_MULTISTATUS).into()).await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    client.list_members(&work).await.unwrap();
    client.list_members(&work).await.unwrap();
    assert_eq!(fake.connections(), 1);
    tokio::time::sleep(fast_limits().idle_timeout + Duration::from_millis(100)).await;
    client.list_members(&work).await.unwrap();
    assert_eq!(fake.connections(), 2);
}

/// A head over the limit on a reused connection is an answer: never a
/// reason to open another connection and send the request again (a PUT
/// is used, as hyper may refuse the head itself while reading, and a read
/// failing that way would be asked again).
#[tokio::test]
async fn a_huge_head_on_a_reused_connection_is_not_sent_again() {
    let fake = Fake::start(|_, index| {
        if index == 0 {
            return xml(207, EMPTY_MULTISTATUS).into();
        }
        Response::builder()
            .status(201)
            .header("x-padding", "a".repeat(100 * 1024))
            .body(full(""))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    client
        .list_members(&href(&client, "/dav/cal/work/"))
        .await
        .unwrap();
    let event = href(&client, "/dav/cal/work/a.ics");
    let result = client
        .put(&event, ResourceKind::Calendar, EVENT, &PutCondition::Create)
        .await;
    assert!(
        matches!(result, Err(DavError::Http(_) | DavError::HeadTooLarge)),
        "{result:?}"
    );
    assert_eq!(fake.seen().len(), 2);
    assert_eq!(fake.connections(), 1);
}

/// A 503 whose compressed body passes the error body's limit only in the
/// final flush keeps its status (it was `Decompress`).
#[tokio::test]
async fn a_compressed_error_body_over_the_limit_keeps_its_status() {
    let limit = Limits::default().max_error_body;
    let compressed = {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&vec![b'a'; limit + 1]).unwrap();
        encoder.finish().unwrap()
    };
    let fake = Fake::start(move |_, _| {
        Response::builder()
            .status(503)
            .header("content-encoding", "gzip")
            .body(full(compressed.clone()))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    let result = client
        .get(&href(&client, "/dav/a.ics"), ResourceKind::Calendar)
        .await;
    assert_eq!(other_status(result), 503);
}

/// A 303 means "GET it there": a write would become a GET whose 200 looks
/// like the write done. Only GET and HEAD follow it; 301/302/307/308
/// repeat the request as it is.
#[tokio::test]
async fn writes_do_not_follow_303() {
    let fake = Fake::start(|seen, _| {
        match (seen.method.as_str(), seen.path.as_str()) {
            (_, "/ok.html") => empty(200),
            ("GET", "/dav/cal/work/see.ics") => redirect(303, "/dav/cal/work/there.ics"),
            ("GET", "/dav/cal/work/there.ics") => Response::builder()
                .status(200)
                .header("content-type", "text/calendar")
                .header("etag", "\"1\"")
                .body(full(EVENT))
                .unwrap(),
            (_, "/dav/cal/work/moved.ics") => redirect(301, "/dav/cal/work/new.ics"),
            ("PUT", "/dav/cal/work/new.ics") => empty(201),
            _ => redirect(303, "/ok.html"),
        }
        .into()
    })
    .await;
    let client = client(&fake);
    let event = href(&client, "/dav/cal/work/a.ics");
    let calendar = href(&client, "/dav/cal/work/");
    let refused = |result: Result<(), DavError>| {
        assert!(
            matches!(result, Err(DavError::Redirect(RedirectError::Method))),
            "{result:?}"
        );
    };
    refused(
        client
            .put(&event, ResourceKind::Calendar, EVENT, &PutCondition::Create)
            .await
            .map(|_| ()),
    );
    refused(client.delete(&event, None).await);
    refused(client.set_acl(&calendar, &[]).await);
    refused(
        client
            .proppatch(
                &calendar,
                &[nebula_dav::proto::request::PropValue::DisplayName(
                    "x".to_owned(),
                )],
                &[],
            )
            .await
            .map(|_| ()),
    );
    assert!(
        fake.seen().iter().all(|s| s.method != "GET"),
        "{:?}",
        fake.seen()
    );
    // A 301 repeats the PUT, with its body.
    client
        .put(
            &href(&client, "/dav/cal/work/moved.ics"),
            ResourceKind::Calendar,
            EVENT,
            &PutCondition::Create,
        )
        .await
        .unwrap();
    let last = fake.seen().pop().unwrap();
    assert_eq!(
        (last.method.as_str(), last.path.as_str()),
        ("PUT", "/dav/cal/work/new.ics")
    );
    assert_eq!(last.body, EVENT);
    // A GET follows a 303.
    let got = client
        .get(
            &href(&client, "/dav/cal/work/see.ics"),
            ResourceKind::Calendar,
        )
        .await
        .unwrap();
    assert_eq!(got.data, EVENT);
}

// --- One bad resource does not stop the rest -------------------------

/// The hrefs a multiget asks for, in order.
fn asked_hrefs(body: &str) -> Vec<String> {
    body.split("<D:href>")
        .skip(1)
        .filter_map(|rest| {
            rest.split_once("</D:href>")
                .map(|(href, _)| href.to_owned())
        })
        .collect()
}

fn event_of(size: usize) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nX-PAD:{}\r\nEND:VCALENDAR\r\n",
        "a".repeat(size)
    )
}

fn fetched_response(href: &str, data: &str) -> String {
    format!(
        r#"<response><href>{href}</href><propstat><prop><getetag>"1"</getetag><C:calendar-data>{data}</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
    )
}

fn multiget_answer(responses: &str) -> String {
    format!(
        r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{responses}</multistatus>"#
    )
}

/// An event over `max_icalendar` comes back alone with 507 and no data;
/// the others of the batch are read (the whole multiget used to fail, in
/// every round).
#[tokio::test]
async fn multiget_survives_an_oversized_resource() {
    let fake = Fake::start(|seen, _| {
        let responses: String = asked_hrefs(&seen.body)
            .iter()
            .map(|href| {
                let size = if href.ends_with("big.ics") {
                    20 * 1024
                } else {
                    10
                };
                fetched_response(href, &event_of(size))
            })
            .collect();
        xml(207, &multiget_answer(&responses)).into()
    })
    .await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let hrefs: Vec<Href> = ["a.ics", "big.ics", "c.ics"]
        .iter()
        .map(|name| href(&client, &format!("/dav/cal/work/{name}")))
        .collect();
    let fetched = client
        .multiget(&work, ResourceKind::Calendar, &hrefs)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 3);
    assert!(fetched[0].data.is_some());
    assert_eq!(fetched[1].data, None);
    assert_eq!(fetched[1].status.map(Status::code), Some(507));
    assert!(fetched[2].data.is_some());
}

/// A batch whose answer is over `max_multistatus` is split in two; a
/// single resource still too large comes back with 507.
#[tokio::test]
async fn multiget_splits_a_batch_that_is_too_large() {
    let fake = Fake::start(|seen, _| {
        let mut responses = String::new();
        for href in asked_hrefs(&seen.body) {
            // The huge one is padded outside its data, so that it fits
            // `max_icalendar` but not the body alone.
            if href.ends_with("huge.ics") {
                responses.push_str("<!--");
                responses.push_str(&"p".repeat(70 * 1024));
                responses.push_str("-->");
            }
            let size = if href.ends_with("huge.ics") {
                15 * 1024
            } else {
                12 * 1024
            };
            responses.push_str(&fetched_response(&href, &event_of(size)));
        }
        xml(207, &multiget_answer(&responses)).into()
    })
    .await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let hrefs: Vec<Href> = (0..8)
        .map(|i| href(&client, &format!("/dav/cal/work/{i}.ics")))
        .collect();
    let fetched = client
        .multiget(&work, ResourceKind::Calendar, &hrefs)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 8);
    assert!(fetched.iter().all(|f| f.data.is_some()));
    let paths: Vec<&str> = fetched.iter().map(|f| f.href.path()).collect();
    let expected: Vec<String> = (0..8).map(|i| format!("/dav/cal/work/{i}.ics")).collect();
    assert_eq!(paths, expected);
    // 8, then 4 and 4.
    assert_eq!(fake.seen().len(), 3);

    let huge = [href(&client, "/dav/cal/work/huge.ics")];
    let fetched = client
        .multiget(&work, ResourceKind::Calendar, &huge)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].data, None);
    assert_eq!(fetched[0].status.map(Status::code), Some(507));
}

/// Against a server whose answer is always too large, the splits are
/// limited (two per batch): then the multiget fails with `TooLarge`
/// instead of asking twice per href.
#[tokio::test]
async fn multiget_splits_are_limited() {
    let pad = "p".repeat(70 * 1024);
    let fake = Fake::start(move |_, _| {
        xml(207, &format!("<!--{pad}--><multistatus xmlns=\"DAV:\"/>")).into()
    })
    .await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let hrefs: Vec<Href> = (0..8)
        .map(|i| href(&client, &format!("/dav/cal/work/{i}.ics")))
        .collect();
    let result = client.multiget(&work, ResourceKind::Calendar, &hrefs).await;
    assert!(matches!(result, Err(DavError::TooLarge)), "{result:?}");
    // 8 (split), 4 (split), 2 (no splits left).
    assert_eq!(fake.seen().len(), 3);
}

/// Only a real 404 says a resource is gone: a 403 or 500 for its data, or
/// a value that is not text, is not.
#[tokio::test]
async fn multiget_statuses_are_kept() {
    let body = multiget_answer(&format!(
        "{}{}{}{}",
        r"<response><href>/dav/cal/work/gone.ics</href><status>HTTP/1.1 404 Not Found</status></response>",
        r#"<response><href>/dav/cal/work/denied.ics</href><propstat><prop><getetag>"1"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat><propstat><prop><C:calendar-data/></prop><status>HTTP/1.1 403 Forbidden</status></propstat></response>"#,
        r"<response><href>/dav/cal/work/odd.ics</href><propstat><prop><C:calendar-data><x/></C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response>",
        fetched_response("/dav/cal/work/ok.ics", EVENT),
    ));
    let fake = Fake::start(move |_, _| xml(207, &body).into()).await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let hrefs: Vec<Href> = ["gone.ics", "denied.ics", "odd.ics", "ok.ics"]
        .iter()
        .map(|name| href(&client, &format!("/dav/cal/work/{name}")))
        .collect();
    let fetched = client
        .multiget(&work, ResourceKind::Calendar, &hrefs)
        .await
        .unwrap();
    let statuses: Vec<(&str, Option<u16>, bool)> = fetched
        .iter()
        .map(|f| (f.href.path(), f.status.map(Status::code), f.data.is_some()))
        .collect();
    assert_eq!(
        statuses,
        [
            ("/dav/cal/work/gone.ics", Some(404), false),
            ("/dav/cal/work/denied.ics", Some(403), false),
            ("/dav/cal/work/odd.ics", None, false),
            ("/dav/cal/work/ok.ics", None, true),
        ]
    );
}

// --- Listings that are not what they seem ----------------------------

/// A collection the server answers from another place (a redirect) is an
/// error: its hrefs would all look outside, the listing would be empty
/// and everything taken as deleted.
#[tokio::test]
async fn a_moved_collection_is_not_an_empty_listing() {
    let fake = Fake::start(|seen, _| {
        if seen.path == "/dav/cal/x/" {
            return redirect(301, "/cal/x/").into();
        }
        let entries = format!(
            "{}{}",
            changed("/cal/x/1.ics", Some("\"1\"")),
            changed("/cal/x/2.ics", Some("\"2\""))
        );
        xml(207, &sync_answer(&entries, "t1")).into()
    })
    .await;
    let client = client(&fake);
    let x = href(&client, "/dav/cal/x/");
    let listed = client.list_members(&x).await;
    assert!(matches!(listed, Err(DavError::Protocol(_))), "{listed:?}");
    let synced = client.sync(&x, None).await;
    assert!(matches!(synced, Err(DavError::Protocol(_))), "{synced:?}");
    // The trailing slash alone is not another place.
    let fake = Fake::start(|seen, _| {
        if seen.path == "/dav/cal/y" {
            return redirect(301, "/dav/cal/y/").into();
        }
        xml(
            207,
            &sync_answer(&changed("/dav/cal/y/1.ics", Some("\"1\"")), "t1"),
        )
        .into()
    })
    .await;
    let client = client_for(config(&fake), Dns::new(&[]));
    let y = Href::resolve("/dav/cal/y", &client.server().base).unwrap();
    assert_eq!(client.list_members(&y).await.unwrap().len(), 1);
}

/// A member with an error other than 404 is there, without etag: it is
/// fetched (and kept if that fails), never taken as deleted.
#[tokio::test]
async fn members_with_errors_are_not_gone() {
    let failing = "<d:response><d:href>/dav/cal/work/locked.ics</d:href><d:status>HTTP/1.1 500 Internal Server Error</d:status></d:response>";
    let entries = format!(
        "{}{failing}{}",
        changed("/dav/cal/work/ok.ics", Some("\"1\"")),
        removed("/dav/cal/work/gone.ics")
    );
    let fake = Fake::start(move |_, _| xml(207, &sync_answer(&entries, "t1")).into()).await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let members = client.list_members(&work).await.unwrap();
    let listed: Vec<(&str, bool)> = members
        .iter()
        .map(|m| (m.href.path(), m.etag.is_some()))
        .collect();
    assert_eq!(
        listed,
        [
            ("/dav/cal/work/ok.ics", true),
            ("/dav/cal/work/locked.ics", false)
        ]
    );
    let result = client.sync(&work, None).await.unwrap();
    let changed: Vec<(&str, bool)> = result
        .changed
        .iter()
        .map(|m| (m.href.path(), m.etag.is_some()))
        .collect();
    assert!(
        changed.contains(&("/dav/cal/work/locked.ics", false)),
        "{changed:?}"
    );
    assert_eq!(result.removed.len(), 1);
    assert_eq!(result.removed[0].path(), "/dav/cal/work/gone.ics");
}

/// What others share is listed in the home too, so the number of
/// collections never fails the listing (anyone who can share would stop
/// the account's sync); the caller keeps what fits.
#[tokio::test]
async fn many_collections_are_listed() {
    let mut responses = String::new();
    for i in 0..2001 {
        let _ = write!(
            responses,
            r"<response><href>/dav/cal/alice/c{i}/</href><propstat><prop><resourcetype><collection/><C:calendar/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>"
        );
    }
    let body = format!(
        r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{responses}</multistatus>"#
    );
    let fake = Fake::start(move |_, _| xml(207, &body).into()).await;
    let mut limits = fast_limits();
    limits.max_multistatus = 16 * 1024 * 1024;
    let client = client_for(config(&fake).with_limits(limits), Dns::new(&[]));
    let found = client
        .list_collections(&href(&client, "/dav/cal/alice/"))
        .await
        .unwrap();
    assert_eq!(found.len(), 2001);
}

/// Lots of `=` in values (never escaped in XML) are text, not markup:
/// three events of about 900 KB full of `=` (2.7 M in all, more than
/// twice `max_xml_nodes`) are read, they do not stop the collection.
#[tokio::test]
async fn equals_signs_in_event_data_are_read() {
    let fake = Fake::start(|seen, _| {
        let data = format!(
            "BEGIN:VCALENDAR\r\nDESCRIPTION:{}\r\nEND:VCALENDAR\r\n",
            "=".repeat(900_000)
        );
        let responses: String = asked_hrefs(&seen.body)
            .iter()
            .map(|href| fetched_response(href, &data))
            .collect();
        xml(207, &multiget_answer(&responses)).into()
    })
    .await;
    let mut limits = fast_limits();
    limits.max_multistatus = 16 * 1024 * 1024;
    limits.max_icalendar = 1024 * 1024;
    let client = client_for(config(&fake).with_limits(limits), Dns::new(&[]));
    let work = href(&client, "/dav/cal/work/");
    let hrefs: Vec<Href> = ["a.ics", "b.ics", "c.ics"]
        .iter()
        .map(|name| href(&client, &format!("/dav/cal/work/{name}")))
        .collect();
    let fetched = client
        .multiget(&work, ResourceKind::Calendar, &hrefs)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 3);
    assert!(fetched.iter().all(|f| f.data.is_some()));
    // One request: the batch was not split.
    assert_eq!(fake.seen().len(), 1);
}

/// Shared calendars whose `calendar-timezone` is full of `=` do not stop
/// the listing of the home (and so the account's sync).
#[tokio::test]
async fn equals_signs_in_timezones_do_not_stop_the_home() {
    let timezone = format!(
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:{}\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
        "=".repeat(700_000)
    );
    let mut responses = String::new();
    for i in 0..4 {
        let _ = write!(
            responses,
            r"<response><href>/dav/cal/alice/c{i}/</href><propstat><prop><resourcetype><collection/><C:calendar/></resourcetype><C:calendar-timezone>{timezone}</C:calendar-timezone></prop><status>HTTP/1.1 200 OK</status></propstat></response>"
        );
    }
    let body = format!(
        r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{responses}</multistatus>"#
    );
    let fake = Fake::start(move |_, _| xml(207, &body).into()).await;
    let mut limits = fast_limits();
    limits.max_multistatus = 16 * 1024 * 1024;
    let client = client_for(config(&fake).with_limits(limits), Dns::new(&[]));
    let found = client
        .list_collections(&href(&client, "/dav/cal/alice/"))
        .await
        .unwrap();
    assert_eq!(found.len(), 4);
}

/// The discovery fake: the principal's answer is `principal`.
async fn discovery_fake(principal: &'static str) -> Fake {
    Fake::start(move |seen, _| {
        if seen.method == "OPTIONS" {
            return empty(200).into();
        }
        let body = match seen.path.as_str() {
            "/dav/" if seen.body.contains("current-user-principal") => {
                principal_answer("/dav/principals/alice/")
            }
            "/dav/principals/alice/" => principal.to_owned(),
            _ => return empty(404).into(),
        };
        xml(207, &body).into()
    })
    .await
}

/// A 207 without the principal, or without any home set in a 2xx
/// propstat, is an error: taken as "no homes", every collection would be
/// hidden (and later purged).
#[tokio::test]
async fn discovery_without_the_principal_is_an_error() {
    let others = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/dav/principals/bob/</href><propstat><prop><C:calendar-home-set><href>/dav/cal/bob/</href></C:calendar-home-set></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/dav/principals/carol/</href><status>HTTP/1.1 404 Not Found</status></response></multistatus>"#;
    let failed = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:R="urn:ietf:params:xml:ns:carddav"><response><href>/dav/principals/alice/</href><propstat><prop><C:calendar-home-set/><R:addressbook-home-set/></prop><status>HTTP/1.1 500 Internal Server Error</status></propstat></response></multistatus>"#;
    let only_cards = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:R="urn:ietf:params:xml:ns:carddav"><response><href>/dav/principals/alice/</href><propstat><prop><R:addressbook-home-set><href>/dav/card/alice/</href></R:addressbook-home-set></prop><status>HTTP/1.1 200 OK</status></propstat><propstat><prop><C:calendar-home-set/></prop><status>HTTP/1.1 404 Not Found</status></propstat></response></multistatus>"#;
    for answer in [others, failed] {
        let fake = discovery_fake(answer).await;
        let result = client(&fake).discover().await;
        assert!(matches!(result, Err(DavError::Protocol(_))), "{result:?}");
    }
    let fake = discovery_fake(only_cards).await;
    let found = client(&fake).discover().await.unwrap();
    assert!(found.calendar_homes.is_empty());
    assert_eq!(found.addressbook_homes.len(), 1);
}

/// Nextcloud's trash bin with events too large to send with their data
/// together: asked again without it, the events are still listed.
#[tokio::test]
async fn a_big_trash_bin_is_listed_without_data() {
    let pad = "p".repeat(70 * 1024);
    let fake = Fake::start(move |seen, _| {
        let body = match (seen.method.as_str(), seen.path.as_str()) {
            ("REPORT", "/dav/cal/alice/trashbin/objects/") if seen.body.contains("calendar-data") => {
                format!("<!--{pad}--><multistatus xmlns=\"DAV:\"/>")
            }
            ("REPORT", "/dav/cal/alice/trashbin/objects/") => r#"<multistatus xmlns="DAV:" xmlns:NC="http://nextcloud.com/ns"><response><href>/dav/cal/alice/trashbin/objects/1.ics</href><propstat><prop><getetag>"1"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/dav/cal/alice/trashbin/objects/2.ics</href><propstat><prop><getetag>"2"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#.to_owned(),
            _ => EMPTY_MULTISTATUS.to_owned(),
        };
        xml(207, &body).into()
    })
    .await;
    let client = client(&fake);
    let home = href(&client, "/dav/cal/alice/");
    let bin = client
        .trash_bin(&home, TrashContents::WithData)
        .await
        .unwrap();
    assert_eq!(bin.events.len(), 2);
    assert!(bin.events.iter().all(|event| event.data.is_none()));
    let reports: Vec<bool> = fake
        .seen()
        .iter()
        .filter(|s| s.method == "REPORT")
        .map(|s| s.body.contains("calendar-data"))
        .collect();
    assert_eq!(reports, [true, false]);
    // Restoring or deleting needs only the hrefs: one small REPORT.
    let before = fake.seen().len();
    let bin = client
        .trash_bin(&home, TrashContents::HrefsOnly)
        .await
        .unwrap();
    assert_eq!(bin.events.len(), 2);
    let reports: Vec<bool> = fake.seen()[before..]
        .iter()
        .filter(|s| s.method == "REPORT")
        .map(|s| s.body.contains("calendar-data"))
        .collect();
    assert_eq!(reports, [false]);
}

/// Namespace and attribute bombs stop before roxmltree, which is
/// quadratic in them: in a free-busy answer, and in the `DAV:error` body
/// of a 401 (read for any error status), which stays `Unauthorized`.
#[tokio::test]
async fn namespace_and_attribute_bombs() {
    let nested: String = (0..10_000).fold(String::new(), |mut out, i| {
        let _ = write!(out, "<a xmlns:b{i}=\"u\">");
        out
    });
    let schedule = format!(
        r#"<C:schedule-response xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{nested}"#
    );
    let error = format!(
        r#"<D:error xmlns:D="DAV:">{}"#,
        nested.get(..64 * 1024).unwrap_or(&nested)
    );
    let fake = Fake::start(move |seen, _| {
        if seen.path == "/dav/outbox/" {
            return xml(200, &schedule).into();
        }
        Response::builder()
            .status(401)
            .header("www-authenticate", "Basic realm=\"x\"")
            .header("content-type", "application/xml")
            .body(full(error.clone()))
            .unwrap()
            .into()
    })
    .await;
    let client = client(&fake);
    let started = std::time::Instant::now();
    let answer = client
        .free_busy(
            &href(&client, "/dav/outbox/"),
            &free_busy_request(&["bob@example.com"]),
        )
        .await;
    assert!(
        matches!(
            answer,
            Err(DavError::Xml(
                ParseError::TooDeep | ParseError::TooManyNamespaces
            ))
        ),
        "{answer:?}"
    );
    let unauthorized = client
        .get(&href(&client, "/dav/a.ics"), ResourceKind::Calendar)
        .await;
    assert!(
        matches!(unauthorized, Err(DavError::Unauthorized)),
        "{unauthorized:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// A home of `count` calendars the user may share (`write-acl`), each
/// with `aces[i]` ACEs; the listing with `DAV:acl` is too big for the
/// limits, so it is split. `c{copy}` is a Calendar Server copy if given.
fn split_home(aces: Vec<usize>, copy: Option<usize>) -> impl Fn(&Seen, usize) -> Script {
    move |seen: &Seen, _| {
        let calendar = |i: usize, sharing: &str| {
            let shared = if copy == Some(i) { "<CS:shared/>" } else { "" };
            format!(
                r"<response><href>/dav/cal/alice/c{i}/</href><propstat><prop><resourcetype><collection/><C:calendar/>{shared}</resourcetype><current-user-privilege-set><privilege><all/></privilege></current-user-privilege-set>{sharing}</prop><status>HTTP/1.1 200 OK</status></propstat></response>"
            )
        };
        let acl = |count: usize| {
            let mut out = String::from("<acl>");
            for n in 0..count {
                let _ = write!(
                    out,
                    "<ace><principal><href>/dav/pal/u{n}/</href></principal><grant><privilege><read/></privilege></grant></ace>"
                );
            }
            out.push_str("</acl>");
            out
        };
        let wrap = |inner: &str| {
            format!(
                r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">{inner}</multistatus>"#
            )
        };
        if seen.method != "PROPFIND" {
            return empty(404).into();
        }
        let asks_acl = seen.body.contains("acl/>");
        if seen.path == "/dav/cal/alice/" {
            let mut inner = String::new();
            for (i, count) in aces.iter().enumerate() {
                let sharing = if asks_acl { acl(*count) } else { String::new() };
                inner.push_str(&calendar(i, &sharing));
            }
            return xml(207, &wrap(&inner)).into();
        }
        let Some(i) = seen
            .path
            .strip_prefix("/dav/cal/alice/c")
            .and_then(|rest| rest.trim_end_matches('/').parse::<usize>().ok())
            .filter(|i| *i < aces.len())
        else {
            return empty(404).into();
        };
        xml(
            207,
            &wrap(&format!(
                r"<response><href>/dav/cal/alice/c{i}/</href><propstat><prop>{}</prop><status>HTTP/1.1 200 OK</status></propstat></response>",
                acl(aces[i])
            )),
        )
        .into()
    }
}

fn depth_zero(fake: &Fake) -> usize {
    fake.seen()
        .iter()
        .filter(|s| s.method == "PROPFIND" && s.path != "/dav/cal/alice/")
        .count()
}

/// A home whose listing with sharing is too big is listed without it,
/// and each collection's sharing is asked for apart, within the budget:
/// at most `max_collections` requests, at most `share_items` ACEs kept
/// (first fit), and one too big on its own is marked so, not a failure.
#[tokio::test]
async fn split_listing_budget() {
    use nebula_dav::{ListOptions, SharingData};
    // 8 calendars with 100 ACEs each (about 10 KB each, 80 KB in all):
    // the whole listing passes 64 KiB.
    let fake = Fake::start(split_home(vec![100; 8], None)).await;
    let mut limits = fast_limits();
    limits.max_collections = 5;
    let client = client_for(config(&fake).with_limits(limits), Dns::new(&[]));
    let home = href(&client, "/dav/cal/alice/");
    let listing = client
        .list_home(&home, ListOptions::default())
        .await
        .unwrap();
    assert!(listing.split);
    assert_eq!(listing.collections.len(), 8);
    // By href: c0..c4 read, the rest not (requests spent).
    let fetched: Vec<bool> = listing
        .collections
        .iter()
        .map(|c| c.sharing_data == SharingData::Fetched)
        .collect();
    assert_eq!(fetched, [true, true, true, true, true, false, false, false]);
    assert_eq!(listing.collections[0].acl.as_ref().unwrap().aces.len(), 100);
    assert!(listing.collections[7].acl.is_none());
    assert_eq!(depth_zero(&fake), 5);
    // Straight to the split listing: the whole one is not asked again.
    let before = fake.seen().len();
    let options = ListOptions {
        split: true,
        share_items: 250,
    };
    let listing = client.list_home(&home, options).await.unwrap();
    let whole = fake.seen()[before..]
        .iter()
        .filter(|s| s.path == "/dav/cal/alice/" && s.body.contains("acl/>"))
        .count();
    assert_eq!(whole, 0);
    // 250 items: two of 100 fit, the others are not kept.
    let kept = listing
        .collections
        .iter()
        .filter(|c| c.sharing_data == SharingData::Fetched)
        .count();
    assert_eq!(kept, 2);
    assert!(
        listing
            .collections
            .iter()
            .filter(|c| c.sharing_data != SharingData::Fetched)
            .all(|c| c.acl.is_none())
    );
}

/// One collection whose sharing is too big on its own is marked
/// `TooLarge`; the others are still read, and a Calendar Server copy is
/// asked first and does not count in `share_items`.
#[tokio::test]
async fn split_listing_too_large_and_copies() {
    use nebula_dav::{ListOptions, SharingData};
    // c0: 700 ACEs (about 70 KB, over 64 KiB); c1..c3: 50 each; c3 a copy.
    let fake = Fake::start(split_home(vec![700, 50, 50, 50], Some(3))).await;
    let client = client_for(config(&fake), Dns::new(&[]));
    let home = href(&client, "/dav/cal/alice/");
    let options = ListOptions {
        split: false,
        share_items: 100,
    };
    let listing = client.list_home(&home, options).await.unwrap();
    assert!(listing.split);
    let data: Vec<SharingData> = listing.collections.iter().map(|c| c.sharing_data).collect();
    assert_eq!(
        data,
        [
            SharingData::TooLarge,
            SharingData::Fetched,
            SharingData::Fetched,
            SharingData::Fetched
        ]
    );
    // The copy was asked first.
    let first = fake
        .seen()
        .into_iter()
        .find(|s| s.path != "/dav/cal/alice/")
        .unwrap();
    assert_eq!(first.path, "/dav/cal/alice/c3/");
}

/// A change in several `POST`s is checked whole before the first: a
/// principal of another origin anywhere stops all of them.
#[tokio::test]
async fn share_is_checked_whole() {
    let fake = Fake::start(|_, _| empty(200).into()).await;
    let client = client(&fake);
    let work = href(&client, "/dav/cal/work/");
    let here = Sharee::Principal(href(&client, "/dav/pal/bob/"));
    let elsewhere = Sharee::Principal(
        Href::from_url(&url::Url::parse("https://other.example/dav/pal/bob/").unwrap()).unwrap(),
    );
    let mut set = vec![(here, ShareAccess::Read); 300];
    assert!(client.check_share(&work, &set, &[]).is_ok());
    set.push((elsewhere.clone(), ShareAccess::Read));
    assert!(matches!(
        client.check_share(&work, &set, &[]),
        Err(DavError::OriginNotAllowed)
    ));
    assert!(matches!(
        client.check_share(&work, &[], &[elsewhere]),
        Err(DavError::OriginNotAllowed)
    ));
    assert!(fake.seen().is_empty());
}

/// The answers of a split listing together stay within twice
/// `max_multistatus`: past that, collections are left without sharing.
#[tokio::test]
async fn split_listing_byte_budget() {
    use nebula_dav::{ListOptions, SharingData};
    // 12 calendars of 300 ACEs, about 30 KB each (360 KB in all): only
    // about 128 KiB of them are read.
    let fake = Fake::start(split_home(vec![300; 12], None)).await;
    let client = client_for(config(&fake), Dns::new(&[]));
    let home = href(&client, "/dav/cal/alice/");
    let listing = client
        .list_home(&home, ListOptions::default())
        .await
        .unwrap();
    let fetched = listing
        .collections
        .iter()
        .filter(|c| c.sharing_data == SharingData::Fetched)
        .count();
    assert!((2..=5).contains(&fetched), "{fetched}");
    let read: usize = fake
        .seen()
        .iter()
        .filter(|s| s.path != "/dav/cal/alice/")
        .count();
    // Past the budget nothing more is asked.
    assert!(read <= fetched + 1, "{read} {fetched}");
}
