// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Where requests may go and which of them carry the login
//! decided in one place: [`DavServer::trust`].
//!
//! - The configured origin (`url` or `contacts_url`) gets the login.
//! - **iCloud only**, a built-in and exact exception ([`AccountHosts`]):
//!   with the configured origin exactly `https://caldav.icloud.com` (port
//!   443), the host `p<1–4 digits>-caldav.icloud.com` on https and port
//!   443 is the account's own server too (and `p<digits>-contacts…` for
//!   `https://contacts.icloud.com`). Discovery may follow any host of the
//!   pattern; then the client keeps the one it found (the account's host)
//!   and only that one gets the login afterwards. Public addresses only,
//!   whatever `private_network` says.
//! - An origin of `allowed_hosts`: requests, never the login.
//! - Anything else: nothing.
//!
//! It is not a "subdomains are trusted" rule (a subdomain can be hosted by
//! a third party, taken over, or given to users): only Apple's two
//! services, only that label shape, only right below `icloud.com`.

use std::borrow::Cow;

use net_guard::{Host, NetworkAccess, Port};

use crate::config::{DavServer, Origin, Scheme};

/// Most digits in the partition number of an iCloud host. Seen in public
/// sources: `p04`, `p22`, `p67`, `p102` (two and three digits, zero
/// padded below 10); four leaves room to grow without allowing any name.
pub const MAX_PARTITION_DIGITS: usize = 4;

/// Apple's per-account DAV hosts for one service:
/// `p<digits>-<service>.<domain>`, one scheme and one port.
///
/// The only production rules are those of [`AccountHosts::for_configured`]
/// (iCloud); there is no setting for others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountHosts {
    /// `caldav` or `contacts`: the last part of the first label.
    service: &'static str,
    /// The labels after the first one (`icloud.com`).
    domain: Cow<'static, str>,
    scheme: Scheme,
    port: Port,
    /// Networks for these hosts: public only, except for test rules.
    network: NetworkAccess,
}

/// The configured origins that enable a rule, and the service of each.
const ICLOUD: [(&str, &str); 2] = [
    ("caldav.icloud.com", "caldav"),
    ("contacts.icloud.com", "contacts"),
];
const ICLOUD_DOMAIN: &str = "icloud.com";

impl AccountHosts {
    /// The rule for a configured origin: only exactly
    /// `https://caldav.icloud.com` (calendar hosts) and
    /// `https://contacts.icloud.com` (contacts hosts), on port 443.
    #[must_use]
    pub fn for_configured(origin: &Origin) -> Option<Self> {
        if origin.scheme != Scheme::Https || origin.port != Port::HTTPS {
            return None;
        }
        let Host::Domain(name) = &origin.host else {
            return None;
        };
        let &(_, service) = ICLOUD
            .iter()
            .find(|(configured, _)| name.as_str() == *configured)?;
        Some(Self {
            service,
            domain: Cow::Borrowed(ICLOUD_DOMAIN),
            scheme: Scheme::Https,
            port: Port::HTTPS,
            network: NetworkAccess::PublicOnly,
        })
    }

    /// A rule for fake servers (`p<digits>-<service>.<domain>` on a
    /// loopback port, with the test's networks). Only built with the
    /// `test-account-hosts` feature, which only dev-dependencies enable
    /// and release builds refuse (see `lib.rs`); the configuration has no
    /// way to reach it.
    #[cfg(any(test, feature = "test-account-hosts"))]
    #[must_use]
    pub fn for_tests(
        service: &'static str,
        domain: &str,
        scheme: Scheme,
        port: Port,
        network: NetworkAccess,
    ) -> Self {
        Self {
            service,
            domain: Cow::Owned(domain.to_owned()),
            scheme,
            port,
            network,
        }
    }

