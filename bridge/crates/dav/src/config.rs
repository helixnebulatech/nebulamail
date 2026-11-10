// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The DAV server of the configuration, its limits and the credentials.

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nebula_dav_proto::Href;
use nebula_tls::TlsVerify;
use net_guard::{Host, NetworkAccess, Port};
use secrecy::{ExposeSecret as _, SecretString};
use url::Url;
use zeroize::Zeroizing;

use crate::DavError;
use crate::scope::{AccountHost, AccountHosts};

/// Size limits and timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest multistatus body: 16 MiB.
    pub max_multistatus: usize,
    /// Largest iCalendar resource: 1 MiB.
    pub max_icalendar: usize,
    /// Largest vCard resource: 4 MiB.
    pub max_vcard: usize,
    /// Largest body read for errors and redirects: 64 KiB (more is
    /// dropped with the connection, never read).
    pub max_error_body: usize,
    /// Most XML nodes in one multistatus.
    pub max_xml_nodes: u32,
    /// Most resources in one collection: 50 000.
    pub max_resources: usize,
    /// Most collections kept per home: 200, the account's own first,
    /// then those shared with it that it already had (never a reason to
    /// fail a listing).
    pub max_collections: usize,
    /// Hrefs per multiget: 100.
    pub multiget_batch: usize,
    /// Redirects followed per request: 5.
    pub max_redirects: u8,
    /// Truncated sync-collection answers (507) followed in a row.
    pub max_sync_rounds: u32,
    /// TCP connection (per address, in `net-guard`) and TLS handshake: 5 s.
    pub connect_timeout: Duration,
    /// A whole request, redirects and body included: 60 s.
    pub request_timeout: Duration,
    /// Without any data from the server while a body comes: 15 s; and a
    /// connection unused for that long is not reused. The wait for the
    /// response head has only `request_timeout`, as servers build big
    /// reports before they answer.
    pub idle_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_multistatus: 16 * 1024 * 1024,
            max_icalendar: 1024 * 1024,
            max_vcard: 4 * 1024 * 1024,
            max_error_body: 64 * 1024,
            max_xml_nodes: 1_000_000,
            max_resources: 50_000,
            max_collections: 200,
            multiget_batch: 100,
            max_redirects: 5,
            max_sync_rounds: 100,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(15),
        }
    }
}

/// `http` or `https`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// Plain HTTP: only to a loopback peer (Docker development servers),
    /// checked on the connected socket.
    Http,
    /// HTTPS.
    Https,
}

/// Scheme, host and port: where requests may go.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The scheme.
    pub scheme: Scheme,
    /// The host (WHATWG-normalised).
    pub host: Host,
    /// The port (the scheme's default if the URL has none).
    pub port: Port,
}

impl Origin {
    /// The origin of an http(s) URL.
    ///
    /// # Errors
    ///
    /// [`DavError::Href`] if it is not an http(s) URL with a host.
    pub fn of(url: &Url) -> Result<Self, DavError> {
        let invalid = || DavError::Href(nebula_dav_proto::HrefError::Url);
        let scheme = match url.scheme() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            _ => return Err(invalid()),
        };
        let host = url
            .host_str()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let port = url
            .port_or_known_default()
            .and_then(|port| Port::try_from(port).ok())
            .ok_or_else(invalid)?;
        Ok(Self { scheme, host, port })
    }

    /// Parses `https://host[:port]` (a path, if any, is ignored).
    ///
    /// # Errors
    ///
    /// Not an http(s) URL with a host, or user info in it.
    pub fn parse(text: &str) -> Result<Self, DavError> {
        let url = Url::parse(text).map_err(|_| DavError::Href(nebula_dav_proto::HrefError::Url))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(DavError::Href(nebula_dav_proto::HrefError::Url));
        }
        Self::of(&url)
    }
}

/// An origin the admin allows besides the server's own (`allowed_hosts`)
/// redirects and hrefs may go there, each hop checked by
/// `net-guard` again, but `Authorization` is never sent to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedOrigin {
    /// The origin.
    pub origin: Origin,
    /// Its networks (public only unless the admin says so).
    pub network: NetworkAccess,
}

