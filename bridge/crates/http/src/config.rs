// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Typed settings of the HTTP server. Every value is checked when it is
//! built, so the handlers never validate configuration again.

use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use nebula_crypto::ClientSecretHash;
use nebula_jmap_proto::UnsignedInt;
use nebula_jmap_proto::method::Collation;
use nebula_jmap_proto::session::CoreCapability;
use url::{Host, Url};

/// A configuration value that is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Not a URL at all.
    #[error("{what}: not a valid URL")]
    Url {
        /// Which setting.
        what: &'static str,
    },
    /// Plain `http` on a host that is not loopback.
    #[error("{what}: must use https (http only for localhost)")]
    NotHttps {
        /// Which setting.
        what: &'static str,
    },
    /// User name, password, query or fragment where none is allowed.
    #[error("{what}: must not contain {part}")]
    ExtraPart {
        /// Which setting.
        what: &'static str,
        /// What was found.
        part: &'static str,
    },
    /// The public URL has a path; the bridge must be served at the root.
    #[error("public_url: must not have a path (serve the bridge at the root of its host)")]
    PublicUrlPath,
    /// A client id with characters outside `A-Z a-z 0-9 . _ ~ -` or of a
    /// bad length.
    #[error("client id must be 1 to 64 characters of A-Z a-z 0-9 . _ ~ -")]
    ClientId,
    /// Two clients share an id.
    #[error("duplicate OAuth client id {0}")]
    DuplicateClient(String),
    /// A client without redirect URIs.
    #[error("OAuth client {0} has no redirect_uris")]
    NoRedirectUris(String),
    /// No OAuth client at all.
    #[error("at least one OAuth client is required")]
    NoClients,
    /// Token lifetimes out of range.
    #[error("{0}")]
    Lifetime(&'static str),
    /// A limit of zero or out of range.
    #[error("{0}")]
    Limit(&'static str),
}

/// Whether `host` is a loopback name or address (where plain http is
/// acceptable for development).
#[must_use]
pub fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// Parses an absolute http(s) URL without user info; http only on loopback.
fn parse_web_url(text: &str, what: &'static str) -> Result<Url, ConfigError> {
    let url = Url::parse(text.trim()).map_err(|_| ConfigError::Url { what })?;
    let host = url.host().ok_or(ConfigError::Url { what })?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        _ => return Err(ConfigError::NotHttps { what }),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::ExtraPart {
            what,
            part: "a user name or password",
        });
    }
    if url.fragment().is_some() {
        return Err(ConfigError::ExtraPart {
            what,
            part: "a fragment",
        });
    }
    Ok(url)
}

/// The public URL of the bridge: the OAuth `issuer` and the base of every
/// URL in the JMAP session. Only an origin (`https://jmap.example.com`).
#[derive(Clone, PartialEq, Eq)]
pub struct PublicUrl {
    /// `scheme://host[:port]`, without a trailing slash: exactly what the
    /// discovery document and the `iss` parameter say.
    issuer: String,
    https: bool,
}

impl PublicUrl {
    /// Parses the public URL.
    ///
    /// # Errors
    /// If it isn't https (http only on loopback) or has a path, query,
    /// fragment or user info.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        const WHAT: &str = "public_url";
        let url = parse_web_url(text, WHAT)?;
        if url.query().is_some() {
            return Err(ConfigError::ExtraPart {
                what: WHAT,
                part: "a query",
            });
        }
        if url.path() != "/" {
            return Err(ConfigError::PublicUrlPath);
        }
        Ok(Self {
            issuer: url.origin().ascii_serialization(),
            https: url.scheme() == "https",
        })
    }

    /// The issuer: the origin without a trailing slash.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// An absolute URL for `path` (which starts with `/`).
    #[must_use]
    pub fn join(&self, path: &str) -> String {
        format!("{}{path}", self.issuer)
    }

    /// Whether it uses https (then HSTS is sent and cookies are `Secure`).
    #[must_use]
    pub fn is_https(&self) -> bool {
        self.https
    }
}

impl fmt::Debug for PublicUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PublicUrl").field(&self.issuer).finish()
    }
}

