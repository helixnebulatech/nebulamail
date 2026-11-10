// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! URLs of things outside the DAV server: where a webcal subscription
//! comes from (`CS:source`) and the public link of a published calendar
//! (`CS:publish-url`). The bridge only shows them or hands them to the
//! DAV server; it never connects to one,
//! so they are checked for shape, not passed through `net-guard`.

use std::fmt;

use url::Url;

/// A URL the bridge never fetches: `http`, `https`, `webcal` or
/// `webcals`, with a host, without credentials, spaces or control
/// characters, at most [`ExternalUrl::MAX_LEN`] bytes. Kept as the
/// normalised text of [`Url`]. `Debug` hides it (a public link is a
/// secret token).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ExternalUrl(String);

/// Which schemes a use accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schemes {
    /// What a server may say: `http(s)` and `webcal(s)`.
    Any,
    /// A new subscription: `https` or `webcal(s)` (plain `http` would
    /// send the calendar in clear).
    Subscription,
}

impl ExternalUrl {
    /// Longest URL kept, in bytes.
    pub const MAX_LEN: usize = 2048;

    fn parse_with(text: &str, schemes: Schemes) -> Option<Self> {
        let text = text.trim();
        if text.is_empty()
            || text.len() > Self::MAX_LEN
            || text.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return None;
        }
        let url = Url::parse(text).ok()?;
        let scheme_ok = matches!(
            (url.scheme(), schemes),
            ("https" | "webcal" | "webcals", _) | ("http", Schemes::Any)
        );
        let host_ok = url.host_str().is_some_and(|host| !host.is_empty());
        let no_credentials = url.username().is_empty() && url.password().is_none();
        let normalised = url.as_str();
        (scheme_ok && host_ok && no_credentials && normalised.len() <= Self::MAX_LEN)
            .then(|| Self(normalised.to_owned()))
    }

    /// A URL as a server gave it (a subscription's source, a public
    /// link): `http`, `https`, `webcal` or `webcals`.
    ///
    /// ```
    /// use nebula_dav_proto::ExternalUrl;
    ///
    /// let url = ExternalUrl::parse("webcal://example.com/Holidays.ics").unwrap();
    /// assert_eq!(url.as_str(), "webcal://example.com/Holidays.ics");
    /// assert!(ExternalUrl::parse("http://127.0.0.1:5234/remote.php/dav/public-calendars/X").is_some());
    /// assert!(ExternalUrl::parse("https://user:pw@example.com/x.ics").is_none());
    /// assert!(ExternalUrl::parse("file:///etc/passwd").is_none());
    /// assert!(ExternalUrl::parse("javascript:alert(1)").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_with(text, Schemes::Any)
    }

    /// A URL for a new subscription: as [`ExternalUrl::parse`], but only
    /// `https`, `webcal` or `webcals`.
    ///
    /// ```
    /// use nebula_dav_proto::ExternalUrl;
    ///
    /// assert!(ExternalUrl::subscription("https://example.com/x.ics").is_some());
    /// assert!(ExternalUrl::subscription("webcals://example.com/x.ics").is_some());
    /// assert!(ExternalUrl::subscription("http://example.com/x.ics").is_none());
    /// assert!(ExternalUrl::subscription("https://a@example.com/x.ics").is_none());
    /// ```
    #[must_use]
    pub fn subscription(text: &str) -> Option<Self> {
        Self::parse_with(text, Schemes::Subscription)
    }

    /// The URL text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExternalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExternalUrl(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::ExternalUrl;

    #[test]
    fn shapes() {
        for bad in [
            "",
            " ",
            "example.com/x.ics",
            "ftp://example.com/x",
            "https://",
            "https://ex ample.com/",
            "https://example.com/\u{0}",
            "https://example.com/\na",
            "https://:pw@example.com/",
            "data:text/calendar,BEGIN",
        ] {
            assert!(ExternalUrl::parse(bad).is_none(), "{bad:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(ExternalUrl::MAX_LEN));
        assert!(ExternalUrl::parse(&long).is_none());
        assert_eq!(
            ExternalUrl::parse("  HTTPS://Example.COM/a b  ").map(|u| u.0),
            None,
            "inner spaces"
        );
        assert_eq!(
            ExternalUrl::parse(" HTTPS://Example.COM/Cal.ics ").map(|u| u.0),
            Some("https://example.com/Cal.ics".to_owned())
        );
        assert_eq!(
            format!("{:?}", ExternalUrl::parse("https://example.com/secret")),
            "Some(ExternalUrl(..))"
        );
    }
}
