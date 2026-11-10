// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Host names and IP literals.
//!
//! Parsed with WHATWG URL semantics (the `url` crate): IPv4 numbers in any
//! form (`2130706433`, `0x7f.1`, `0177.0.0.1`, `127.1`), Unicode dots,
//! IDNA to punycode and lowercase. Safety is never decided on the text:
//! IP literals become [`IpAddr`] and names are resolved and checked.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

use crate::ParseError;

/// A DNS name in ASCII (punycode), lowercase, as WHATWG URL parsing gives
/// it. It is never an IP address. A trailing dot is kept (`localhost.`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainName(String);

impl DomainName {
    /// The name, as it is resolved.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name for TLS (SNI and certificate checks): without the trailing
    /// dot, which SNI does not allow (RFC 6066 §3).
    #[must_use]
    pub fn tls_name(&self) -> &str {
        self.0.strip_suffix('.').unwrap_or(&self.0)
    }

    /// True if the name can only be a local or special-use host: a single
    /// label (`intranet`, `localhost`) or one under a suffix that is never
    /// public (`.internal`, `.local`, `.onion`, ...). Case and trailing
    /// dots are ignored.
    ///
    /// [`resolve`](crate::resolve) refuses these names with
    /// [`NetworkAccess::PublicOnly`](crate::NetworkAccess::PublicOnly)
    /// before any lookup (`dns`), like Stalwart 1.0: with split-horizon DNS
    /// they may resolve to an address that looks public but is only
    /// reachable from inside. Never applied to `PrivateAllowed`, where the
    /// admin chose the host. The twin of `isNonPublicName` in
    /// `packages/net-guard/src/host-name.ts`.
    ///
    /// ```
    /// use net_guard::Host;
    ///
    /// let name = |text: &str| match text.parse::<Host>().unwrap() {
    ///     Host::Domain(name) => name.is_non_public(),
    ///     _ => unreachable!(),
    /// };
    /// assert!(name("intranet"));
    /// assert!(name("grafana.internal"));
    /// assert!(name("Printer.LOCAL."));
    /// assert!(!name("images.example.com."));
    /// ```
    #[must_use]
    pub fn is_non_public(&self) -> bool {
        let name = self.0.trim_end_matches('.');
        name.rsplit_once('.').is_none_or(|(_, tld)| {
            NON_PUBLIC_TLDS
                .iter()
                .any(|suffix| suffix.eq_ignore_ascii_case(tld))
        })
    }
}

/// Top-level labels that are never a public DNS name (same list as the
/// TypeScript `NON_PUBLIC_TLDS`).
///
/// A static list instead of the Public Suffix List: no dependency to keep up
/// to date, and the PSL would also refuse `.test` and `.example`, which the
/// tests use as stand-in public names. Those are left out on purpose: RFC
/// 6761 §6.2/§6.5 keeps them out of the root, so a real resolver answers
/// NXDOMAIN (already `dns`).
const NON_PUBLIC_TLDS: &[&str] = &[
    // RFC 6761 §6.3: loopback.
    "localhost",
    // RFC 6761 §6.4: APIs should answer "no such name" at once.
    "invalid",
    // RFC 6762: multicast DNS on the local link.
    "local",
    // RFC 7686: Tor.
    "onion",
    // RFC 9476: names of non-DNS systems.
    "alt",
    // RFC 3172 infrastructure, with home.arpa (RFC 8375) and reverse DNS.
    "arpa",
    // ICANN 2024: reserved for private use, never delegated.
    "internal",
    // ICANN name collision (2018): never delegated, widely used inside.
    "corp",
    "home",
    "mail",
    // Common private conventions (routers, Linux defaults, intranets).
    "lan",
    "intranet",
    "localdomain",
];

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A destination host: an IP literal or a name to resolve.
///
/// Parse it from the configuration with [`str::parse`]:
///
/// ```
/// use std::net::{IpAddr, Ipv4Addr};
/// use net_guard::Host;
///
/// let host: Host = "IMAP.Example.COM".parse().unwrap();
/// assert_eq!(host.to_string(), "imap.example.com");
///
/// // Every IPv4 spelling becomes the same address.
/// let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// for text in ["127.0.0.1", "2130706433", "0x7f.1", "0177.0.0.1", "127.1"] {
///     assert_eq!(text.parse::<Host>().unwrap(), Host::Ip(loopback));
/// }
///
/// // IPv6 with or without brackets.
/// assert_eq!("[::1]".parse::<Host>().unwrap(), "::1".parse::<Host>().unwrap());
///
/// // IDNA: Unicode names become punycode.
/// assert_eq!("bücher.example".parse::<Host>().unwrap().to_string(), "xn--bcher-kva.example");
///
/// // A host is not a URL: no port, path or user name.
/// assert!("imap.example.com:993".parse::<Host>().is_err());
/// assert!("user@imap.example.com".parse::<Host>().is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Host {
    /// An IP literal. Still checked before connecting.
    Ip(IpAddr),
    /// A name, resolved (and every address checked) before connecting.
    Domain(DomainName),
}

