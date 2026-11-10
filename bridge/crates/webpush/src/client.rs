// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sends one push message (RFC 8030 §5) to an endpoint: encrypted
//! (RFC 8291), signed (VAPID, RFC 8292), over a connection made by
//! `net-guard` (every resolved address checked, the connection pinned to
//! one, no proxy), TLS with the platform verifier, HTTP/1.1.
//!
//! - **No redirects**: a 3xx is a refusal; push services never redirect.
//! - **Limits**: [`Limits`] (connection 5 s, whole request 10 s, response
//!   body read up to 1 KiB and dropped, never logged).
//! - **Outcome** ([`Outcome`]): 2xx delivered; 404/410 the subscription is
//!   gone; 429 throttled (with `Retry-After`); other 4xx (and 3xx)
//!   refused for good; 5xx and network failures are temporary.
//!
//! Logs carry the host and the status, never the URL.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HOST};
use http::{HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::client::conn::http1;
use hyper::rt::{Read, Write};
use hyper_util::rt::TokioIo;
use nebula_tls::{TlsConnector, TlsVerify, peer_is_loopback};
use net_guard::{Host, NetworkAccess, Policy, PortPolicy, Resolver, Target};
use rustls_pki_types::ServerName;

use crate::endpoint::{PushEndpoint, Transport};
use crate::keys::{ClientKeys, VapidKey};
use crate::vapid::{Contact, authorization};
use crate::{WebPushError, ece, http_date};

/// Time limits of one push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// TCP connection (inside `net-guard`) and TLS handshake, each.
    pub connect: Duration,
    /// The whole request, from DNS to the end of the response.
    pub total: Duration,
}

impl Limits {
    /// Limits for the connection and the whole request.
    #[must_use]
    pub const fn new(connect: Duration, total: Duration) -> Self {
        Self { connect, total }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            total: Duration::from_secs(10),
        }
    }
}

/// Longest response body read (then dropped).
const MAX_RESPONSE_BODY: usize = 1024;
/// Longest `Retry-After` honoured.
const MAX_RETRY_AFTER: Duration = Duration::from_hours(1);

/// RFC 8030 §5.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Urgency {
    /// `very-low`: only on power and Wi-Fi.
    VeryLow,
    /// `low`: on power, or Wi-Fi.
    Low,
    /// `normal`.
    Normal,
    /// `high`: time-sensitive (a reminder).
    High,
}

impl Urgency {
    /// The header value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VeryLow => "very-low",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// One message to send.
#[derive(Debug, Clone, Copy)]
pub struct Message<'a> {
    /// The plaintext (a JSON push object).
    pub payload: &'a [u8],
    /// How long the push service keeps it for an offline browser.
    pub ttl: Duration,
    /// RFC 8030 §5.3.
    pub urgency: Urgency,
    /// RFC 8030 §5.4: a newer message with the same topic replaces an
    /// undelivered one (at most 32 base64url characters).
    pub topic: Option<&'a str>,
}

/// Why a push did not get through this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// `net-guard` refused the address (a name now resolving to a private
    /// address, or a name that is never public): as permanent as a
    /// refusal. A failing, slow or empty DNS answer is [`Failure::Network`].
    Blocked(net_guard::BlockReason),
    /// Connection, TLS, DNS or HTTP failed, or a time limit passed.
    Network,
    /// The push service answered 5xx.
    Server(u16),
}

/// What the push service said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Accepted (2xx).
    Delivered,
    /// 404 or 410: the browser unsubscribed; drop the subscription.
    Gone,
    /// 429: try again later (after the `Retry-After`, if given).
    Throttled(Option<Duration>),
    /// Refused for good (3xx, or 4xx such as 400, 401, 403 after a VAPID
    /// key change, 413): drop the subscription.
    Rejected(u16),
    /// Temporary: try again later.
    Failed(Failure),
}

/// Sends push messages. Cheap to share (`Arc` inside).
#[derive(Clone)]
pub struct PushClient {
    tls: TlsConnector,
    /// For test endpoints on loopback only.
    loopback_tls: TlsConnector,
    resolver: Arc<dyn Resolver>,
    vapid: Arc<VapidKey>,
    contact: Contact,
    limits: Limits,
}

impl std::fmt::Debug for PushClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushClient")
            .field("vapid", &self.vapid)
            .field("contact", &self.contact)
            .finish_non_exhaustive()
    }
}

/// What a refused connection means for the subscription. Only a policy
/// decision is permanent (a refused address or port, a name that is never
/// public); a DNS lookup that fails, times out or gives nothing is
/// temporary, so a resolver outage doesn't drop subscriptions.
fn connect_failure(endpoint: &PushEndpoint, reason: net_guard::BlockReason) -> Failure {
    use net_guard::BlockReason;
    match reason {
        BlockReason::Network | BlockReason::Timeout => Failure::Network,
        BlockReason::Dns => match (endpoint.host(), endpoint.transport()) {
            (Host::Domain(name), Transport::Https) if name.is_non_public() => {
                Failure::Blocked(BlockReason::Dns)
            }
            _ => Failure::Network,
        },
        reason => Failure::Blocked(reason),
    }
}

