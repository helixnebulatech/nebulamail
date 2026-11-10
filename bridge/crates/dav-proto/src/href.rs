// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hrefs: where a server says a resource is.
//!
//! An href from a server is resolved against the URL of the request it
//! answered (RFC 4918 §8.3), like a browser would, but stricter: it is
//! refused, never "fixed", when it has a dot segment (`..`, also as
//! `%2e%2E`), an encoded slash or backslash, an empty segment, a query, a
//! fragment, control characters, or user info. Each segment is
//! percent-decoded (it must be UTF-8) and encoded again in one canonical
//! way, so two spellings of the same path compare equal (libdav does the
//! same normalisation) and the path we send back is always well-formed.
//!
//! Whether an href may be used at all (same origin as the configured
//! server, or an origin the admin allowed) is the client's decision, with
//! [`Href::origin`]; [`Href::is_member_of`] checks that a resource is a
//! direct member of its collection on the same origin.

use std::hash::{Hash, Hasher};

use url::{Origin, Url};

use crate::HrefError;
use crate::percent::{decode_segment, encode_segment};

/// A resolved, normalised http(s) location: origin and path, without
/// query or fragment.
#[derive(Debug, Clone)]
pub struct Href {
    url: Url,
    segments: Vec<String>,
    collection: bool,
}

impl PartialEq for Href {
    fn eq(&self, other: &Self) -> bool {
        // The canonical URL determines origin, segments and trailing slash.
        self.url.as_str() == other.url.as_str()
    }
}

impl Eq for Href {}

impl Hash for Href {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.as_str().hash(state);
    }
}

/// What [`Href::same_location`] compares (origin and decoded segments,
/// without the trailing slash), as a value: see [`Href::location_key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocationKey {
    scheme: String,
    host: String,
    port: Option<u16>,
    segments: Vec<String>,
}