/// An origin of the web app, allowed by CORS to call JMAP from the browser.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AppOrigin(String);

impl AppOrigin {
    /// Parses an origin (`https://mail.example.com`, no path).
    ///
    /// # Errors
    /// If it isn't an https origin (http only on loopback).
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        const WHAT: &str = "app_origins";
        let url = parse_web_url(text, WHAT)?;
        if url.path() != "/" || url.query().is_some() {
            return Err(ConfigError::ExtraPart {
                what: WHAT,
                part: "a path or query (only scheme://host[:port])",
            });
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    /// The origin as the browser sends it in `Origin`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AppOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AppOrigin").field(&self.0).finish()
    }
}

/// A redirect URI registered for a client. Compared as an exact string
/// (OAuth 2.1 §2.3.1) with its normalised form (`Url::as_str`).
#[derive(Clone, PartialEq, Eq)]
pub struct RedirectUri(Url);

impl RedirectUri {
    /// Parses a redirect URI: absolute https, or http on a loopback host
    /// for development; no fragment, no user info.
    ///
    /// # Errors
    /// If it breaks one of those rules.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        parse_web_url(text, "redirect_uris").map(Self)
    }

    /// The URI exactly as it must be sent by the client.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The URL, to add the response parameters.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.0
    }

    /// Its origin, for the login page's `form-action` CSP (browsers check
    /// the redirect that follows a form post too).
    #[must_use]
    pub fn origin(&self) -> String {
        self.0.origin().ascii_serialization()
    }
}

impl fmt::Debug for RedirectUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RedirectUri")
            .field(&self.0.as_str())
            .finish()
    }
}

/// An OAuth `client_id`: 1 to 64 characters of `A-Z a-z 0-9 . _ ~ -`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientId(String);

impl ClientId {
    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ClientId {
    type Err = ConfigError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let ok = (1..=64).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'));
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(ConfigError::ClientId)
        }
    }
}

impl fmt::Debug for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ClientId").field(&self.0).finish()
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A confidential OAuth client (the web app's BFF), fixed in the
/// configuration: no dynamic registration.
#[derive(Debug, Clone)]
pub struct OAuthClient {
    id: ClientId,
    secret_hash: ClientSecretHash,
    redirect_uris: Vec<RedirectUri>,
}

impl OAuthClient {
    /// A client with at least one redirect URI.
    ///
    /// # Errors
    /// [`ConfigError::NoRedirectUris`] if `redirect_uris` is empty.
    pub fn new(
        id: ClientId,
        secret_hash: ClientSecretHash,
        redirect_uris: Vec<RedirectUri>,
    ) -> Result<Self, ConfigError> {
        if redirect_uris.is_empty() {
            return Err(ConfigError::NoRedirectUris(id.0));
        }
        Ok(Self {
            id,
            secret_hash,
            redirect_uris,
        })
    }

    /// The client id.
    #[must_use]
    pub fn id(&self) -> &ClientId {
        &self.id
    }

    /// The argon2id hash of its secret.
    #[must_use]
    pub fn secret_hash(&self) -> &ClientSecretHash {
        &self.secret_hash
    }

    /// The registered redirect URI equal to `candidate`, if any.
    #[must_use]
    pub fn redirect_uri(&self, candidate: &str) -> Option<&RedirectUri> {
        self.redirect_uris.iter().find(|r| r.as_str() == candidate)
    }
}

/// How long codes, tokens and sessions live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifetimes {
    access_token: Duration,
    refresh_token: Duration,
    session: Duration,
}

impl Lifetimes {
    /// Authorization codes: one minute, single use.
    pub const CODE: Duration = Duration::from_secs(60);
    /// Time to fill in the login form.
    pub const LOGIN_FORM: Duration = Duration::from_mins(10);

