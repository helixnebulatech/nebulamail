// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors of the DAV client. `Display` never shows URLs, hrefs, names or
//! credentials (no hrefs or titles in the logs).

use nebula_dav_proto::{EncodeError, HrefError, ParseError, Precondition};
use nebula_tls::TlsError;
use net_guard::{BlockReason, GuardError};

/// Why a redirect was not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RedirectError {
    /// More than [`Limits::max_redirects`](crate::Limits::max_redirects).
    #[error("too many redirects")]
    TooMany,
    /// A 3xx without `Location`.
    #[error("redirect without Location")]
    MissingLocation,
    /// A `Location` that is not http(s) (`file:`, `gopher:`, `javascript:`…).
    #[error("redirect to another scheme")]
    Scheme,
    /// A `Location` that is not a clean URL or path.
    #[error("invalid redirect location: {0}")]
    InvalidLocation(HrefError),
    /// A POST answered with 301, 302 or 303 (only 307 and 308 keep the
    /// method and the body).
    #[error("redirect would change the request method")]
    Method,
}

/// Errors of the DAV client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DavError {
    /// `net-guard` refused the destination (port, private IP, DNS) or the
    /// connection failed.
    #[error(transparent)]
    Guard(#[from] GuardError),
    /// TLS failed (handshake, certificate, or checks off for a host that
    /// is not loopback).
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// `http://` to a server that is not on loopback (only Docker
    /// development servers may skip TLS).
    #[error("plain HTTP is only allowed to a loopback server")]
    PlainHttpNotLoopback,
    /// The HTTP exchange failed (connection closed, malformed response).
    #[error("HTTP error: {0}")]
    Http(#[from] hyper::Error),
    /// A response head (status line and headers) over 64 KiB.
    #[error("response head too large")]
    HeadTooLarge,
    /// A time limit was reached (connection, whole request, or no data).
    #[error("timed out")]
    Timeout,
    /// A body bigger than its limit (also once decompressed).
    #[error("response too large")]
    TooLarge,
    /// A `Content-Encoding` we do not decode (only gzip and deflate are).
    #[error("unsupported content encoding")]
    ContentEncoding,
    /// A compressed body that does not decompress.
    #[error("invalid compressed body")]
    Decompress,
    /// A response of the wrong type (HTML instead of XML, an event that is
    /// not `text/calendar`…).
    #[error("unexpected content type")]
    ContentType,
    /// A body that is not UTF-8 (iCalendar and vCard must be).
    #[error("body is not UTF-8")]
    NotUtf8,
    /// A redirect that was not followed.
    #[error("redirect refused: {0}")]
    Redirect(RedirectError),
    /// A location (redirect, principal, home, href) on an origin that is
    /// neither the server's nor one the admin allowed.
    #[error("location on an origin that is not allowed")]
    OriginNotAllowed,
    /// A resource that is not a direct member of its collection.
    #[error("href outside its collection")]
    OutsideCollection,
    /// 401: wrong credentials. Not retried.
    #[error("unauthorized (401)")]
    Unauthorized,
    /// 403.
    #[error("forbidden (403)")]
    Forbidden {
        /// The `DAV:error` condition, if the body had one.
        precondition: Option<Precondition>,
    },
    /// 404.
    #[error("not found (404)")]
    NotFound,
    /// 405 (e.g. MKCALENDAR where something already exists).
    #[error("method not allowed (405)")]
    MethodNotAllowed,
    /// 409.
    #[error("conflict (409)")]
    Conflict {
        /// The `DAV:error` condition, if the body had one.
        precondition: Option<Precondition>,
    },
    /// 412: `If-Match` or `If-None-Match` did not hold (someone else
    /// changed or created the resource).
    #[error("precondition failed (412)")]
    PreconditionFailed,
    /// 507 outside a sync report (quota).
    #[error("insufficient storage (507)")]
    InsufficientStorage,
    /// Another unexpected status.
    #[error("unexpected HTTP status {status}")]
    Status {
        /// The code.
        status: u16,
        /// The `DAV:error` condition, if the body had one.
        precondition: Option<Precondition>,
    },
    /// A multistatus (or error body) that does not parse.
    #[error(transparent)]
    Xml(#[from] ParseError),
    /// An href or URL that is not accepted.
    #[error(transparent)]
    Href(#[from] HrefError),
    /// A request value that cannot be written.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// A user name with `:` or control characters (Basic, RFC 7617).
    #[error("invalid user name for Basic authentication")]
    InvalidCredentials,
    /// The server broke the protocol (no sync token, no principal…).
    #[error("protocol error: {0}")]
    Protocol(&'static str),
    /// More collections or resources than the limits allow.
    #[error("too many collections or resources")]
    TooMany,
}

impl DavError {
    /// The shared reason of `security/ssrf-vectors.json`,
    /// for the errors that are one.
    #[must_use]
    pub fn block_reason(&self) -> Option<BlockReason> {
        match self {
            Self::Guard(error) => Some(error.reason()),
            Self::Redirect(RedirectError::TooMany) => Some(BlockReason::TooManyRedirects),
            Self::Redirect(RedirectError::Scheme) => Some(BlockReason::Scheme),
            Self::TooLarge => Some(BlockReason::Size),
            Self::Timeout => Some(BlockReason::Timeout),
            Self::ContentType => Some(BlockReason::ContentType),
            Self::PlainHttpNotLoopback | Self::Tls(_) | Self::Http(_) => Some(BlockReason::Network),
            _ => None,
        }
    }

    /// An answer too big to read whole: over its byte limit, or a
    /// multistatus over the node or attribute limits. Asking for less
    /// (a smaller multiget batch, no data) may work.
    #[must_use]
    pub const fn is_oversized(&self) -> bool {
        matches!(
            self,
            Self::TooLarge
                | Self::Xml(
                    ParseError::TooLarge | ParseError::TooManyNodes | ParseError::TooManyAttributes
                )
        )
    }

    /// Worth trying again later (network, timeouts, 5xx); not for 401,
    /// 403, 404, 412 or a refused destination.
    #[must_use]
    pub fn is_temporary(&self) -> bool {
        match self {
            Self::Guard(error) => {
                matches!(
                    error.reason(),
                    BlockReason::Network | BlockReason::Timeout | BlockReason::Dns
                )
            }
            Self::Http(_) | Self::Timeout | Self::InsufficientStorage => true,
            Self::Status { status, .. } => *status >= 500,
            _ => false,
        }
    }
}
