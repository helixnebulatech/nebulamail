// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The client: one request at a time per call, redirects by hand, the
//! limits of [`Limits`](crate::Limits), and one idle connection kept for
//! the next request (reused only within
//! [`Limits::idle_timeout`](crate::Limits::idle_timeout): a connection a
//! NAT dropped silently would look alive). A request that fails on that
//! connection is sent again on a new one only if it was never written,
//! or if it only reads. A read answered 503 with `Retry-After` is repeated a
//! few times (Stalwart 1.0 says so when its memory budget runs out);
//! nothing that writes is. A 429 with `Retry-After` is repeated whatever
//! the method: the server refused it without doing it (RFC 6585).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, IF_MATCH,
    IF_NONE_MATCH, LOCATION, RETRY_AFTER,
};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use nebula_dav_proto::{Depth, ETag, Href, HrefError, parse_error_body};
use nebula_tls::TlsConnector;
use net_guard::{NetworkAccess, Resolver, SystemResolver};
use tokio::time::Instant;

use crate::config::{Credentials, DavServer, Origin};
use crate::core::body::read_body;
use crate::core::connection::{Connector, Sender};
use crate::scope::{AccountHost, Trust};
use crate::{DavError, RedirectError};

/// A CalDAV/CardDAV client for one account.
///
/// Requests only go to the server's origin, the account's iCloud host
/// and the origins the admin allowed, each connection through
/// `net-guard`. The credentials only go to the server's own origin and
/// the account's iCloud host ([`DavServer::trust`]).
pub struct DavClient {
    server: DavServer,
    /// Changed in place when the user logs in again with a new password
    /// ([`Self::set_credentials`]); read (not held) for each request.
    credentials: Mutex<Option<Credentials>>,
    connector: Connector,
    /// The idle connection, its origin and since when it is unused.
    idle: Mutex<Option<(Origin, Sender, Instant)>>,
    /// The account's own iCloud host, as the last discovery found it
    /// (only with [`DavServer::account_hosts`]).
    account_host: Mutex<Option<Origin>>,
    /// The server announced `nc-calendar-webcal-cache` (Nextcloud): its
    /// requests carry [`WEBCAL_CACHING`], so subscriptions are served as
    /// read-only calendars with Nextcloud's cached copy of their events.
    webcal_cache: AtomicBool,
}

/// Nextcloud: serve subscriptions as calendars with their cached events
/// (sent only to a server that announced `nc-calendar-webcal-cache`).
const WEBCAL_CACHING: &str = "x-nc-caldav-webcal-caching";
/// Nextcloud: delete for good, without the trash bin.
const NO_TRASHBIN: &str = "x-nc-caldav-no-trashbin";

/// Times one read is sent again after a 503.
const MAX_BUSY_RETRIES: u8 = 2;
/// Longest `Retry-After` waited for (Stalwart asks for 1 or 5 s). A longer
/// one is not shortened: the 503 goes to the caller, who tries later.
const MAX_BUSY_WAIT: Duration = Duration::from_secs(10);
/// The same for a 429 (Stalwart's limit per IP lasts up to its window).
const MAX_LIMITED_WAIT: Duration = Duration::from_secs(30);
/// Shortest wait, so `Retry-After: 0` is not an immediate repeat.
const MIN_BUSY_WAIT: Duration = Duration::from_millis(100);

impl std::fmt::Debug for DavClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavClient")
            .field("server", &self.server.origin)
            .field(
                "credentials",
                &*self
                    .credentials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            )
            .finish_non_exhaustive()
    }
}

/// One request, before redirects.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches of one request, each its own header or rule"
)]
pub(crate) struct Call<'a> {
    pub(crate) method: Method,
    pub(crate) href: &'a Href,
    pub(crate) depth: Option<Depth>,
    pub(crate) body: Option<(Bytes, &'static str)>,
    pub(crate) if_match: Option<&'a ETag>,
    pub(crate) if_none_match_any: bool,
    pub(crate) destination: Option<&'a Href>,
    pub(crate) overwrite: Option<bool>,
    pub(crate) accept: Option<&'static str>,
    /// Largest body of a successful answer.
    pub(crate) limit: usize,
    /// Sent by discovery: any host of the server's account-host pattern
    /// is the account's own (iCloud).
    pub(crate) discovering: bool,
    /// Without [`WEBCAL_CACHING`] even if the server takes it (to see
    /// subscriptions as such, with their source).
    pub(crate) plain_subscriptions: bool,
    /// With `X-NC-CalDAV-No-Trashbin: 1` (Nextcloud: delete for good).
    pub(crate) no_trashbin: bool,
}

