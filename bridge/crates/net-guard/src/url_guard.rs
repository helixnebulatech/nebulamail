// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! URL checks, the twin of
//! `packages/net-guard/src/url.ts` and `resolve.ts`.
//!
//! For URLs that do not come from the configuration (autoconfig, HTTP
//! fetches), so only public addresses are accepted.

use url::Url;

use crate::{
    Addresses, BlockReason, Host, NetworkAccess, Port, PortPolicy, Resolver, check_ip, resolve,
};

/// A URL scheme we may fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scheme {
    /// `http`.
    Http,
    /// `https`.
    Https,
}

impl Scheme {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            _ => None,
        }
    }
}

/// The schemes and ports a URL may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPolicy {
    schemes: &'static [Scheme],
    ports: PortPolicy,
}

impl UrlPolicy {
    /// Image proxy and other public fetches: http/https on 80/443.
    pub const PROXY: Self = Self {
        schemes: &[Scheme::Http, Scheme::Https],
        ports: PortPolicy::PROXY,
    };
    /// A server the user typed (JMAP, OIDC, autoconfig): HTTPS on 443 only.
    pub const HTTPS: Self = Self {
        schemes: &[Scheme::Https],
        ports: PortPolicy::HTTPS,
    };
}

/// A URL that passed the static checks: allowed scheme, no user info,
/// allowed port, and if the host is an IP literal, a public one. A name
/// still needs [`resolve_url`]. Only [`parse_target_url`] creates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    url: Url,
    scheme: Scheme,
    host: Host,
    port: Port,
}

impl ParsedUrl {
    /// The normalized URL (WHATWG serialization), without fragment.
    /// Do not log it (§8.1 rule 12).
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// The scheme.
    #[must_use]
    pub const fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The host (normalized).
    #[must_use]
    pub const fn host(&self) -> &Host {
        &self.host
    }

    /// The port, with the scheme default applied.
    #[must_use]
    pub const fn port(&self) -> Port {
        self.port
    }
}

/// A URL whose every address is public. Only [`resolve_url`] creates it.
/// Connect to [`ValidatedUrl::addresses`] and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedUrl {
    parsed: ParsedUrl,
    addresses: Addresses,
}

impl ValidatedUrl {
    /// The URL and its parts.
    #[must_use]
    pub const fn parsed(&self) -> &ParsedUrl {
        &self.parsed
    }

    /// The validated addresses.
    #[must_use]
    pub const fn addresses(&self) -> &Addresses {
        &self.addresses
    }
}

/// Parse and statically check a URL. Never resolves.
///
/// Checks run in a fixed order (scheme, userinfo, port, IP literal) so the
/// reason is the same as in TypeScript. Unparseable input is `scheme`
/// ("not an http(s) URL").
///
/// ```
/// use net_guard::{parse_target_url, BlockReason, UrlPolicy};
///
/// let ok = parse_target_url("https://images.example.com/a.png#x", &UrlPolicy::PROXY).unwrap();
/// assert_eq!(ok.url().as_str(), "https://images.example.com/a.png");
///
/// let check = |url| parse_target_url(url, &UrlPolicy::PROXY).unwrap_err();
/// assert_eq!(check("file:///etc/passwd"), BlockReason::Scheme);
/// assert_eq!(check("http://a.example.com@127.0.0.1/"), BlockReason::Userinfo);
/// assert_eq!(check("http://images.example.com:8080/"), BlockReason::Port);
/// assert_eq!(check("http://0x7f.1/"), BlockReason::PrivateIp);
/// ```
///
/// # Errors
///
/// The [`BlockReason`] of the first failed check.
pub fn parse_target_url(input: &str, policy: &UrlPolicy) -> Result<ParsedUrl, BlockReason> {
    let mut url = Url::parse(input).map_err(|_| BlockReason::Scheme)?;
    let scheme = Scheme::parse(url.scheme())
        .filter(|scheme| policy.schemes.contains(scheme))
        .ok_or(BlockReason::Scheme)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BlockReason::Userinfo);
    }
    let port = url
        .port_or_known_default()
        .and_then(|number| Port::try_from(number).ok())
        .ok_or(BlockReason::Port)?;
    policy.ports.check(port)?;
    // http(s) URLs always have a host; a missing one is not an http(s) URL.
    let host = url.host().map(Host::from_url).ok_or(BlockReason::Scheme)?;
    if let Host::Ip(ip) = host {
        check_ip(ip, NetworkAccess::PublicOnly).map_err(|_| BlockReason::PrivateIp)?;
    }
    url.set_fragment(None);
    Ok(ParsedUrl {
        url,
        scheme,
        host,
        port,
    })
}

/// Resolve a parsed URL and check every address (public only).
///
/// # Errors
///
/// `private-ip` or `dns`, as [`resolve`].
pub async fn resolve_url(
    parsed: ParsedUrl,
    resolver: &dyn Resolver,
) -> Result<ValidatedUrl, BlockReason> {
    let addresses = resolve(parsed.host(), NetworkAccess::PublicOnly, resolver).await?;
    Ok(ValidatedUrl { parsed, addresses })
}

/// [`parse_target_url`] + [`resolve_url`]: the full static and DNS guard.
///
/// # Errors
///
/// The [`BlockReason`] of the first failed check.
pub async fn guard_url(
    input: &str,
    resolver: &dyn Resolver,
    policy: &UrlPolicy,
) -> Result<ValidatedUrl, BlockReason> {
    let parsed = parse_target_url(input, policy).inspect_err(|reason| {
        tracing::debug!(%reason, "net-guard: URL refused");
    })?;
    let host = parsed.host().clone();
    resolve_url(parsed, resolver).await.inspect_err(|reason| {
        tracing::debug!(%host, %reason, "net-guard: URL refused");
    })
}

#[cfg(test)]
mod tests {
    use super::{UrlPolicy, guard_url, parse_target_url};
    use crate::BlockReason;
    use crate::resolve::testing::FakeDns;

    #[test]
    fn https_policy_is_stricter() {
        let policy = UrlPolicy::HTTPS;
        assert!(parse_target_url("https://jmap.example.com/", &policy).is_ok());
        assert_eq!(
            parse_target_url("http://jmap.example.com/", &policy).unwrap_err(),
            BlockReason::Scheme
        );
        assert_eq!(
            parse_target_url("https://jmap.example.com:8443/", &policy).unwrap_err(),
            BlockReason::Port
        );
    }

    #[test]
    fn parts_are_normalized() {
        let parsed = parse_target_url(
            "HTTP://Images.Example.COM:80/a b?x=1#frag",
            &UrlPolicy::PROXY,
        )
        .unwrap();
        assert_eq!(parsed.url().as_str(), "http://images.example.com/a%20b?x=1");
        assert_eq!(parsed.host().to_string(), "images.example.com");
        assert_eq!(parsed.port().get(), 80);
    }

    #[tokio::test]
    async fn guard_url_resolves_names_once() {
        let dns = FakeDns::answering(&[&["93.184.215.14"]]);
        let validated = guard_url("https://images.example.com/", &dns, &UrlPolicy::PROXY)
            .await
            .unwrap();
        assert_eq!(validated.addresses().first().to_string(), "93.184.215.14");
        assert_eq!(validated.parsed().host().to_string(), "images.example.com");
        assert_eq!(dns.calls(), ["images.example.com"]);
    }
}