fn is_http(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// `scheme:` at the start (RFC 3986 §3.1).
fn has_scheme(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    let Some(scheme) = text.get(..colon) else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

impl Href {
    /// Longest href text accepted.
    pub const MAX_LEN: usize = 8 * 1024;
    /// Most path segments accepted.
    pub const MAX_SEGMENTS: usize = 64;

    /// An http(s) URL from the configuration (the server's base URL).
    ///
    /// # Errors
    ///
    /// Not http(s), user info, a query, or a path that [`Href::resolve`]
    /// would refuse.
    pub fn from_url(url: &Url) -> Result<Self, HrefError> {
        if !is_http(url) || url.cannot_be_a_base() {
            return Err(HrefError::Url);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HrefError::Url);
        }
        if url.query().is_some() {
            return Err(HrefError::Character);
        }
        let (segments, collection) = parse_path(url.path())?;
        Self::build(url, segments, collection)
    }

    /// Resolves an href from a server (an `<href>` or a `Location`) against
    /// the URL it answered.
    ///
    /// ```
    /// use url::Url;
    /// use nebula_dav_proto::{Href, HrefError};
    ///
    /// let base = Href::from_url(&Url::parse("https://dav.example.com/cal/alice/").unwrap()).unwrap();
    /// let event = Href::resolve("work/caf%c3%a9%201.ics", &base).unwrap();
    /// assert_eq!(event.path(), "/cal/alice/work/caf%C3%A9%201.ics");
    /// assert_eq!(Href::resolve("/cal/alice/%2e%2e/bob/", &base), Err(HrefError::Segment));
    /// assert_eq!(Href::resolve("/cal/a%2Fb", &base), Err(HrefError::Segment));
    /// assert_eq!(Href::resolve("//evil.example/", &base), Err(HrefError::Url));
    /// ```
    ///
    /// # Errors
    ///
    /// [`HrefError`] for anything that is not a plain, normalised path or
    /// http(s) URL.
    pub fn resolve(text: &str, base: &Self) -> Result<Self, HrefError> {
        let text = text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
        if text.is_empty() || text.len() > Self::MAX_LEN {
            return Err(HrefError::Length);
        }
        if text
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '?' | '#'))
        {
            return Err(HrefError::Character);
        }
        if text.starts_with("//") {
            return Err(HrefError::Url);
        }
        if has_scheme(text) {
            let url = Url::parse(text).map_err(|_| HrefError::Url)?;
            if !is_http(&url) || !url.username().is_empty() || url.password().is_some() {
                return Err(HrefError::Url);
            }
            // The path as written, before the URL parser drops dot segments.
            let after_scheme = text
                .get(url.scheme().len() + 1..)
                .and_then(|rest| rest.strip_prefix("//"))
                .ok_or(HrefError::Url)?;
            let raw_path = after_scheme
                .find('/')
                .and_then(|slash| after_scheme.get(slash..))
                .unwrap_or("/");
            let (segments, collection) = parse_path(raw_path)?;
            return Self::build(&url, segments, collection);
        }
        if text.starts_with('/') {
            let (segments, collection) = parse_path(text)?;
            return Self::build(&base.url, segments, collection);
        }
        // A relative path: relative to the base's "directory".
        let mut segments = base.segments.clone();
        if !base.collection {
            segments.pop();
        }
        let (relative, collection) = parse_relative(text)?;
        segments.extend(relative);
        Self::build(&base.url, segments, collection)
    }

    fn build(origin: &Url, segments: Vec<String>, collection: bool) -> Result<Self, HrefError> {
        if segments.len() > Self::MAX_SEGMENTS {
            return Err(HrefError::TooDeep);
        }
        let mut path = String::with_capacity(64);
        for segment in &segments {
            path.push('/');
            encode_segment(segment, &mut path);
        }
        if collection || segments.is_empty() {
            path.push('/');
        }
        let mut url = origin.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.set_path(&path);
        // Our encoding uses only characters the URL parser keeps as they
        // are; if that ever changed, refuse rather than send another path.
        if url.path() != path {
            return Err(HrefError::Url);
        }
        // Re-encoding can make it longer than what came in (`<` → `%3C`):
        // the limit holds for what we keep and send, so writing an href
        // out and reading it back always gives the same one (found by the
        // `href` fuzzer).
        if url.as_str().len() > Self::MAX_LEN {
            return Err(HrefError::Length);
        }
        Ok(Self {
            url,
            segments,
            collection: collection || path == "/",
        })
    }

    /// The path, canonically percent-encoded (`/cal/alice/caf%C3%A9.ics`).
    /// This is what goes back to the server, in a request line or in XML.
    #[must_use]
    pub fn path(&self) -> &str {
        self.url.path()
    }

    /// The whole URL (origin and path).
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// The origin (scheme, host, port).
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.url.origin()
    }

    /// Same scheme, host and port as `other`. Compared part by part, with
    /// no allocation: for http(s), the only schemes an `Href` takes
    /// (`is_http`), this is the same as comparing [`Href::origin`].
    #[must_use]
    pub fn same_origin(&self, other: &Self) -> bool {
        debug_assert!(is_http(&self.url) && is_http(&other.url));
        self.url.scheme() == other.url.scheme()
            && self.url.host() == other.url.host()
            && self.url.port_or_known_default() == other.url.port_or_known_default()
    }

    /// A key that is equal for two hrefs exactly when
    /// [`Href::same_location`] holds: to find hrefs in a set or map.
    #[must_use]
    pub fn location_key(&self) -> LocationKey {
        LocationKey {
            scheme: self.url.scheme().to_owned(),
            host: self.url.host_str().unwrap_or_default().to_owned(),
            port: self.url.port_or_known_default(),
            segments: self.segments.clone(),
        }
    }

    /// The decoded path segments.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// The path ends with `/` (a collection, by convention).
    #[must_use]
    pub const fn is_collection(&self) -> bool {
        self.collection
    }

    /// The last segment, decoded (`None` for `/`).
    #[must_use]
    pub fn last_segment(&self) -> Option<&str> {
        self.segments.last().map(String::as_str)
    }

    /// The same location as `other`, ignoring a trailing slash (servers
    /// disagree on whether a collection's own href has one).
    #[must_use]
    pub fn same_location(&self, other: &Self) -> bool {
        self.same_origin(other) && self.segments == other.segments
    }

    /// A direct member of `collection` on the same origin. Nothing deeper,
    /// nothing elsewhere (a hostile server cannot make a resource of
    /// another collection or another account look like one of this one).
    #[must_use]
    pub fn is_member_of(&self, collection: &Self) -> bool {
        self.same_origin(collection)
            && self.segments.len() == collection.segments.len() + 1
            && self.segments.starts_with(&collection.segments)
    }

    /// Strictly below `ancestor` (at any depth) on the same origin.
    #[must_use]
    pub fn is_inside(&self, ancestor: &Self) -> bool {
        self.same_origin(ancestor)
            && self.segments.len() > ancestor.segments.len()
            && self.segments.starts_with(&ancestor.segments)
    }

    /// The member `name` of this collection, as a resource or a collection.
    ///
    /// # Errors
    ///
    /// [`HrefError::TooDeep`] past [`Href::MAX_SEGMENTS`].
    pub fn member(&self, name: &ResourceName, collection: bool) -> Result<Self, HrefError> {
        let mut segments = self.segments.clone();
        segments.push(name.0.clone());
        Self::build(&self.url, segments, collection)
    }

    /// The same path with a trailing slash.
    #[must_use]
    pub fn as_collection(&self) -> Self {
        if self.collection {
            return self.clone();
        }
        let mut url = self.url.clone();
        let path = format!("{}/", self.url.path());
        url.set_path(&path);
        Self {
            url,
            segments: self.segments.clone(),
            collection: true,
        }
    }
}