impl Host {
    /// Convert a host from a parsed URL.
    pub(crate) fn from_url<S: AsRef<str>>(host: url::Host<S>) -> Self {
        match host {
            url::Host::Domain(name) => Self::Domain(DomainName(name.as_ref().to_owned())),
            url::Host::Ipv4(ip) => Self::Ip(IpAddr::V4(ip)),
            url::Host::Ipv6(ip) => Self::Ip(IpAddr::V6(ip)),
        }
    }
}

impl FromStr for Host {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // A bare IPv6 (`::1`) is unambiguous: `:` cannot be in a name. The
        // std parser is strict (decimal IPv4 tail only, no zone id).
        if let Ok(ip) = s.parse::<Ipv6Addr>() {
            return Ok(Self::Ip(IpAddr::V6(ip)));
        }
        // Percent-encoding is a URL feature; a configured host never needs it.
        if s.contains('%') {
            return Err(ParseError::InvalidHost);
        }
        url::Host::parse(s)
            .map(Self::from_url)
            .map_err(|_| ParseError::InvalidHost)
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => fmt::Display::fmt(ip, f),
            Self::Domain(name) => fmt::Display::fmt(name, f),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use proptest::prelude::*;

    use super::Host;

    fn domain(text: &str) -> String {
        match text.parse::<Host>().unwrap() {
            Host::Domain(name) => name.as_str().to_owned(),
            Host::Ip(ip) => panic!("{text} parsed as IP {ip}"),
        }
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(domain("Mail.Example.com"), "mail.example.com");
        assert_eq!(domain("localhost."), "localhost.");
        assert_eq!(domain("127.0.0.1.nip.io"), "127.0.0.1.nip.io");
        assert_eq!(domain("ÉXAMPLE.com"), "xn--xample-9ua.com");
    }

    #[test]
    fn trailing_dot_is_dropped_for_tls_only() {
        let Host::Domain(name) = "imap.example.com.".parse::<Host>().unwrap() else {
            panic!("not a name");
        };
        assert_eq!(name.as_str(), "imap.example.com.");
        assert_eq!(name.tls_name(), "imap.example.com");
    }

    #[test]
    fn non_public_names() {
        let non_public = |text: &str| match text.parse::<Host>().unwrap() {
            Host::Domain(name) => name.is_non_public(),
            Host::Ip(ip) => panic!("{text} parsed as IP {ip}"),
        };
        for name in [
            "localhost",
            "localhost.",
            "LOCALHOST",
            "x.localhost.",
            "intranet",
            "grafana.internal",
            "metadata.google.internal",
            "printer.local",
            "Foo.LOCAL.",
            "abc.onion",
            "x.alt",
            "nas.home.arpa",
            "1.0.0.127.in-addr.arpa",
            "a.invalid",
            "fileserver.corp",
            "router.home",
            "box.mail",
            "nas.lan",
            "wiki.intranet",
            "pc.localdomain",
        ] {
            assert!(non_public(name), "{name}");
        }
        for name in [
            "images.example.com",
            "images.example.com.",
            "local.example.com",
            "localhost.example.com",
            "internal.example.org",
            "onion.example",
            "img.test",
            "mail.example",
            "bücher.example",
            "a.b.c.d.co.uk",
        ] {
            assert!(!non_public(name), "{name}");
        }
    }

    #[test]
    fn ipv4_forms_are_normalized() {
        let ip = |text: &str| text.parse::<Host>().unwrap();
        assert_eq!(
            ip("127。0。0。1"),
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(ip("0"), Host::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert_eq!(
            ip("10.0.0.5"),
            Host::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)))
        );
        assert_eq!(ip("[::ffff:127.0.0.1]").to_string(), "::ffff:127.0.0.1");
    }

    #[test]
    fn rejects_what_is_not_a_host() {
        for bad in [
            "",
            " ",
            "imap.example.com:993",
            "[::1]:993",
            "imap example.com",
            "user@host",
            "http://host",
            "host/path",
            "fe80::1%eth0",
            "[fe80::1%25eth0]",
            "1.2.3.4.5",
            "256.0.0.1",
            "imap%2eexample.com",
        ] {
            assert!(bad.parse::<Host>().is_err(), "{bad:?}");
        }
    }

    proptest! {
        #[test]
        fn decimal_number_is_that_ipv4(n in any::<u32>()) {
            let host: Host = n.to_string().parse().unwrap();
            prop_assert_eq!(host, Host::Ip(IpAddr::V4(Ipv4Addr::from(n))));
        }

        #[test]
        fn dotted_ipv4_round_trips(ip in any::<Ipv4Addr>()) {
            let host: Host = ip.to_string().parse().unwrap();
            prop_assert_eq!(host, Host::Ip(IpAddr::V4(ip)));
        }

        #[test]
        fn never_panics(text in "\\PC{0,40}") {
            let _ = text.parse::<Host>();
        }
    }
}
