// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Push endpoints: the URL a browser's push service gave it, which the
//! user hands to the bridge (`PushSubscription.url`) and the bridge then
//! POSTs to. It is user input pointing at the network, so it goes
//! through the SSRF rules before it is even stored:
//!
//! - `https` only, port 443, no user name or password, at most
//!   [`MAX_URL_LEN`] bytes, WHATWG parsing (`net-guard`);
//! - an IP literal must be public (and is refused unless any host is
//!   allowed); a name must match the [`EndpointPolicy`]: by default only
//!   the browsers' push services ([`KNOWN_PUSH_SERVICES`]);
//! - at connection time every resolved address is checked again and the
//!   connection is pinned to it (`net-guard`), see [`crate::client`].
//!
//! For tests and local development only, [`EndpointPolicy::with_loopback`]
//! also accepts `http(s)://127.0.0.1:<port>` and `[::1]`.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use net_guard::{BlockReason, Host, Port, UrlPolicy, parse_target_url};
use url::Url;

/// Longest endpoint URL accepted (real ones are 150–500 bytes).
pub const MAX_URL_LEN: usize = 1024;

/// The push services of the major browsers (2026): Chrome and other
/// Chromium browsers (FCM), Firefox (Mozilla autopush), Safari (Apple)
/// and Edge (Windows Push Notification Services). `*.` matches any
/// subdomain (not the name itself).
pub const KNOWN_PUSH_SERVICES: &[&str] = &[
    "fcm.googleapis.com",
    "*.push.services.mozilla.com",
    "*.push.apple.com",
    "*.notify.windows.com",
];

/// Why an endpoint is refused. Carries no part of the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EndpointError {
    /// Longer than [`MAX_URL_LEN`].
    #[error("push URL too long")]
    TooLong,
    /// A `net-guard` rule (scheme, user info, port, private IP).
    #[error("push URL refused: {0}")]
    Blocked(BlockReason),
    /// The host is not one of the allowed push services.
    #[error("push URL host is not an allowed push service")]
    HostNotAllowed,
}

impl EndpointError {
    /// The reason in the `ssrf-vectors.json` contract, when there is one.
    #[must_use]
    pub fn block_reason(&self) -> Option<BlockReason> {
        match self {
            Self::Blocked(reason) => Some(*reason),
            Self::TooLong | Self::HostNotAllowed => None,
        }
    }
}

/// A host pattern of the allowlist: `push.example.com` (that name) or
/// `*.example.com` (any name under it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern {
    suffix: bool,
    name: String,
}

/// A host pattern that is not a domain name (or `*.` and one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid push host pattern")]
pub struct InvalidHostPattern;

impl FromStr for HostPattern {
    type Err = InvalidHostPattern;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (suffix, name) = match text.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        match name.parse::<Host>() {
            // At least two labels: `*.com` would allow far too much.
            Ok(Host::Domain(domain)) if domain.as_str().contains('.') => Ok(Self {
                suffix,
                name: domain.as_str().to_owned(),
            }),
            _ => Err(InvalidHostPattern),
        }
    }
}

impl HostPattern {
    fn matches(&self, host: &str) -> bool {
        if self.suffix {
            host.strip_suffix(self.name.as_str())
                .is_some_and(|head| head.len() > 1 && head.ends_with('.'))
        } else {
            host == self.name
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.suffix {
            f.write_str("*.")?;
        }
        f.write_str(&self.name)
    }
}

/// Which endpoints the bridge accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointPolicy {
    /// `None`: any public host (`allowed_hosts = ["*"]`).
    hosts: Option<Vec<HostPattern>>,
    loopback: bool,
}

impl Default for EndpointPolicy {
    fn default() -> Self {
        Self::known_services()
    }
}

impl EndpointPolicy {
    /// Only [`KNOWN_PUSH_SERVICES`] (the default).
    #[must_use]
    pub fn known_services() -> Self {
        Self {
            hosts: Some(
                KNOWN_PUSH_SERVICES
                    .iter()
                    .filter_map(|pattern| pattern.parse().ok())
                    .collect(),
            ),
            loopback: false,
        }
    }

    /// Only these hosts.
    #[must_use]
    pub fn only(hosts: Vec<HostPattern>) -> Self {
        Self {
            hosts: Some(hosts),
            loopback: false,
        }
    }

    /// Any host whose addresses are all public.
    #[must_use]
    pub const fn any_public_host() -> Self {
        Self {
            hosts: None,
            loopback: false,
        }
    }

