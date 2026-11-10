// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `DAV:status` lines.

use std::fmt;

/// An HTTP status code from a `DAV:status` element.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(u16);

impl Status {
    /// 200 OK.
    pub const OK: Self = Self(200);
    /// 403 Forbidden.
    pub const FORBIDDEN: Self = Self(403);
    /// 404 Not Found.
    pub const NOT_FOUND: Self = Self(404);
    /// 410 Gone.
    pub const GONE: Self = Self(410);
    /// 507 Insufficient Storage (a truncated sync-collection, RFC 6578 §3.6).
    pub const INSUFFICIENT_STORAGE: Self = Self(507);

    /// A code from 100 to 599.
    #[must_use]
    pub const fn new(code: u16) -> Option<Self> {
        if code >= 100 && code <= 599 {
            Some(Self(code))
        } else {
            None
        }
    }

    /// Parses a status line, `HTTP/1.1 200 OK` (RFC 4918 §14.28): the
    /// `HTTP/` version, one space, three digits, then an optional reason.
    ///
    /// ```
    /// use nebula_dav_proto::Status;
    ///
    /// assert_eq!(Status::parse_line("HTTP/1.1 404 Not Found"), Status::new(404));
    /// assert_eq!(Status::parse_line("  HTTP/1.1 200\n"), Status::new(200));
    /// assert_eq!(Status::parse_line("200 OK"), None);
    /// assert_eq!(Status::parse_line("HTTP/1.1 2000 OK"), None);
    /// ```
    #[must_use]
    pub fn parse_line(line: &str) -> Option<Self> {
        let line = line.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
        let rest = line.strip_prefix("HTTP/")?;
        let (version, rest) = rest.split_once(' ')?;
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let code = rest.get(..3)?;
        if !code.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        match rest.as_bytes().get(3) {
            None | Some(b' ') => {}
            Some(_) => return None,
        }
        Self::new(code.parse().ok()?)
    }

    /// The code.
    #[must_use]
    pub const fn code(self) -> u16 {
        self.0
    }

    /// 2xx.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Status({})", self.0)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