/// The CalDAV/CardDAV server of every account, from the configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DavServer {
    /// Where discovery starts (`https://dav.example.com/`, or a context
    /// path like `https://cloud.example.com/remote.php/dav/`).
    pub base: Href,
    /// Its origin.
    pub origin: Origin,
    /// `private_network = true`: the server may be on a local network.
    pub network: NetworkAccess,
    /// Certificate check (`tls_verify = false` only for loopback).
    pub certificates: TlsVerify,
    /// Other origins the server may send us to.
    pub allowed_origins: Vec<AllowedOrigin>,
    /// Limits and timeouts.
    pub limits: Limits,
    /// iCloud's per-account hosts, only for the two exact iCloud origins
    /// ([`AccountHosts::for_configured`]); set by [`DavServer::new`],
    /// never by the configuration.
    account_hosts: Option<AccountHosts>,
}

impl DavServer {
    /// A server with safe defaults: platform certificate checks, public
    /// addresses only, no other origins, the default limits.
    ///
    /// ```
    /// use nebula_dav::DavServer;
    ///
    /// let server = DavServer::new("https://dav.example.com/dav/").unwrap();
    /// assert_eq!(server.base.path(), "/dav/");
    /// assert!(DavServer::new("https://alice:secret@dav.example.com/").is_err());
    /// assert!(DavServer::new("ftp://dav.example.com/").is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// The URL is not http(s), has user info, a query, or a path that
    /// would not be accepted from a server either.
    pub fn new(base_url: &str) -> Result<Self, DavError> {
        let url =
            Url::parse(base_url).map_err(|_| DavError::Href(nebula_dav_proto::HrefError::Url))?;
        let base = Href::from_url(&url)?.as_collection();
        let origin = Origin::of(base.url())?;
        let account_hosts = AccountHosts::for_configured(&origin);
        Ok(Self {
            account_hosts,
            base,
            origin,
            network: NetworkAccess::PublicOnly,
            certificates: TlsVerify::Platform,
            allowed_origins: Vec::new(),
            limits: Limits::default(),
        })
    }

    /// Allows (or not) a server on a private network (`private_network`).
    #[must_use]
    pub const fn with_private_network(mut self, allowed: bool) -> Self {
        self.network = if allowed {
            NetworkAccess::PrivateAllowed
        } else {
            NetworkAccess::PublicOnly
        };
        self
    }

    /// Sets the certificate check.
    #[must_use]
    pub const fn with_certificates(mut self, certificates: TlsVerify) -> Self {
        self.certificates = certificates;
        self
    }

    /// Adds an allowed origin.
    #[must_use]
    pub fn with_allowed_origin(mut self, origin: AllowedOrigin) -> Self {
        self.allowed_origins.push(origin);
        self
    }

    /// Sets the limits.
    #[must_use]
    pub const fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The per-account hosts of this server (iCloud only).
    #[must_use]
    pub const fn account_hosts(&self) -> Option<&AccountHosts> {
        self.account_hosts.as_ref()
    }

    /// Fake per-account hosts for tests (`test-account-hosts` feature,
    /// dev-dependencies only; release builds refuse the feature).
    #[cfg(any(test, feature = "test-account-hosts"))]
    #[must_use]
    pub fn with_test_account_hosts(mut self, hosts: AccountHosts) -> Self {
        self.account_hosts = Some(hosts);
        self
    }

    /// The href may be used without an account host: on the server's
    /// origin or an allowed one. [`DavClient::check_origin`](crate::DavClient::check_origin)
    /// also knows the account's iCloud host.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`].
    pub fn check_origin(&self, href: &Href) -> Result<(), DavError> {
        let origin = Origin::of(href.url())?;
        self.trust(&origin, AccountHost::Known(None))
            .map(|_| ())
            .ok_or(DavError::OriginNotAllowed)
    }
}

/// The account's login for HTTP Basic (RFC 7617), the same as for IMAP.
/// Only sent over TLS (or to a loopback server), only to
/// the server's own origin, and never logged.
#[derive(Clone)]
pub struct Credentials {
    username: String,
    password: SecretString,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(..)")
    }
}