impl<'a> Call<'a> {
    pub(crate) fn new(method: Method, href: &'a Href, limit: usize) -> Self {
        Self {
            method,
            href,
            depth: None,
            body: None,
            if_match: None,
            if_none_match_any: false,
            destination: None,
            overwrite: None,
            accept: None,
            limit,
            discovering: false,
            plain_subscriptions: false,
            no_trashbin: false,
        }
    }

    pub(crate) fn depth(mut self, depth: Depth) -> Self {
        self.depth = Some(depth);
        self
    }

    pub(crate) fn body(mut self, body: impl Into<Bytes>, content_type: &'static str) -> Self {
        self.body = Some((body.into(), content_type));
        self
    }
}

/// The final answer, after redirects.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
    /// Where the request finally went (hrefs in the body are relative to
    /// it).
    pub(crate) url: Href,
}

impl Reply {
    /// The reply as an error, for a status the operation does not expect.
    pub(crate) fn into_error(self) -> DavError {
        status_error(self.status, &self.body)
    }

    /// 2xx, or the error for its status.
    pub(crate) fn success(self) -> Result<Self, DavError> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(self.into_error())
        }
    }

    /// The `ETag` header, if it is a valid entity tag.
    pub(crate) fn etag(&self) -> Option<ETag> {
        self.headers
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .and_then(ETag::parse)
    }

    /// The media type without parameters, lowercase.
    pub(crate) fn media_type(&self) -> Option<String> {
        let value = self.headers.get(CONTENT_TYPE)?.to_str().ok()?;
        let media = value.split(';').next().unwrap_or_default().trim();
        Some(media.to_ascii_lowercase())
    }

    /// A multistatus must be XML when the server says what it is.
    pub(crate) fn expect_xml(&self) -> Result<(), DavError> {
        match self.media_type() {
            None => Ok(()),
            Some(media)
                if media == "application/xml" || media == "text/xml" || media.ends_with("+xml") =>
            {
                Ok(())
            }
            Some(_) => Err(DavError::ContentType),
        }
    }
}

pub(crate) fn status_error(status: StatusCode, body: &[u8]) -> DavError {
    let precondition = parse_error_body(body);
    match status.as_u16() {
        401 => DavError::Unauthorized,
        403 => DavError::Forbidden { precondition },
        404 | 410 => DavError::NotFound,
        405 => DavError::MethodNotAllowed,
        409 => DavError::Conflict { precondition },
        412 => DavError::PreconditionFailed,
        507 => DavError::InsufficientStorage,
        status => DavError::Status {
            status,
            precondition,
        },
    }
}

/// Whether sending the method twice is harmless: it only reads. PUT and
/// DELETE are idempotent, but with `If-Match` a repeat of one that did go
/// through would fail with 412; MOVE and POST are not repeated either.
fn only_reads(method: &Method) -> bool {
    *method == Method::GET
        || *method == Method::HEAD
        || *method == Method::OPTIONS
        || matches!(method.as_str(), "PROPFIND" | "REPORT")
}

/// How long a 503 or a 429 asks to wait. Only delay-seconds: a date depends
/// on the other server's clock, and Stalwart doesn't send it.
fn busy_wait(status: StatusCode, headers: &HeaderMap) -> Option<Duration> {
    let longest = match status {
        StatusCode::SERVICE_UNAVAILABLE => MAX_BUSY_WAIT,
        StatusCode::TOO_MANY_REQUESTS => MAX_LIMITED_WAIT,
        _ => return None,
    };
    let seconds: u64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let wait = Duration::from_secs(seconds);
    (wait <= longest).then(|| wait.max(MIN_BUSY_WAIT))
}

const fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// The next location of a redirect, checked like an href.
fn redirect_target(headers: &HeaderMap, current: &Href) -> Result<Href, DavError> {
    let location = headers
        .get(LOCATION)
        .ok_or(DavError::Redirect(RedirectError::MissingLocation))?
        .to_str()
        .map_err(|_| DavError::Redirect(RedirectError::InvalidLocation(HrefError::Character)))?;
    // `file:`, `gopher:`, `javascript:`… are a scheme change, whatever
    // else is wrong with them.
    if let Some((scheme, _)) = location.trim().split_once(':')
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && !scheme.eq_ignore_ascii_case("http")
        && !scheme.eq_ignore_ascii_case("https")
    {
        return Err(DavError::Redirect(RedirectError::Scheme));
    }
    Href::resolve(location, current)
        .map_err(|error| DavError::Redirect(RedirectError::InvalidLocation(error)))
}

