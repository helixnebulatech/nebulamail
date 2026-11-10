// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors. `Display` never shows hrefs, URLs or other server data (they
//! may carry calendar names or e-mail addresses).

/// An href (or `Location`) that is not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HrefError {
    /// Empty, or longer than [`Href::MAX_LEN`](crate::Href::MAX_LEN).
    #[error("href is empty or too long")]
    Length,
    /// Control characters, a backslash, spaces, a query or a fragment.
    #[error("href has characters that are not allowed")]
    Character,
    /// A `%` not followed by two hex digits, or bytes that are not UTF-8.
    #[error("href has invalid percent-encoding")]
    PercentEncoding,
    /// A `.` or `..` segment (also percent-encoded), an encoded `/` or `\`,
    /// or an empty segment (`//`).
    #[error("href has a dot segment, an encoded slash or an empty segment")]
    Segment,
    /// Not `http` or `https`, user info in the URL, `//host` without a
    /// scheme, or a URL the WHATWG parser rejects.
    #[error("href is not an http(s) URL or path")]
    Url,
    /// Too many path segments.
    #[error("href has too many segments")]
    TooDeep,
}

/// A value that cannot be written into a request safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// A character that XML 1.0 cannot carry (control characters other
    /// than tab, CR and LF; U+FFFE, U+FFFF).
    #[error("text has characters XML cannot carry")]
    XmlCharacter,
    /// A value outside its allowed range (an empty or too long name…).
    #[error("value out of range")]
    Range,
}

/// A response body that is not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Bigger than the limit (checked before parsing).
    #[error("XML body too large")]
    TooLarge,
    /// Not UTF-8, or the XML declaration names another encoding.
    #[error("XML body is not UTF-8")]
    Encoding,
    /// A document type declaration: refused, whatever it contains (no
    /// XXE, no billion laughs).
    #[error("XML body has a DTD")]
    Dtd,
    /// More elements nested than the limit.
    #[error("XML body nested too deeply")]
    TooDeep,
    /// More nodes than the limit.
    #[error("XML body has too many nodes")]
    TooManyNodes,
    /// An element with more attributes than
    /// [`MAX_ATTRIBUTES_PER_ELEMENT`](crate::MAX_ATTRIBUTES_PER_ELEMENT).
    #[error("XML body has too many attributes")]
    TooManyAttributes,
    /// More `xmlns` declarations in scope at once than
    /// [`MAX_NAMESPACES_IN_SCOPE`](crate::MAX_NAMESPACES_IN_SCOPE).
    #[error("XML body declares too many namespaces")]
    TooManyNamespaces,
    /// Not well-formed XML (only where: the parser's message may quote
    /// the body).
    #[error("malformed XML at {row}:{col}")]
    Xml {
        /// Line, from 1.
        row: u32,
        /// Column, from 1.
        col: u32,
    },
    /// Well-formed, but not what WebDAV says (root not `DAV:multistatus`,
    /// a response without href, a bad status line…).
    #[error("invalid WebDAV response: {0}")]
    Invalid(&'static str),
    /// A property value is longer than allowed.
    #[error("property value too large")]
    ValueTooLarge,
    /// An href in the body.
    #[error(transparent)]
    Href(#[from] HrefError),
}