impl Credentials {
    /// A login.
    ///
    /// # Errors
    ///
    /// [`DavError::InvalidCredentials`] if the user name is empty or has a
    /// `:` or control characters (Basic cannot carry them).
    pub fn new(username: &str, password: SecretString) -> Result<Self, DavError> {
        if username.is_empty() || username.contains(':') || username.chars().any(char::is_control) {
            return Err(DavError::InvalidCredentials);
        }
        if password.expose_secret().chars().any(char::is_control) {
            return Err(DavError::InvalidCredentials);
        }
        Ok(Self {
            username: username.to_owned(),
            password,
        })
    }

    /// Whether these are the same user name and password as `other`
    /// (compared in constant time; the password is never exposed).
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq as _;
        let names = self.username.as_bytes().ct_eq(other.username.as_bytes());
        let passwords = self
            .password
            .expose_secret()
            .as_bytes()
            .ct_eq(other.password.expose_secret().as_bytes());
        bool::from(names & passwords)
    }

    /// The `Authorization` header value, marked sensitive (hyper and our
    /// logs never print it). Our own copies are zeroized; buffers are
    /// sized up front so they never reallocate and leave copies behind.
    /// (hyper keeps its own copy for the request, which we cannot wipe.)
    pub(crate) fn header(&self) -> Result<http::HeaderValue, DavError> {
        let password = self.password.expose_secret();
        let mut plain = Zeroizing::new(String::with_capacity(
            self.username.len() + 1 + password.len(),
        ));
        plain.push_str(&self.username);
        plain.push(':');
        plain.push_str(password);
        let mut encoded = Zeroizing::new(String::with_capacity(6 + plain.len().div_ceil(3) * 4));
        encoded.push_str("Basic ");
        STANDARD.encode_string(plain.as_bytes(), &mut encoded);
        let mut value =
            http::HeaderValue::from_str(&encoded).map_err(|_| DavError::InvalidCredentials)?;
        value.set_sensitive(true);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::{Credentials, DavServer, Origin};

    #[test]
    fn same_as() {
        let a = Credentials::new("alice", SecretString::from("one")).unwrap();
        let same = Credentials::new("alice", SecretString::from("one")).unwrap();
        let other_password = Credentials::new("alice", SecretString::from("two")).unwrap();
        let other_user = Credentials::new("bob", SecretString::from("one")).unwrap();
        assert!(a.same_as(&same));
        assert!(!a.same_as(&other_password));
        assert!(!a.same_as(&other_user));
    }

    #[test]
    fn credentials() {
        let ok = Credentials::new("alice@example.com", SecretString::from("p:ss wörd")).unwrap();
        let header = ok.header().unwrap();
        assert!(header.is_sensitive());
        assert_eq!(
            header.to_str().unwrap(),
            "Basic YWxpY2VAZXhhbXBsZS5jb206cDpzcyB3w7ZyZA=="
        );
        assert_eq!(format!("{ok:?}"), "Credentials(..)");
        for bad in ["", "a:b", "a\nb"] {
            assert!(Credentials::new(bad, SecretString::from("x")).is_err());
        }
        assert!(Credentials::new("a", SecretString::from("x\r\ny")).is_err());
    }

    #[test]
    fn origins() {
        let server = DavServer::new("http://127.0.0.1:5232").unwrap();
        assert_eq!(server.base.path(), "/");
        assert_eq!(
            server.origin,
            Origin::parse("http://127.0.0.1:5232/x").unwrap()
        );
        assert_ne!(
            server.origin,
            Origin::parse("https://127.0.0.1:5232").unwrap()
        );
        assert_ne!(
            server.origin,
            Origin::parse("http://localhost:5232").unwrap()
        );
        let default_port = Origin::parse("https://dav.example.com").unwrap();
        assert_eq!(
            default_port,
            Origin::parse("https://DAV.example.com:443/").unwrap()
        );
        // A configured path without a final slash is a collection.
        assert_eq!(
            DavServer::new("https://a.example/dav").unwrap().base.path(),
            "/dav/"
        );
    }
}