/// `host[:port]` for the `Host` header (brackets for IPv6, no default
/// port).
fn authority(href: &Href) -> Result<HeaderValue, DavError> {
    let url = href.url();
    let host = url.host_str().ok_or(DavError::Href(HrefError::Url))?;
    let text = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    HeaderValue::from_str(&text).map_err(|_| DavError::Href(HrefError::Url))
}

impl DavClient {
    /// A client for `server` with the system resolver.
    ///
    /// # Errors
    ///
    /// The TLS configuration or the system DNS configuration could not be
    /// loaded.
    pub fn new(server: DavServer, credentials: Option<Credentials>) -> Result<Self, DavError> {
        let resolver = SystemResolver::new()
            .map_err(|_| DavError::Guard(net_guard::BlockReason::Dns.into()))?;
        Self::with_resolver(server, credentials, Arc::new(resolver))
    }

    /// A client with another resolver (tests use a fake DNS).
    ///
    /// # Errors
    ///
    /// The TLS configuration could not be built.
    pub fn with_resolver(
        server: DavServer,
        credentials: Option<Credentials>,
        resolver: Arc<dyn Resolver>,
    ) -> Result<Self, DavError> {
        let tls = TlsConnector::new(server.certificates, &[b"http/1.1"])?;
        let connector = Connector::new(tls, resolver, server.limits.connect_timeout);
        Ok(Self {
            server,
            credentials: Mutex::new(credentials),
            connector,
            idle: Mutex::new(None),
            account_host: Mutex::new(None),
            webcal_cache: AtomicBool::new(false),
        })
    }

    /// Replaces the login sent from the next request on (a new password
    /// after the user logged in again), or removes it (`None`: the account
    /// is closing, nothing in flight may carry it any more). Requests
    /// already sent keep the header they had.
    pub fn set_credentials(&self, credentials: Option<Credentials>) {
        *self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = credentials;
    }

    /// Whether requests ask Nextcloud for its cached subscriptions (set by
    /// discovery from `nc-calendar-webcal-cache`).
    #[must_use]
    pub fn webcal_cache(&self) -> bool {
        self.webcal_cache.load(Ordering::Relaxed)
    }

    /// Sets [`DavClient::webcal_cache`] (discovery; tests).
    pub fn set_webcal_cache(&self, on: bool) {
        self.webcal_cache.store(on, Ordering::Relaxed);
    }

    /// The server.
    #[must_use]
    pub const fn server(&self) -> &DavServer {
        &self.server
    }