    /// Checks and builds the lifetimes.
    ///
    /// # Errors
    /// If the access token lives less than a minute or more than an hour,
    /// or the refresh token or session less than the access token or more
    /// than a year.
    pub fn new(
        access_token: Duration,
        refresh_token: Duration,
        session: Duration,
    ) -> Result<Self, ConfigError> {
        const YEAR: Duration = Duration::from_hours(24 * 366);
        if !(Duration::from_mins(1)..=Duration::from_hours(1)).contains(&access_token) {
            return Err(ConfigError::Lifetime(
                "access tokens must live between 1 and 60 minutes",
            ));
        }
        if !(access_token..=YEAR).contains(&refresh_token)
            || !(access_token..=YEAR).contains(&session)
        {
            return Err(ConfigError::Lifetime(
                "refresh tokens and sessions must live longer than access tokens and at most a year",
            ));
        }
        Ok(Self {
            access_token,
            refresh_token,
            session,
        })
    }

    /// Lifetime of an access token.
    #[must_use]
    pub fn access_token(&self) -> Duration {
        self.access_token
    }

    /// Lifetime of a refresh token (never past the session's end).
    #[must_use]
    pub fn refresh_token(&self) -> Duration {
        self.refresh_token
    }

    /// Maximum length of a login session, whatever the refreshes.
    #[must_use]
    pub fn session(&self) -> Duration {
        self.session
    }
}

impl Default for Lifetimes {
    /// 15 minutes, 30 days, 30 days.
    fn default() -> Self {
        Self {
            access_token: Duration::from_mins(15),
            refresh_token: Duration::from_hours(24 * 30),
            session: Duration::from_hours(24 * 30),
        }
    }
}

/// A limit of `max` events per `period`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    max: u32,
    period: Duration,
}

impl RateLimit {
    /// Checks and builds a limit.
    ///
    /// # Errors
    /// If `max` or `period` is zero.
    pub fn new(max: u32, period: Duration) -> Result<Self, ConfigError> {
        if max == 0 || period.is_zero() {
            return Err(ConfigError::Limit("rate limits must be above zero"));
        }
        Ok(Self { max, period })
    }

    /// Events allowed per period.
    #[must_use]
    pub fn max(&self) -> u32 {
        self.max
    }

    /// The period.
    #[must_use]
    pub fn period(&self) -> Duration {
        self.period
    }
}

/// Failed login limits (a failed login is also an attempt
/// against someone's IMAP account).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginLimits {
    /// Per login name.
    pub per_login: RateLimit,
    /// Per client IP (IPv6: per /64).
    pub per_ip: RateLimit,
}

impl Default for LoginLimits {
    /// 5 failures per login and 20 per IP in 15 minutes.
    fn default() -> Self {
        let period = Duration::from_mins(15);
        Self {
            per_login: RateLimit { max: 5, period },
            per_ip: RateLimit { max: 20, period },
        }
    }
}

/// Where the client IP for rate limiting comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ClientIpSource {
    /// The TCP peer (the bridge faces the internet directly).
    #[default]
    Peer,
    /// The last address in `X-Forwarded-For`, i.e. the one the closest
    /// reverse proxy saw. Only behind a proxy that sets it, or anyone can
    /// fake their IP.
    XForwardedFor,
}

/// JMAP limits (the core capability) and per-account stream limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JmapLimits {
    core: CoreCapability,
    event_streams_per_account: usize,
}

impl JmapLimits {
    /// Checks and builds the limits.
    ///
    /// # Errors
    /// If a limit is zero or the request size is above 64 MiB or the
    /// upload size above 1 GiB (the bridge buffers bodies in memory).
    pub fn new(
        core: CoreCapability,
        event_streams_per_account: usize,
    ) -> Result<Self, ConfigError> {
        let size_request = u64::from(core.max_size_request);
        let size_upload = u64::from(core.max_size_upload);
        let counts = [
            core.max_concurrent_upload,
            core.max_concurrent_requests,
            core.max_calls_in_request,
            core.max_objects_in_get,
            core.max_objects_in_set,
        ];
        if counts.iter().any(|n| u64::from(*n) == 0) || event_streams_per_account == 0 {
            return Err(ConfigError::Limit("JMAP limits must be above zero"));
        }
        if !(1024..=64 * 1024 * 1024).contains(&size_request) {
            return Err(ConfigError::Limit(
                "max_size_request must be between 1 KiB and 64 MiB",
            ));
        }
        if !(1024..=1024 * 1024 * 1024).contains(&size_upload) {
            return Err(ConfigError::Limit(
                "max_size_upload must be between 1 KiB and 1 GiB",
            ));
        }
        Ok(Self {
            core,
            event_streams_per_account,
        })
    }