fn server_name(host: &Host) -> Option<ServerName<'static>> {
    match host {
        Host::Ip(ip) => Some(ServerName::IpAddress((*ip).into())),
        Host::Domain(name) => ServerName::try_from(name.tls_name().to_owned()).ok(),
        _ => None,
    }
}

/// `Retry-After` (RFC 9110 §10.2.3): `delay-seconds` or an HTTP date in
/// any of its three formats (§5.6.7: recipients MUST accept them all); a
/// date in the past is "now". At most [`MAX_RETRY_AFTER`].
fn retry_after(value: Option<&HeaderValue>) -> Option<Duration> {
    retry_after_at(value, time::OffsetDateTime::now_utc())
}

fn retry_after_at(value: Option<&HeaderValue>, now: time::OffsetDateTime) -> Option<Duration> {
    let text = value?.to_str().ok()?.trim();
    let delay = if text.bytes().all(|b| b.is_ascii_digit()) {
        Duration::from_secs(text.parse().unwrap_or(u64::MAX))
    } else {
        let at = http_date::parse(text, now)?;
        let seconds = at.saturating_sub(now.unix_timestamp());
        Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
    };
    Some(delay.min(MAX_RETRY_AFTER))
}

fn outcome(status: StatusCode, retry: Option<&HeaderValue>) -> Outcome {
    let code = status.as_u16();
    if status.is_success() {
        Outcome::Delivered
    } else if code == 404 || code == 410 {
        Outcome::Gone
    } else if code == 429 {
        Outcome::Throttled(retry_after(retry))
    } else if status.is_server_error() {
        Outcome::Failed(Failure::Server(code))
    } else {
        Outcome::Rejected(code)
    }
}

impl PushClient {
    /// A client that signs with `vapid` and names `contact`.
    ///
    /// # Errors
    /// [`WebPushError::Tls`] if the TLS setup fails.
    pub fn new(
        vapid: Arc<VapidKey>,
        contact: Contact,
        resolver: Arc<dyn Resolver>,
    ) -> Result<Self, WebPushError> {
        Ok(Self {
            tls: TlsConnector::new(TlsVerify::Platform, &[b"http/1.1"])?,
            loopback_tls: TlsConnector::new(TlsVerify::InsecureLoopbackOnly, &[b"http/1.1"])?,
            resolver,
            vapid,
            contact,
            limits: Limits::default(),
        })
    }

    /// Other time limits (tests).
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The VAPID key.
    #[must_use]
    pub fn vapid(&self) -> &VapidKey {
        &self.vapid
    }

    /// Encrypts `message` for `keys` and POSTs it to `endpoint`. `now`
    /// (Unix seconds) dates the VAPID token.
    ///
    /// # Errors
    /// [`WebPushError::TooLarge`] for a payload that doesn't fit one
    /// message, [`WebPushError::Crypto`] if encryption or signing fails.
    /// What the network or the push service did is in the [`Outcome`].
    pub async fn send(
        &self,
        endpoint: &PushEndpoint,
        keys: &ClientKeys,
        message: Message<'_>,
        now: i64,
    ) -> Result<Outcome, WebPushError> {
        let body = ece::encrypt(message.payload, keys)?;
        let auth = authorization(&self.vapid, &endpoint.origin(), &self.contact, now)?;
        let host = endpoint.host().to_string();
        let result =
            tokio::time::timeout(self.limits.total, self.post(endpoint, body, &auth, message))
                .await
                .unwrap_or(Ok(Outcome::Failed(Failure::Network)));
        match &result {
            Ok(Outcome::Delivered) => tracing::debug!(%host, "web push delivered"),
            Ok(other) => tracing::info!(%host, outcome = ?other, "web push not delivered"),
            Err(error) => tracing::warn!(%host, %error, "web push failed"),
        }
        result
    }

    async fn post(
        &self,
        endpoint: &PushEndpoint,
        body: Vec<u8>,
        auth: &str,
        message: Message<'_>,
    ) -> Result<Outcome, WebPushError> {
        let target = Target::new(endpoint.host().clone(), endpoint.port());
        let (network, tls) = match endpoint.transport() {
            Transport::Https => (NetworkAccess::PublicOnly, Some(&self.tls)),
            Transport::LoopbackHttps => (NetworkAccess::PrivateAllowed, Some(&self.loopback_tls)),
            Transport::LoopbackHttp => (NetworkAccess::PrivateAllowed, None),
        };
        let policy = Policy::new(PortPolicy::only(vec![endpoint.port()]), network);
        let guarded = match net_guard::connect(&target, &policy, self.resolver.as_ref()).await {
            Ok(guarded) => guarded,
            Err(error) => {
                return Ok(Outcome::Failed(connect_failure(endpoint, error.reason())));
            }
        };
        let (tcp, _peer, host) = guarded.into_parts();
        if endpoint.transport() != Transport::Https && !peer_is_loopback(&tcp).unwrap_or(false) {
            return Ok(Outcome::Failed(Failure::Blocked(
                net_guard::BlockReason::PrivateIp,
            )));
        }
        let request = Self::request(endpoint, body, auth, message)?;
        match tls {
            Some(tls) => {
                let Some(name) = server_name(&host) else {
                    return Ok(Outcome::Failed(Failure::Network));
                };
                let Ok(Ok(stream)) =
                    tokio::time::timeout(self.limits.connect, tls.handshake(tcp, name)).await
                else {
                    return Ok(Outcome::Failed(Failure::Network));
                };
                Ok(self.exchange(TokioIo::new(stream), request).await)
            }
            None => Ok(self.exchange(TokioIo::new(tcp), request).await),
        }
    }