/// An absolute path (`/a/b/`).
fn parse_path(raw: &str) -> Result<(Vec<String>, bool), HrefError> {
    let rest = raw.strip_prefix('/').ok_or(HrefError::Url)?;
    if rest.is_empty() {
        return Ok((Vec::new(), true));
    }
    parse_relative(rest)
}

/// Segments of `a/b` or `a/b/`: no empty segment except a final slash.
fn parse_relative(raw: &str) -> Result<(Vec<String>, bool), HrefError> {
    let (body, collection) = match raw.strip_suffix('/') {
        Some(body) => (body, true),
        None => (raw, false),
    };
    let mut segments = Vec::new();
    for piece in body.split('/') {
        if segments.len() >= Href::MAX_SEGMENTS {
            return Err(HrefError::TooDeep);
        }
        segments.push(parse_segment(piece)?);
    }
    Ok((segments, collection))
}

fn parse_segment(raw: &str) -> Result<String, HrefError> {
    if raw.is_empty() || raw == "." || raw == ".." {
        return Err(HrefError::Segment);
    }
    let decoded = decode_segment(raw)?;
    if decoded == "." || decoded == ".." || decoded.contains(['/', '\\']) {
        return Err(HrefError::Segment);
    }
    if decoded.chars().any(char::is_control) {
        return Err(HrefError::Character);
    }
    Ok(decoded)
}