    /// The service of the hosts (`caldav` or `contacts`).
    #[must_use]
    pub const fn service(&self) -> &'static str {
        self.service
    }

    /// Whether `origin` is one of these hosts: same scheme and port, a
    /// domain name (never an IP), first label `p<1–4 ASCII digits>-<service>`
    /// and the rest exactly the rule's domain. The name is the one WHATWG
    /// parsing gives (lowercase, punycode), so `P42-CalDAV.iCloud.com` is
    /// the same name and a Unicode lookalike is `xn--…`, which never
    /// matches; a trailing dot is a different name and is refused.
    #[must_use]
    pub fn matches(&self, origin: &Origin) -> bool {
        origin.scheme == self.scheme && origin.port == self.port && self.partition(origin).is_some()
    }

    /// The partition number of an account host (`p42-…` → 42).
    #[must_use]
    pub fn partition(&self, origin: &Origin) -> Option<u16> {
        let Host::Domain(name) = &origin.host else {
            return None;
        };
        let (first, rest) = name.as_str().split_once('.')?;
        if rest != self.domain {
            return None;
        }
        let digits = first
            .strip_prefix('p')?
            .strip_suffix(self.service)?
            .strip_suffix('-')?;
        if digits.is_empty()
            || digits.len() > MAX_PARTITION_DIGITS
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        digits.parse().ok()
    }

    pub(crate) const fn network(&self) -> NetworkAccess {
        self.network
    }
}

/// What may go to an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The account's own server (the configured origin, or its iCloud
    /// host): requests with the login.
    Login(NetworkAccess),
    /// An origin of `allowed_hosts`: requests, never the login.
    NoLogin(NetworkAccess),
}

impl Trust {
    /// The networks the connection may use.
    #[must_use]
    pub const fn network(self) -> NetworkAccess {
        match self {
            Self::Login(network) | Self::NoLogin(network) => network,
        }
    }

    /// Whether the login goes with the request.
    #[must_use]
    pub const fn sends_login(self) -> bool {
        matches!(self, Self::Login(_))
    }
}

/// What the client knows of the account's own host (iCloud).
#[derive(Debug, Clone, Copy)]
pub enum AccountHost<'a> {
    /// Discovery is looking for it: any host of the pattern is the
    /// account's.
    Discovering,
    /// Found by discovery (or not yet / none): only that one is.
    Known(Option<&'a Origin>),
}

impl DavServer {
    /// What may go to `origin` (the whole rule of this module).
    #[must_use]
    pub fn trust(&self, origin: &Origin, account: AccountHost<'_>) -> Option<Trust> {
        if *origin == self.origin {
            return Some(Trust::Login(self.network));
        }
        if let Some(rule) = self.account_hosts()
            && rule.matches(origin)
        {
            let ours = match account {
                AccountHost::Discovering => true,
                AccountHost::Known(known) => known == Some(origin),
            };
            if ours {
                return Some(Trust::Login(rule.network()));
            }
        }
        self.allowed_origins
            .iter()
            .find(|allowed| allowed.origin == *origin)
            .map(|allowed| Trust::NoLogin(allowed.network))
    }
}

#[cfg(test)]
mod tests {
    use net_guard::{NetworkAccess, Port};
    use proptest::prelude::*;

    use super::{AccountHost, AccountHosts, Trust};
    use crate::config::{AllowedOrigin, DavServer, Origin, Scheme};

    fn origin(text: &str) -> Origin {
        Origin::parse(text).unwrap()
    }

    /// Whether `text` parses and is one of the rule's hosts.
    fn matches(rule: &AccountHosts, text: &str) -> bool {
        Origin::parse(text).is_ok_and(|parsed| rule.matches(&parsed))
    }

    fn rule(configured: &str) -> Option<AccountHosts> {
        AccountHosts::for_configured(&origin(configured))
    }

    #[test]
    fn only_the_two_exact_icloud_origins_enable_a_rule() {
        assert_eq!(
            rule("https://caldav.icloud.com/").map(|r| r.service()),
            Some("caldav")
        );
        assert_eq!(
            rule("https://contacts.icloud.com").map(|r| r.service()),
            Some("contacts")
        );
        // Same origin, other spellings of it.
        assert!(rule("https://CalDAV.iCloud.com:443/x/").is_some());
        for other in [
            "http://caldav.icloud.com/",
            "https://caldav.icloud.com:8443/",
            "https://caldav.icloud.com./",
            "https://p42-caldav.icloud.com/",
            "https://icloud.com/",
            "https://www.icloud.com/",
            "https://caldav.icloud.com.evil.com/",
            "https://caldav.icloud.co/",
            "https://evilcaldav.icloud.com/",
            "https://carddav.icloud.com/",
            "https://caldav.me.com/",
            "https://caldav.fastmail.com/",
            "https://17.0.0.1/",
            "https://[2620:149::1]/",
        ] {
            assert!(rule(other).is_none(), "{other}");
        }
    }