    /// Also `http(s)://127.0.0.1:<port>/…` and `[::1]` (tests and local
    /// development only: a fake push service on this machine).
    #[must_use]
    pub const fn with_loopback(mut self) -> Self {
        self.loopback = true;
        self
    }

    /// Whether loopback endpoints are accepted.
    #[must_use]
    pub const fn allows_loopback(&self) -> bool {
        self.loopback
    }

    fn host_allowed(&self, host: &Host) -> bool {
        match (&self.hosts, host) {
            (None, _) => true,
            (Some(patterns), Host::Domain(name)) => patterns
                .iter()
                .any(|pattern| pattern.matches(name.as_str())),
            (Some(_), _) => false,
        }
    }
}

/// How to reach an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    /// TLS with the platform's certificate checks.
    Https,
    /// Loopback only (tests): TLS without certificate checks.
    LoopbackHttps,
    /// Loopback only (tests): plain HTTP.
    LoopbackHttp,
}

/// A push endpoint that passed the policy. Its URL is a capability
/// (anyone with it, the keys and the VAPID key can push to the browser):
/// `Debug` and `Display` show only the host, and it is stored sealed.
#[derive(Clone, PartialEq, Eq)]
pub struct PushEndpoint {
    url: Url,
    host: Host,
    port: Port,
    transport: Transport,
}

impl fmt::Debug for PushEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushEndpoint")
            .field("host", &self.host.to_string())
            .finish_non_exhaustive()
    }
}

fn loopback_literal(url: &Url) -> Option<IpAddr> {
    let ip = match url.host()? {
        url::Host::Ipv4(ip) => IpAddr::V4(ip),
        url::Host::Ipv6(ip) => IpAddr::V6(ip),
        url::Host::Domain(_) => return None,
    };
    ip.is_loopback().then_some(ip)
}

impl PushEndpoint {
    /// Checks `text` against `policy` (no DNS: names are resolved and
    /// checked when connecting).
    ///
    /// # Errors
    /// [`EndpointError`] for the first rule it breaks.
    pub fn parse(text: &str, policy: &EndpointPolicy) -> Result<Self, EndpointError> {
        if text.len() > MAX_URL_LEN {
            return Err(EndpointError::TooLong);
        }
        if policy.loopback
            && let Ok(url) = Url::parse(text)
            && let Some(ip) = loopback_literal(&url)
            && url.username().is_empty()
            && url.password().is_none()
        {
            let transport = match url.scheme() {
                "https" => Transport::LoopbackHttps,
                "http" => Transport::LoopbackHttp,
                _ => return Err(EndpointError::Blocked(BlockReason::Scheme)),
            };
            let port = url
                .port_or_known_default()
                .and_then(|number| Port::try_from(number).ok())
                .ok_or(EndpointError::Blocked(BlockReason::Port))?;
            let mut url = url;
            url.set_fragment(None);
            return Ok(Self {
                url,
                host: Host::Ip(ip),
                port,
                transport,
            });
        }
        let parsed = parse_target_url(text, &UrlPolicy::HTTPS).map_err(EndpointError::Blocked)?;
        if !policy.host_allowed(parsed.host()) {
            return Err(EndpointError::HostNotAllowed);
        }
        Ok(Self {
            url: parsed.url().clone(),
            host: parsed.host().clone(),
            port: parsed.port(),
            transport: Transport::Https,
        })
    }

    /// The URL (normalized, without fragment). Never log it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// The host.
    #[must_use]
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The port.
    #[must_use]
    pub fn port(&self) -> Port {
        self.port
    }

    /// How to connect.
    #[must_use]
    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// The origin (`https://host[:port]`): the JWT `aud`.
    #[must_use]
    pub fn origin(&self) -> String {
        self.url.origin().ascii_serialization()
    }