    fn request(
        endpoint: &PushEndpoint,
        body: Vec<u8>,
        auth: &str,
        message: Message<'_>,
    ) -> Result<Request<Full<Bytes>>, WebPushError> {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(endpoint.path_and_query())
            .header(HOST, endpoint.authority())
            .header(AUTHORIZATION, auth)
            .header(CONTENT_ENCODING, "aes128gcm")
            // RFC 8620 §7.2: the POST MUST be `application/json`. The
            // body is that JSON with the `aes128gcm` content coding of
            // RFC 8291 §4 on top: `Content-Type` names the representation
            // and `Content-Encoding` the coding applied to it (RFC 9110
            // §8.3, §8.4), so both hold. RFC 8291 and RFC 8030 set no
            // `Content-Type`; the push service passes it on unchecked.
            .header(CONTENT_TYPE, "application/json")
            .header(CONTENT_LENGTH, body.len())
            .header("TTL", message.ttl.as_secs())
            .header("Urgency", message.urgency.as_str());
        if let Some(topic) = message.topic {
            builder = builder.header("Topic", topic);
        }
        builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| WebPushError::Crypto)
    }

    async fn exchange<T>(&self, io: T, request: Request<Full<Bytes>>) -> Outcome
    where
        T: Read + Write + Unpin + Send + 'static,
    {
        let mut builder = http1::Builder::new();
        builder.max_headers(64).max_buf_size(16 * 1024);
        let Ok(Ok((mut sender, connection))) =
            tokio::time::timeout(self.limits.connect, builder.handshake(io)).await
        else {
            return Outcome::Failed(Failure::Network);
        };
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let Ok(response) = sender.send_request(request).await else {
            driver.abort();
            return Outcome::Failed(Failure::Network);
        };
        let result = outcome(response.status(), response.headers().get("retry-after"));
        // Read a little of the body so the server sees a clean end, then
        // drop it (never logged: it may echo the URL).
        let _ = Limited::new(response.into_body(), MAX_RESPONSE_BODY)
            .collect()
            .await;
        drop(sender);
        driver.abort();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes() {
        let none = None;
        assert_eq!(outcome(StatusCode::CREATED, none), Outcome::Delivered);
        assert_eq!(outcome(StatusCode::NOT_FOUND, none), Outcome::Gone);
        assert_eq!(outcome(StatusCode::GONE, none), Outcome::Gone);
        assert_eq!(outcome(StatusCode::FORBIDDEN, none), Outcome::Rejected(403));
        assert_eq!(outcome(StatusCode::FOUND, none), Outcome::Rejected(302));
        assert_eq!(
            outcome(StatusCode::PAYLOAD_TOO_LARGE, none),
            Outcome::Rejected(413)
        );
        assert_eq!(
            outcome(StatusCode::BAD_GATEWAY, none),
            Outcome::Failed(Failure::Server(502))
        );
        let retry = HeaderValue::from_static("120");
        assert_eq!(
            outcome(StatusCode::TOO_MANY_REQUESTS, Some(&retry)),
            Outcome::Throttled(Some(Duration::from_secs(120)))
        );
        let huge = HeaderValue::from_static("999999");
        assert_eq!(retry_after(Some(&huge)), Some(MAX_RETRY_AFTER));
        assert_eq!(retry_after(Some(&HeaderValue::from_static("soon"))), None);
        assert_eq!(retry_after(None), None);
    }

    /// `Retry-After` as an HTTP date, in all three formats (RFC 9110
    /// §5.6.7); one in the past is now, one far away the cap.
    #[test]
    fn retry_after_dates() {
        // 1994-11-06T08:47:37Z: two minutes before the RFC's example date.
        let now = time::OffsetDateTime::from_unix_timestamp(784_111_657).unwrap();
        for date in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            let value = HeaderValue::from_static(date);
            assert_eq!(
                retry_after_at(Some(&value), now),
                Some(Duration::from_secs(120)),
                "{date}"
            );
        }
        let past = HeaderValue::from_static("Sat, 05 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_after_at(Some(&past), now), Some(Duration::ZERO));
        let far = HeaderValue::from_static("Mon, 07 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_after_at(Some(&far), now), Some(MAX_RETRY_AFTER));
    }
}