    /// The core capability announced in the session.
    #[must_use]
    pub fn core(&self) -> &CoreCapability {
        &self.core
    }

    /// Open `EventSource` connections allowed per account.
    #[must_use]
    pub fn event_streams_per_account(&self) -> usize {
        self.event_streams_per_account
    }

    pub(crate) fn max_size_request(&self) -> usize {
        self.core.max_size_request.to_usize_saturating()
    }

    pub(crate) fn max_size_upload(&self) -> usize {
        self.core.max_size_upload.to_usize_saturating()
    }

    pub(crate) fn max_concurrent_requests(&self) -> usize {
        self.core.max_concurrent_requests.to_usize_saturating()
    }

    pub(crate) fn max_concurrent_upload(&self) -> usize {
        self.core.max_concurrent_upload.to_usize_saturating()
    }
}

impl Default for JmapLimits {
    /// Close to Stalwart's defaults: 50 MB uploads, 10 MB requests.
    fn default() -> Self {
        let n = UnsignedInt::from;
        Self {
            core: CoreCapability {
                max_size_upload: n(50_000_000),
                max_concurrent_upload: n(4),
                max_size_request: n(10_000_000),
                max_concurrent_requests: n(4),
                max_calls_in_request: n(32),
                max_objects_in_get: n(500),
                max_objects_in_set: n(500),
                collation_algorithms: vec![
                    Collation::AsciiNumeric,
                    Collation::AsciiCasemap,
                    Collation::UnicodeCasemap,
                ],
            },
            event_streams_per_account: 8,
        }
    }
}

/// Whether held messages (scheduled send) may go out after the user signs
/// out (`[submission] send_while_signed_out`). When on, an account whose
/// last session ended normally (logout, revoke, expiry) keeps running, and
/// its password stays sealed in the store, while it has a held message
/// marked `sendWhenSignedOut` that can still be sent. A session revoked as
/// stolen, or a password the mail server refuses, deletes it at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SignedOutSending {
    /// Off: the password goes with the last session.
    #[default]
    Off,
    /// On.
    On {
        /// How long after its time a held message may still be sent (the
        /// scheduler's `max_wait`): past it, it no longer keeps the
        /// password.
        max_wait: Duration,
    },
}

impl SignedOutSending {
    /// Whether it is on.
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On { .. })
    }
}

/// Everything the HTTP server needs to know.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    public_url: PublicUrl,
    app_origins: Vec<AppOrigin>,
    clients: Vec<OAuthClient>,
    /// Token and session lifetimes.
    pub lifetimes: Lifetimes,
    /// Failed login limits.
    pub login_limits: LoginLimits,
    /// JMAP limits.
    pub jmap: JmapLimits,
    /// Where the client IP comes from.
    pub client_ip: ClientIpSource,
    /// Held messages sent after sign-out.
    pub signed_out_sending: SignedOutSending,
}

impl HttpConfig {
    /// A configuration with default lifetimes and limits.
    ///
    /// # Errors
    /// [`ConfigError::NoClients`] without clients and
    /// [`ConfigError::DuplicateClient`] if two share an id.
    pub fn new(
        public_url: PublicUrl,
        app_origins: Vec<AppOrigin>,
        clients: Vec<OAuthClient>,
    ) -> Result<Self, ConfigError> {
        if clients.is_empty() {
            return Err(ConfigError::NoClients);
        }
        let mut seen = HashSet::new();
        for client in &clients {
            if !seen.insert(client.id.as_str()) {
                return Err(ConfigError::DuplicateClient(client.id.0.clone()));
            }
        }
        Ok(Self {
            public_url,
            app_origins,
            clients,
            lifetimes: Lifetimes::default(),
            login_limits: LoginLimits::default(),
            jmap: JmapLimits::default(),
            client_ip: ClientIpSource::default(),
            signed_out_sending: SignedOutSending::default(),
        })
    }