    #[test]
    fn calendar_hosts() {
        let caldav = rule("https://caldav.icloud.com/").unwrap();
        for good in [
            "https://p42-caldav.icloud.com",
            "https://p04-caldav.icloud.com/",
            "https://p1-caldav.icloud.com:443/1234/calendars/",
            "https://p102-caldav.icloud.com",
            "https://p9999-caldav.icloud.com",
            "https://p0-caldav.icloud.com",
            // The same name once normalised (WHATWG: lowercase; fullwidth
            // letters map to ASCII by IDNA): DNS and TLS see Apple's host.
            "https://P42-CalDAV.iCloud.COM",
            "https://ｐ42-caldav.icloud.com",
        ] {
            assert!(caldav.matches(&origin(good)), "{good}");
        }
        assert_eq!(
            caldav.partition(&origin("https://p042-caldav.icloud.com")),
            Some(42)
        );
        for bad in [
            // Other shapes of label.
            "https://p-caldav.icloud.com",
            "https://p12345-caldav.icloud.com",
            "https://42-caldav.icloud.com",
            "https://q42-caldav.icloud.com",
            "https://pp42-caldav.icloud.com",
            "https://p42caldav.icloud.com",
            "https://p42--caldav.icloud.com",
            "https://p42-caldav2.icloud.com",
            "https://p4a-caldav.icloud.com",
            "https://p-42-caldav.icloud.com",
            "https://p+42-caldav.icloud.com",
            "https://p42-caldav-x.icloud.com",
            "https://x.p42-caldav.icloud.com",
            "https://p42-caldav.x.icloud.com",
            "https://caldav.icloud.com",
            "https://blog.icloud.com",
            "https://icloud.com",
            // Another service: contacts hosts are not calendar hosts.
            "https://p42-contacts.icloud.com",
            // Another domain.
            "https://p42-caldav.icloud.com.evil.com",
            "https://p42-caldav.icloud.co",
            "https://p42-caldav.icloud.com.",
            "https://p42-caldav.icloudxcom",
            "https://p42-caldav.icloud.com.icloud.com",
            "https://p42-caldav.evil-icloud.com",
            "https://p42-caldav.me.com",
            // Another scheme or port.
            "http://p42-caldav.icloud.com",
            "https://p42-caldav.icloud.com:8443",
            "https://p42-caldav.icloud.com:80",
            // Lookalikes: Unicode becomes punycode, never the name.
            "https://р42-caldav.icloud.com",
            "https://p42-сaldav.icloud.com",
            "https://p42-caldav.іcloud.com",
            "https://p٤٢-caldav.icloud.com",
            "https://p42-caldav.icloud.com\u{3002}evil.com",
            // IP literals.
            "https://17.253.144.10",
            "https://[::1]",
        ] {
            assert!(!matches(&caldav, bad), "{bad}");
        }
        // Not even an origin: user info.
        assert!(Origin::parse("https://p42-caldav.icloud.com@evil.com/").is_err());
        assert!(Origin::parse("https://evil.com@p42-caldav.icloud.com/").is_err());
    }

    #[test]
    fn contacts_hosts() {
        let contacts = rule("https://contacts.icloud.com/").unwrap();
        assert!(contacts.matches(&origin("https://p42-contacts.icloud.com")));
        assert!(contacts.matches(&origin("https://p103-contacts.icloud.com/")));
        for bad in [
            "https://p42-caldav.icloud.com",
            "https://p42-contacts.icloud.com.evil.com",
            "https://p42-contacts.icloud.com:8443",
            "http://p42-contacts.icloud.com",
            "https://p42-contact.icloud.com",
            "https://p42-carddav.icloud.com",
        ] {
            assert!(!matches(&contacts, bad), "{bad}");
        }
    }