    /// The account's own iCloud host, once discovery found it.
    #[must_use]
    pub fn account_host(&self) -> Option<Origin> {
        self.account_host
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Remembers the account's iCloud host (discovery). Only a host of the
    /// server's pattern is kept.
    pub(crate) fn set_account_host(&self, host: Option<Origin>) {
        let host = host.filter(|origin| {
            self.server
                .account_hosts()
                .is_some_and(|rule| rule.matches(origin))
        });
        *self
            .account_host
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = host;
    }

    /// What may go to `origin` now.
    pub(crate) fn trust(&self, origin: &Origin, discovering: bool) -> Option<Trust> {
        if discovering {
            return self.server.trust(origin, AccountHost::Discovering);
        }
        let known = self.account_host();
        self.server
            .trust(origin, AccountHost::Known(known.as_ref()))
    }

    /// The href may be used: on the server's origin, the account's iCloud
    /// host or an allowed origin.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`].
    pub fn check_origin(&self, href: &Href) -> Result<(), DavError> {
        self.check(href, false)
    }

    pub(crate) fn check(&self, href: &Href, discovering: bool) -> Result<(), DavError> {
        let origin = Origin::of(href.url())?;
        self.trust(&origin, discovering)
            .map(|_| ())
            .ok_or(DavError::OriginNotAllowed)
    }

    /// Whether `href` is on an origin that gets the login: the configured
    /// one or the account's iCloud host (not an allowed host).
    #[must_use]
    pub fn gets_login(&self, href: &Href) -> bool {
        Origin::of(href.url())
            .ok()
            .and_then(|origin| self.trust(&origin, false))
            .is_some_and(Trust::sends_login)
    }

    /// Sends a call, following redirects, and reads the answer's body.
    /// The retries share the call's time limit.
    pub(crate) async fn send(&self, call: &Call<'_>) -> Result<Reply, DavError> {
        let deadline = Instant::now() + self.server.limits.request_timeout;
        tokio::time::timeout_at(deadline, self.send_retrying(call, deadline))
            .await
            .map_err(|_| DavError::Timeout)?
    }

    /// [`Self::send_until`], again while a read is answered 503, or anything
    /// 429, with a `Retry-After` that can be waited for before the deadline.
    /// Then the last answer counts.
    async fn send_retrying(&self, call: &Call<'_>, deadline: Instant) -> Result<Reply, DavError> {
        let mut retries = 0_u8;
        loop {
            let reply = self.send_until(call, deadline).await?;
            let refused = reply.status == StatusCode::TOO_MANY_REQUESTS;
            if retries >= MAX_BUSY_RETRIES || !(refused || only_reads(&call.method)) {
                return Ok(reply);
            }
            let Some(wait) = busy_wait(reply.status, &reply.headers) else {
                return Ok(reply);
            };
            let until = Instant::now() + wait;
            if until >= deadline {
                return Ok(reply);
            }
            retries += 1;
            tracing::debug!(method = %call.method, "dav: server busy, trying again");
            tokio::time::sleep_until(until).await;
        }
    }

    async fn send_until(&self, call: &Call<'_>, deadline: Instant) -> Result<Reply, DavError> {
        let limits = self.server.limits;
        let mut url = call.href.clone();
        let method = call.method.clone();
        let body = call.body.clone();
        let mut redirects = 0_u8;
        loop {
            let origin = Origin::of(url.url())?;
            let trust = self
                .trust(&origin, call.discovering)
                .ok_or(DavError::OriginNotAllowed)?;
            let build = || self.request(call, &method, &url, trust, body.as_ref());
            let (response, sender) = self
                .round_trip(&origin, trust.network(), &method, build)
                .await?;
            let status = response.status();
            if is_redirect(status) {
                let next = redirect_target(response.headers(), &url);
                // The body is not read: dropping it closes that connection.
                drop(response);
                drop(sender);
                if redirects >= limits.max_redirects {
                    return Err(DavError::Redirect(RedirectError::TooMany));
                }
                redirects += 1;
                url = next?;
                // A POST (the free-busy request to the outbox) only follows
                // 307/308, which repeat it as it is; 301/302/303 would turn
                // it into a GET elsewhere (browsers do), which is not the
                // same request.
                if method == Method::POST
                    && status != StatusCode::TEMPORARY_REDIRECT
                    && status != StatusCode::PERMANENT_REDIRECT
                {
                    return Err(DavError::Redirect(RedirectError::Method));
                }
                // 303 means "GET it there": only for a GET or HEAD, which
                // it repeats as it is. Any other method would become a GET
                // whose 2xx looks like the write done. 301/302/307/308
                // repeat the request, method and body.
                if status == StatusCode::SEE_OTHER
                    && method != Method::GET
                    && method != Method::HEAD
                {
                    return Err(DavError::Redirect(RedirectError::Method));
                }
                tracing::debug!(status = status.as_u16(), "dav: following a redirect");
                continue;
            }
            let (parts, incoming) = response.into_parts();
            let success = parts.status.is_success();
            let limit = if success {
                call.limit
            } else {
                limits.max_error_body
            };
            let body = match self.read(incoming, &parts.headers, limit, deadline).await {
                Ok(body) => {
                    self.keep_idle(&origin, sender, &parts.headers);
                    body
                }
                // An error page too big to read: the status says enough.
                Err(DavError::TooLarge) if !success => Bytes::new(),
                Err(error) => return Err(error),
            };
            tracing::debug!(method = %method, status = parts.status.as_u16(), "dav: answer");
            return Ok(Reply {
                status: parts.status,
                headers: parts.headers,
                body,
                url,
            });
        }
    }

    async fn read(
        &self,
        incoming: Incoming,
        headers: &HeaderMap,
        limit: usize,
        deadline: Instant,
    ) -> Result<Bytes, DavError> {
        read_body(
            incoming,
            headers,
            limit,
            self.server.limits.idle_timeout,
            deadline,
        )
        .await
    }

    fn request(
        &self,
        call: &Call<'_>,
        method: &Method,
        url: &Href,
        trust: Trust,
        body: Option<&(Bytes, &'static str)>,
    ) -> Result<Request<Full<Bytes>>, DavError> {
        let mut builder = Request::builder()
            .method(method.clone())
            .uri(url.path())
            .header(HOST, authority(url)?)
            .header(ACCEPT_ENCODING, "identity");
        let headers = builder
            .headers_mut()
            .ok_or(DavError::Protocol("request builder"))?;
        // Credentials only to the server's own origin (and the account's
        // iCloud host), never to an allowed one or after a redirect
        // elsewhere.
        if trust.sends_login() {
            // Built under the lock, which is not held across an await.
            let header = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(Credentials::header)
                .transpose()?;
            if let Some(header) = header {
                headers.insert(AUTHORIZATION, header);
            }
        }
        if let Some(depth) = call.depth {
            headers.insert("depth", HeaderValue::from_static(depth.as_str()));
        }
        // Nextcloud's own headers, only to the server that announced them
        // (its own origin), never to an allowed host.
        if trust.sends_login() {
            if self.webcal_cache() && !call.plain_subscriptions {
                headers.insert(WEBCAL_CACHING, HeaderValue::from_static("On"));
            }
            if call.no_trashbin {
                headers.insert(NO_TRASHBIN, HeaderValue::from_static("1"));
            }
        }
        if let Some(etag) = call.if_match {
            // Only a strong tag: a weak one never matches (RFC 9110
            // §13.1.1), the write would fail with 412 again and again.
            // The callers re-read the resource for a strong one first.
            let tag = etag.if_match().ok_or(DavError::Protocol("weak etag"))?;
            let value = HeaderValue::from_str(&tag).map_err(|_| DavError::Protocol("etag"))?;
            headers.insert(IF_MATCH, value);
        }
        if call.if_none_match_any {
            headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
        }
        if let Some(destination) = call.destination {
            let value = HeaderValue::from_str(destination.url().as_str())
                .map_err(|_| DavError::Href(HrefError::Url))?;
            headers.insert("destination", value);
        }
        if let Some(overwrite) = call.overwrite {
            headers.insert(
                "overwrite",
                HeaderValue::from_static(if overwrite { "T" } else { "F" }),
            );
        }
        if let Some(accept) = call.accept {
            headers.insert(ACCEPT, HeaderValue::from_static(accept));
        }
        let content = match body {
            Some((bytes, content_type)) => {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
                bytes.clone()
            }
            None => Bytes::new(),
        };
        builder
            .body(Full::new(content))
            .map_err(|_| DavError::Protocol("invalid request"))
    }

    /// Sends one request on the idle connection (if it goes to the same
    /// origin) or a new one. A request that fails on a reused connection
    /// (the server closed it meanwhile) is sent once more on a new one if
    /// it was never written, whatever the method, or if it only reads.
    /// One that was written may have been done: a PUT repeated would fail
    /// with 412, a MKCOL with 405, a DELETE with 404, so the error goes to
    /// the caller.
    async fn round_trip<F>(
        &self,
        origin: &Origin,
        network: NetworkAccess,
        method: &Method,
        build: F,
    ) -> Result<(http::Response<Incoming>, Sender), DavError>
    where
        F: Fn() -> Result<Request<Full<Bytes>>, DavError>,
    {
        let mut unsent = None;
        if let Some(mut sender) = self.take_idle(origin) {
            match send_on(&mut sender, build()?).await {
                Ok(response) => {
                    check_head(&response)?;
                    return Ok((response, sender));
                }
                Err(SendFailure::NotSent(request, _)) => {
                    tracing::debug!("dav: idle connection closed, opening a new one");
                    unsent = Some(request);
                }
                Err(SendFailure::Sent(error)) if !only_reads(method) => return Err(error),
                Err(SendFailure::Sent(_)) => {
                    tracing::debug!("dav: idle connection failed, reading again on a new one");
                }
            }
        }
        let mut sender = self.connector.open(origin, network).await?;
        let request = match unsent {
            Some(request) => *request,
            None => build()?,
        };
        let response = send_on(&mut sender, request)
            .await
            .map_err(SendFailure::into_error)?;
        check_head(&response)?;
        Ok((response, sender))
    }

    /// The idle connection, if it goes to `origin`, is open and was used
    /// less than [`Limits::idle_timeout`](crate::Limits::idle_timeout) ago.
    /// An older one is dropped (closed).
    fn take_idle(&self, origin: &Origin) -> Option<Sender> {
        let mut idle = self.idle.lock().ok()?;
        match idle.take() {
            Some((idle_origin, sender, parked))
                if idle_origin == *origin
                    && !sender.is_closed()
                    && parked.elapsed() < self.server.limits.idle_timeout =>
            {
                Some(sender)
            }
            _ => None,
        }
    }

    fn keep_idle(&self, origin: &Origin, sender: Sender, headers: &HeaderMap) {
        let close = headers
            .get(CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("close"));
        if close || sender.is_closed() {
            return;
        }
        if let Ok(mut idle) = self.idle.lock() {
            *idle = Some((origin.clone(), sender, Instant::now()));
        }
    }
}

/// Why a request on a connection failed.
enum SendFailure {
    /// Nothing of it was written (the connection was closed before): the
    /// request comes back, to send on another connection.
    NotSent(Box<Request<Full<Bytes>>>, DavError),
    /// It was written, maybe done by the server.
    Sent(DavError),
}

impl SendFailure {
    fn into_error(self) -> DavError {
        match self {
            Self::NotSent(_, error) | Self::Sent(error) => error,
        }
    }
}

async fn send_on(
    sender: &mut Sender,
    request: Request<Full<Bytes>>,
) -> Result<http::Response<Incoming>, SendFailure> {
    if let Err(error) = sender.ready().await {
        return Err(SendFailure::NotSent(Box::new(request), error.into()));
    }
    sender
        .try_send_request(request)
        .await
        .map_err(|mut error| match error.take_message() {
            Some(request) => SendFailure::NotSent(Box::new(request), error.into_error().into()),
            None => SendFailure::Sent(error.into_error().into()),
        })
}

/// hyper's `max_buf_size` only bounds the read buffer, and one read can
/// bring a whole head bigger than it (seen in tests: a 100 KiB header
/// accepted now and then), so the limit is checked here too. Never a
/// reason to send again: the answer came.
fn check_head(response: &http::Response<Incoming>) -> Result<(), DavError> {
    let head: usize = response
        .headers()
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len() + 4)
        .sum();
    if head > super::connection::MAX_HEAD_BYTES {
        return Err(DavError::HeadTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_after(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn only_reads_are_repeated() {
        for method in ["GET", "HEAD", "OPTIONS", "PROPFIND", "REPORT"] {
            assert!(only_reads(&Method::from_bytes(method.as_bytes()).unwrap()));
        }
        for method in [
            "PUT",
            "DELETE",
            "POST",
            "MOVE",
            "MKCALENDAR",
            "MKCOL",
            "PROPPATCH",
            "ACL",
        ] {
            assert!(!only_reads(&Method::from_bytes(method.as_bytes()).unwrap()));
        }
    }

    #[test]
    fn busy_wait_reads_seconds_within_the_limit() {
        let busy = StatusCode::SERVICE_UNAVAILABLE;
        let seconds = |n| Some(Duration::from_secs(n));
        assert_eq!(busy_wait(busy, &retry_after("5")), seconds(5));
        assert_eq!(busy_wait(busy, &retry_after(" 1 ")), seconds(1));
        assert_eq!(busy_wait(busy, &retry_after("10")), seconds(10));
        assert_eq!(busy_wait(busy, &retry_after("0")), Some(MIN_BUSY_WAIT));
        assert_eq!(busy_wait(busy, &retry_after("11")), None);
        assert_eq!(busy_wait(busy, &retry_after("99999999999999999999")), None);
        assert_eq!(busy_wait(busy, &retry_after("-1")), None);
        assert_eq!(
            busy_wait(busy, &retry_after("Sat, 31 Oct 2026 10:00:00 GMT")),
            None
        );
        assert_eq!(busy_wait(busy, &HeaderMap::new()), None);
        for status in [StatusCode::OK, StatusCode::BAD_GATEWAY] {
            assert_eq!(busy_wait(status, &retry_after("1")), None);
        }
        let limited = StatusCode::TOO_MANY_REQUESTS;
        assert_eq!(busy_wait(limited, &retry_after("30")), seconds(30));
        assert_eq!(busy_wait(limited, &retry_after("31")), None);
    }
}