    /// The public URL (issuer).
    #[must_use]
    pub fn public_url(&self) -> &PublicUrl {
        &self.public_url
    }

    /// Origins allowed by CORS.
    #[must_use]
    pub fn app_origins(&self) -> &[AppOrigin] {
        &self.app_origins
    }

    /// The client with this id.
    #[must_use]
    pub fn client(&self, id: &str) -> Option<&OAuthClient> {
        self.clients.iter().find(|c| c.id.as_str() == id)
    }
}

/// The key used to count failed logins from one address: the address for
/// IPv4, its /64 network for IPv6 (one host usually has a whole /64).
#[must_use]
pub(crate) fn ip_bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let mut segments = v6.segments();
            for segment in segments.iter_mut().skip(4) {
                *segment = 0;
            }
            IpAddr::V6(segments.into())
        }
    }
}

/// A wider key than [`ip_bucket`] for limits on whole networks: the /48 for
/// IPv6 (where one site gets 65,536 /64s); for IPv4, the address itself
/// (no wider limit there).
#[must_use]
pub(crate) fn ip_bucket_wide(ip: IpAddr) -> IpAddr {
    match ip_bucket(ip) {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            for segment in segments.iter_mut().skip(3) {
                *segment = 0;
            }
            IpAddr::V6(segments.into())
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_is_an_origin() {
        let url = PublicUrl::parse("https://jmap.example.com/").unwrap();
        assert_eq!(url.issuer(), "https://jmap.example.com");
        assert_eq!(url.join("/jmap/api"), "https://jmap.example.com/jmap/api");
        assert!(url.is_https());
        assert_eq!(
            PublicUrl::parse("http://127.0.0.1:8080").unwrap().issuer(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            PublicUrl::parse("HTTPS://JMAP.example.com:443")
                .unwrap()
                .issuer(),
            "https://jmap.example.com"
        );
        for bad in [
            "http://jmap.example.com",
            "https://jmap.example.com/bridge",
            "https://jmap.example.com/?a=b",
            "https://jmap.example.com/#x",
            "https://user:pw@jmap.example.com",
            "ftp://jmap.example.com",
            "jmap.example.com",
            "",
        ] {
            assert!(PublicUrl::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn redirect_uris() {
        let uri = RedirectUri::parse("https://mail.example.com/auth/callback").unwrap();
        assert_eq!(uri.as_str(), "https://mail.example.com/auth/callback");
        assert_eq!(uri.origin(), "https://mail.example.com");
        assert!(RedirectUri::parse("http://localhost:3001/auth/callback").is_ok());
        assert!(RedirectUri::parse("http://[::1]:3001/auth/callback").is_ok());
        for bad in [
            "http://mail.example.com/auth/callback",
            "https://mail.example.com/auth/callback#x",
            "/auth/callback",
            "custom:callback",
        ] {
            assert!(RedirectUri::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn app_origins() {
        assert_eq!(
            AppOrigin::parse("https://mail.example.com")
                .unwrap()
                .as_str(),
            "https://mail.example.com"
        );
        assert!(AppOrigin::parse("https://mail.example.com/app").is_err());
        assert!(AppOrigin::parse("http://mail.example.com").is_err());
    }

    #[test]
    fn client_ids_and_lifetimes() {
        assert!("nebula-dev".parse::<ClientId>().is_ok());
        assert!("".parse::<ClientId>().is_err());
        assert!("a b".parse::<ClientId>().is_err());
        assert!("x".repeat(65).parse::<ClientId>().is_err());
        assert!(
            Lifetimes::new(
                Duration::from_secs(10),
                Duration::from_hours(1),
                Duration::from_hours(1)
            )
            .is_err()
        );
        assert!(
            Lifetimes::new(
                Duration::from_mins(15),
                Duration::from_mins(5),
                Duration::from_hours(1)
            )
            .is_err()
        );
    }

    #[test]
    fn ipv6_counts_per_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        assert_eq!(ip_bucket(a), ip_bucket(b));
        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(ip_bucket(mapped), "192.0.2.1".parse::<IpAddr>().unwrap());
    }
}