    #[test]
    fn trust_of_each_origin() {
        let server = DavServer::new("https://caldav.icloud.com/")
            .unwrap()
            .with_private_network(true)
            .with_allowed_origin(AllowedOrigin {
                origin: origin("https://cdn.example.com"),
                network: NetworkAccess::PrivateAllowed,
            });
        let own = origin("https://p42-caldav.icloud.com");
        let other = origin("https://p43-caldav.icloud.com");
        let contacts = origin("https://p42-contacts.icloud.com");
        // The configured origin: the login, with its networks.
        assert_eq!(
            server.trust(
                &origin("https://caldav.icloud.com"),
                AccountHost::Known(None)
            ),
            Some(Trust::Login(NetworkAccess::PrivateAllowed))
        );
        // A host of the pattern while discovering: the login, public only.
        assert_eq!(
            server.trust(&own, AccountHost::Discovering),
            Some(Trust::Login(NetworkAccess::PublicOnly))
        );
        // Not yet known (or none): nothing.
        assert_eq!(server.trust(&own, AccountHost::Known(None)), None);
        // Known: only that one.
        assert_eq!(
            server.trust(&own, AccountHost::Known(Some(&own))),
            Some(Trust::Login(NetworkAccess::PublicOnly))
        );
        assert_eq!(server.trust(&other, AccountHost::Known(Some(&own))), None);
        // The contacts pattern on the calendar server: never.
        for account in [
            AccountHost::Discovering,
            AccountHost::Known(Some(&contacts)),
        ] {
            assert_eq!(server.trust(&contacts, account), None);
        }
        // Allowed hosts: never the login.
        assert_eq!(
            server.trust(&origin("https://cdn.example.com"), AccountHost::Discovering),
            Some(Trust::NoLogin(NetworkAccess::PrivateAllowed))
        );
        for refused in [
            "https://blog.icloud.com",
            "https://p42-caldav.icloud.com.evil.com",
            "https://p42-caldav.icloud.com:8443",
            "http://p42-caldav.icloud.com",
        ] {
            assert_eq!(
                server.trust(&origin(refused), AccountHost::Discovering),
                None,
                "{refused}"
            );
        }
        // Any other configured server: no pattern at all.
        let other_server = DavServer::new("https://dav.example.com/").unwrap();
        assert!(other_server.account_hosts().is_none());
        assert_eq!(other_server.trust(&own, AccountHost::Discovering), None);
        let fastmail = DavServer::new("https://caldav.fastmail.com/").unwrap();
        assert_eq!(fastmail.trust(&own, AccountHost::Discovering), None);
    }

    #[test]
    fn test_rules_follow_their_own_shape() {
        let rule = AccountHosts::for_tests(
            "caldav",
            "icloud.test",
            Scheme::Http,
            Port::try_from(8080).unwrap(),
            NetworkAccess::PrivateAllowed,
        );
        assert!(rule.matches(&origin("http://p42-caldav.icloud.test:8080")));
        assert!(!rule.matches(&origin("http://p42-caldav.icloud.test:8081")));
        assert!(!rule.matches(&origin("https://p42-caldav.icloud.test:8080")));
        assert!(!rule.matches(&origin("http://p42-caldav.icloud.com:8080")));
    }

    proptest! {
        /// Every host of the shape matches; changing anything does not.
        #[test]
        fn shape(n in 0_u16..10_000, label in "[a-z0-9-]{0,12}", tld in "[a-z]{2,6}") {
            let caldav = rule("https://caldav.icloud.com/").unwrap();
            let host = origin(&format!("https://p{n}-caldav.icloud.com"));
            prop_assert!(caldav.matches(&host));
            prop_assert_eq!(caldav.partition(&host), Some(n));
            let mut refused = vec![
                format!("https://{label}x.icloud.{tld}"),
                format!("https://p{n}-caldav.icloud.com.{tld}"),
                format!("https://p{n}-caldav.{tld}.icloud.com"),
                format!("https://p{n}-contacts.icloud.com"),
                format!("http://p{n}-caldav.icloud.com"),
                format!("https://p{n}-caldav.icloud.com:{}", 444 + n),
            ];
            if !label.is_empty() {
                refused.push(format!("https://{label}p{n}-caldav.icloud.com"));
            }
            for text in &refused {
                prop_assert!(!matches(&caldav, text), "{}", text);
            }
        }

        /// Arbitrary text never panics, and a match is always the exact
        /// shape.
        #[test]
        fn arbitrary_hosts(text in "\\PC{0,40}") {
            let caldav = rule("https://caldav.icloud.com/").unwrap();
            if let Ok(parsed) = Origin::parse(&format!("https://{text}/"))
                && caldav.matches(&parsed)
            {
                let name = parsed.host.to_string();
                let (first, rest) = name.split_once('.').unwrap();
                prop_assert_eq!(rest, "icloud.com");
                prop_assert!(first.starts_with('p') && first.ends_with("-caldav"));
                prop_assert_eq!(parsed.port, Port::HTTPS);
            }
        }
    }
}