/// The name of a new member of a collection, for PUT, MKCALENDAR, MKCOL
/// and MOVE: one path segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    /// Longest name, in bytes.
    pub const MAX_LEN: usize = 255;

    /// The name as given (decoded).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ResourceName {
    type Error = HrefError;

    /// ```
    /// use nebula_dav_proto::{HrefError, ResourceName};
    ///
    /// assert!(ResourceName::try_from("0b5c3e1a.ics").is_ok());
    /// assert_eq!(ResourceName::try_from("../x"), Err(HrefError::Segment));
    /// assert_eq!(ResourceName::try_from(""), Err(HrefError::Length));
    /// ```
    fn try_from(name: &str) -> Result<Self, HrefError> {
        if name.is_empty() || name.len() > Self::MAX_LEN {
            return Err(HrefError::Length);
        }
        if name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(HrefError::Segment);
        }
        if name.chars().any(char::is_control) {
            return Err(HrefError::Character);
        }
        Ok(Self(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{Href, HrefError, ResourceName};

    fn base(url: &str) -> Href {
        Href::from_url(&Url::parse(url).unwrap()).unwrap()
    }

    fn home() -> Href {
        base("https://dav.example.com/dav/cal/alice/")
    }

    #[test]
    fn resolves_like_rfc_3986_without_dot_segments() {
        let home = home();
        assert_eq!(
            Href::resolve("/dav/cal/alice/work/", &home).unwrap().path(),
            "/dav/cal/alice/work/"
        );
        assert_eq!(
            Href::resolve("work/1.ics", &home).unwrap().url().as_str(),
            "https://dav.example.com/dav/cal/alice/work/1.ics"
        );
        let file = Href::resolve("/dav/cal/alice/work/1.ics", &home).unwrap();
        assert_eq!(
            Href::resolve("2.ics", &file).unwrap().path(),
            "/dav/cal/alice/work/2.ics"
        );
        assert_eq!(
            Href::resolve("https://dav.example.com:443/dav/", &home)
                .unwrap()
                .path(),
            "/dav/"
        );
        // Whitespace around the text (pretty-printed XML) is not part of it.
        assert_eq!(
            Href::resolve("\n   /dav/cal/alice/\n  ", &home)
                .unwrap()
                .path(),
            "/dav/cal/alice/"
        );
    }

    #[test]
    fn normalises_percent_encoding() {
        let home = home();
        let a = Href::resolve("/dav/cal/alice%40example.com/caf%c3%a9.ics", &home).unwrap();
        let b = Href::resolve("/dav/cal/alice@example.com/café.ics", &home).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.path(), "/dav/cal/alice@example.com/caf%C3%A9.ics");
        assert_eq!(a.last_segment(), Some("café.ics"));
        let spaced = Href::resolve("/dav/my calendar/", &home).unwrap();
        assert_eq!(spaced.path(), "/dav/my%20calendar/");
    }

    #[test]
    fn refuses_escapes_and_odd_syntax() {
        let home = home();
        let cases = [
            ("", HrefError::Length),
            ("   ", HrefError::Length),
            ("/dav/../etc/", HrefError::Segment),
            ("/dav/./x", HrefError::Segment),
            ("../bob/", HrefError::Segment),
            ("/dav/%2e%2e/bob/", HrefError::Segment),
            ("/dav/%2E./bob/", HrefError::Segment),
            ("/dav/.%2e/bob/", HrefError::Segment),
            ("/dav/a%2fb", HrefError::Segment),
            ("/dav/a%5cb", HrefError::Segment),
            ("/dav//x", HrefError::Segment),
            ("/dav/a%00b", HrefError::Character),
            ("/dav/a%0d%0aSet-Cookie:x", HrefError::Character),
            ("/dav/a\\b", HrefError::Character),
            ("/dav/a?b", HrefError::Character),
            ("/dav/a#b", HrefError::Character),
            ("/dav/a\u{0}b", HrefError::Character),
            ("/dav/%zz", HrefError::PercentEncoding),
            ("/dav/%ff", HrefError::PercentEncoding),
            ("//evil.example/x", HrefError::Url),
            ("file:///etc/passwd", HrefError::Url),
            ("javascript:alert(1)", HrefError::Url),
            ("https://user:pass@dav.example.com/", HrefError::Url),
            ("https://dav.example.com/a/../b", HrefError::Segment),
            ("https://dav.example.com/a/%2e%2e/b", HrefError::Segment),
        ];
        for (text, want) in cases {
            assert_eq!(Href::resolve(text, &home), Err(want), "{text:?}");
        }
        let long = format!("/{}", "a".repeat(Href::MAX_LEN));
        assert_eq!(Href::resolve(&long, &home), Err(HrefError::Length));
        // Short as written, too long once encoded.
        let grows = format!("/{}", "<".repeat(Href::MAX_LEN / 2));
        assert_eq!(Href::resolve(&grows, &home), Err(HrefError::Length));
        let deep = "/a".repeat(Href::MAX_SEGMENTS + 1);
        assert_eq!(Href::resolve(&deep, &home), Err(HrefError::TooDeep));
    }

    #[test]
    fn other_origins_resolve_but_are_not_the_same_origin() {
        let home = home();
        let other = Href::resolve("https://evil.example/dav/cal/alice/x.ics", &home).unwrap();
        assert!(!other.same_origin(&home));
        assert!(!other.is_member_of(&home));
        let downgrade = Href::resolve("http://dav.example.com/dav/cal/alice/x.ics", &home).unwrap();
        assert!(!downgrade.same_origin(&home));
        let port = Href::resolve("https://dav.example.com:8443/dav/cal/alice/x", &home).unwrap();
        assert!(!port.same_origin(&home));
        // Host names are case-insensitive (WHATWG lowercases them).
        let upper = Href::resolve("https://DAV.Example.COM/dav/cal/alice/x", &home).unwrap();
        assert!(upper.is_member_of(&home));
    }

    #[test]
    fn membership() {
        let home = home();
        let work = Href::resolve("work/", &home).unwrap();
        let event = Href::resolve("work/1.ics", &home).unwrap();
        let nested = Href::resolve("work/sub/1.ics", &home).unwrap();
        let bob = Href::resolve("/dav/cal/bob/work/1.ics", &home).unwrap();
        assert!(work.is_member_of(&home));
        assert!(event.is_member_of(&work));
        assert!(!nested.is_member_of(&work));
        assert!(nested.is_inside(&work));
        assert!(!bob.is_member_of(&work));
        assert!(!bob.is_inside(&home));
        assert!(!work.is_member_of(&work));
        // A collection without its trailing slash is still itself.
        let bare = Href::resolve("/dav/cal/alice/work", &home).unwrap();
        assert!(bare.same_location(&work));
        assert_ne!(bare, work);
        assert_eq!(bare.as_collection(), work);
        // A prefix of a segment is not a parent.
        let workshop = Href::resolve("/dav/cal/alice/workshop/1.ics", &home).unwrap();
        assert!(!workshop.is_inside(&work));
    }

    /// `location_key` agrees with `same_location` (trailing slash, host
    /// case, default port, other origins).
    #[test]
    fn location_keys_match_same_location() {
        let home = home();
        let hrefs = [
            Href::resolve("work/", &home).unwrap(),
            Href::resolve("/dav/cal/alice/work", &home).unwrap(),
            Href::resolve("https://DAV.example.com:443/dav/cal/alice/work/", &home).unwrap(),
            Href::resolve("https://dav.example.com:8443/dav/cal/alice/work/", &home).unwrap(),
            Href::resolve("http://dav.example.com/dav/cal/alice/work/", &home).unwrap(),
            Href::resolve("https://other.example.com/dav/cal/alice/work/", &home).unwrap(),
            Href::resolve("/dav/cal/alice/work/1.ics", &home).unwrap(),
            Href::resolve("/dav/cal/alice/caf%c3%a9/", &home).unwrap(),
            Href::resolve("/dav/cal/alice/café", &home).unwrap(),
            home,
        ];
        for a in &hrefs {
            for b in &hrefs {
                assert_eq!(
                    a.same_location(b),
                    a.location_key() == b.location_key(),
                    "{a:?} {b:?}"
                );
                assert_eq!(a.same_origin(b), a.origin() == b.origin(), "{a:?} {b:?}");
            }
        }
        assert!(hrefs[0].same_location(&hrefs[2]));
        assert!(hrefs[7].same_location(&hrefs[8]));
    }

    #[test]
    fn members_are_one_segment() {
        let work = Href::resolve("work/", &home()).unwrap();
        let name = ResourceName::try_from("evént 1.ics").unwrap();
        let event = work.member(&name, false).unwrap();
        assert_eq!(event.path(), "/dav/cal/alice/work/ev%C3%A9nt%201.ics");
        assert!(event.is_member_of(&work));
        for bad in ["..", ".", "a/b", "a\\b", "a\nb"] {
            assert!(ResourceName::try_from(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn configuration_urls() {
        assert_eq!(base("http://127.0.0.1:5232").path(), "/");
        assert!(base("http://127.0.0.1:5232").is_collection());
        assert_eq!(
            Href::from_url(&Url::parse("https://a@dav.example.com/").unwrap()),
            Err(HrefError::Url)
        );
        assert_eq!(
            Href::from_url(&Url::parse("https://dav.example.com/?x").unwrap()),
            Err(HrefError::Character)
        );
        assert_eq!(
            Href::from_url(&Url::parse("ftp://dav.example.com/").unwrap()),
            Err(HrefError::Url)
        );
    }
}