    /// The `Host` header: the host, and the port if not the default.
    #[must_use]
    pub fn authority(&self) -> String {
        let host = self.url.host_str().unwrap_or_default();
        match self.url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        }
    }

    /// The request target: path and query.
    #[must_use]
    pub fn path_and_query(&self) -> String {
        match self.url.query() {
            Some(query) => format!("{}?{query}", self.url.path()),
            None => self.url.path().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(url: &str) -> Result<PushEndpoint, EndpointError> {
        PushEndpoint::parse(url, &EndpointPolicy::known_services())
    }

    /// Real endpoints of each browser (shapes of 2026).
    #[test]
    fn browsers() {
        for url in [
            "https://fcm.googleapis.com/fcm/send/dGVzdA:APA91bE",
            "https://updates.push.services.mozilla.com/wpush/v2/gAAAAABk",
            "https://web.push.apple.com/QGuQyavXutnMqvqVN4BAkGnN",
            "https://wns2-par02p.notify.windows.com/w/?token=BQYAAAB",
        ] {
            let endpoint = known(url).unwrap();
            assert_eq!(endpoint.transport(), Transport::Https);
            assert!(endpoint.origin().starts_with("https://"));
            assert!(!format!("{endpoint:?}").contains("/fcm/send"));
        }
        let wns = known("https://wns2-par02p.notify.windows.com/w/?token=BQYAAAB").unwrap();
        assert_eq!(wns.path_and_query(), "/w/?token=BQYAAAB");
        assert_eq!(wns.origin(), "https://wns2-par02p.notify.windows.com");
    }

    /// scheme, user info, port, private and reserved IPs
    /// (in every spelling), hosts that are not push services.
    #[test]
    fn refused() {
        let blocked = |url: &str| known(url).unwrap_err().block_reason();
        assert_eq!(
            blocked("http://fcm.googleapis.com/fcm/send/x"),
            Some(BlockReason::Scheme)
        );
        assert_eq!(blocked("file:///etc/passwd"), Some(BlockReason::Scheme));
        assert_eq!(
            blocked("https://user:pw@fcm.googleapis.com/x"),
            Some(BlockReason::Userinfo)
        );
        assert_eq!(
            blocked("https://fcm.googleapis.com:8443/x"),
            Some(BlockReason::Port)
        );
        for private in [
            "https://127.0.0.1/x",
            "https://2130706433/x",
            "https://0x7f.1/x",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/x",
            "https://192.168.1.1/x",
        ] {
            assert_eq!(blocked(private), Some(BlockReason::PrivateIp), "{private}");
        }
        for other in [
            "https://evil.example.com/x",
            "https://fcm.googleapis.com.evil.example/x",
            "https://notify.windows.com/x",
            "https://xnotify.windows.com/x",
            "https://8.8.8.8/x",
            "https://metadata.google.internal/x",
        ] {
            assert_eq!(known(other), Err(EndpointError::HostNotAllowed), "{other}");
        }
        let long = format!("https://fcm.googleapis.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(known(&long), Err(EndpointError::TooLong));
        // Loopback is never accepted outside tests, not even over http.
        assert!(known("http://127.0.0.1:9000/push").is_err());
    }

    #[test]
    fn policies() {
        let any = EndpointPolicy::any_public_host();
        assert!(PushEndpoint::parse("https://push.example.org/x", &any).is_ok());
        assert!(PushEndpoint::parse("https://8.8.8.8/x", &any).is_ok());
        assert!(PushEndpoint::parse("https://10.1.2.3/x", &any).is_err());
        let only = EndpointPolicy::only(vec!["push.example.org".parse().unwrap()]);
        assert!(PushEndpoint::parse("https://push.example.org/x", &only).is_ok());
        assert!(PushEndpoint::parse("https://fcm.googleapis.com/x", &only).is_err());
        let loopback = EndpointPolicy::only(vec![]).with_loopback();
        let local = PushEndpoint::parse("http://127.0.0.1:9000/push/1#frag", &loopback).unwrap();
        assert_eq!(local.transport(), Transport::LoopbackHttp);
        assert_eq!(local.port().get(), 9000);
        assert_eq!(local.as_str(), "http://127.0.0.1:9000/push/1");
        assert_eq!(
            PushEndpoint::parse("https://[::1]:9443/p", &loopback)
                .unwrap()
                .transport(),
            Transport::LoopbackHttps
        );
        // Still nothing else: a private address or a name.
        assert!(PushEndpoint::parse("http://10.0.0.1:9000/p", &loopback).is_err());
        assert!(PushEndpoint::parse("http://localhost:9000/p", &loopback).is_err());
        assert!(PushEndpoint::parse("http://u@127.0.0.1:9000/p", &loopback).is_err());
    }

    #[test]
    fn patterns() {
        assert!("*.com".parse::<HostPattern>().is_err());
        assert!("*".parse::<HostPattern>().is_err());
        assert!("10.0.0.1".parse::<HostPattern>().is_err());
        assert!("push example.com".parse::<HostPattern>().is_err());
        let pattern: HostPattern = "*.Push.Apple.com".parse().unwrap();
        assert_eq!(pattern.to_string(), "*.push.apple.com");
        assert!(pattern.matches("web.push.apple.com"));
        assert!(!pattern.matches("push.apple.com"));
        assert!(!pattern.matches("evilpush.apple.com"));
    }
}
