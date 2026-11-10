// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Request-level errors (RFC 8620 §3.6.1) as RFC 7807 problem details.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::macros::string_enum;

/// `Content-Type` of a problem details document (RFC 7807 §3).
pub const PROBLEM_JSON_CONTENT_TYPE: &str = "application/problem+json";

string_enum! {
    /// Which limit of the core capability was hit (`"limit"` property of a
    /// `urn:ietf:params:jmap:error:limit` problem).
    pub enum LimitName ("limit name") {
        /// `maxSizeRequest`.
        MaxSizeRequest = "maxSizeRequest",
        /// `maxCallsInRequest`.
        MaxCallsInRequest = "maxCallsInRequest",
        /// `maxConcurrentRequests`.
        MaxConcurrentRequests = "maxConcurrentRequests",
        /// `maxSizeUpload` (blob upload).
        MaxSizeUpload = "maxSizeUpload",
        /// `maxConcurrentUpload` (blob upload).
        MaxConcurrentUpload = "maxConcurrentUpload",
    }
}

/// The problem type of [`RequestErrorType::OverQuota`], under the
/// bridge's capability URI like its other extensions
/// (`https://helixnebula.tech/jmap/bridge/…`).
pub const OVER_QUOTA_URI: &str = "https://helixnebula.tech/jmap/bridge/error/overQuota";

/// The problem type of a request-level error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestErrorType {
    /// `urn:ietf:params:jmap:error:unknownCapability`: `using` names a
    /// capability the server doesn't support.
    UnknownCapability,
    /// `urn:ietf:params:jmap:error:notJSON`: not `application/json` or
    /// not valid I-JSON.
    NotJson,
    /// `urn:ietf:params:jmap:error:notRequest`: JSON, but not a Request
    /// object.
    NotRequest,
    /// `urn:ietf:params:jmap:error:limit`: a server limit was exceeded.
    Limit(LimitName),
    /// [`OVER_QUOTA_URI`]: an upload doesn't fit in the account's storage
    /// for blobs, even after deleting its oldest uploads (RFC 8620 §6).
    /// HTTP 413. RFC 8620 defines no request-level type for this, and the
    /// `urn:ietf:params:jmap:error:` namespace only holds types in the IANA
    /// "JMAP Error Codes" registry (RFC 8620 §9.4.4: `overQuota` there is a
    /// method/set-level error, not a request-level one), so the type is a
    /// URI of the bridge's own (RFC 7807 §3.1 lets any URI be a type).
    OverQuota,
}

impl RequestErrorType {
    /// The problem type URI.
    #[must_use]
    pub const fn uri(self) -> &'static str {
        match self {
            Self::UnknownCapability => "urn:ietf:params:jmap:error:unknownCapability",
            Self::NotJson => "urn:ietf:params:jmap:error:notJSON",
            Self::NotRequest => "urn:ietf:params:jmap:error:notRequest",
            Self::Limit(_) => "urn:ietf:params:jmap:error:limit",
            Self::OverQuota => OVER_QUOTA_URI,
        }
    }
}

/// A request-level error: the whole request is rejected with an HTTP error
/// and this problem details body (RFC 7807).
///
/// ```json
/// {"type": "urn:ietf:params:jmap:error:limit", "status": 400,
///  "limit": "maxCallsInRequest", "detail": "..."}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct RequestError {
    kind: RequestErrorType,
    detail: String,
}

impl RequestError {
    /// A request error with a human-readable `detail` (for developers).
    #[must_use]
    pub fn new(kind: RequestErrorType, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// The problem type.
    #[must_use]
    pub fn kind(&self) -> RequestErrorType {
        self.kind
    }

    /// The detail message.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// The HTTP status to answer with. RFC 8620 uses 400 for all of these;
    /// too many concurrent requests is 429 (Too Many Requests).
    #[must_use]
    pub fn status(&self) -> u16 {
        match self.kind {
            RequestErrorType::Limit(
                LimitName::MaxConcurrentRequests | LimitName::MaxConcurrentUpload,
            ) => 429,
            RequestErrorType::Limit(LimitName::MaxSizeUpload) | RequestErrorType::OverQuota => 413,
            RequestErrorType::UnknownCapability
            | RequestErrorType::NotJson
            | RequestErrorType::NotRequest
            | RequestErrorType::Limit(LimitName::MaxSizeRequest | LimitName::MaxCallsInRequest) => {
                400
            }
        }
    }
}

impl Serialize for RequestError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", self.kind.uri())?;
        map.serialize_entry("status", &self.status())?;
        if let RequestErrorType::Limit(limit) = self.kind {
            map.serialize_entry("limit", &limit)?;
        }
        map.serialize_entry("detail", &self.detail)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rfc_8620_examples() {
        let error = RequestError::new(
            RequestErrorType::UnknownCapability,
            "The Request object used capability 'https://example.com/apis/foobar', which is not supported by this server.",
        );
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            json!({
                "type": "urn:ietf:params:jmap:error:unknownCapability",
                "status": 400,
                "detail": "The Request object used capability 'https://example.com/apis/foobar', which is not supported by this server."
            })
        );
        let limit = RequestError::new(
            RequestErrorType::Limit(LimitName::MaxSizeRequest),
            "The request is larger than the server is willing to process.",
        );
        assert_eq!(
            serde_json::to_value(&limit).unwrap(),
            json!({
                "type": "urn:ietf:params:jmap:error:limit",
                "limit": "maxSizeRequest",
                "status": 400,
                "detail": "The request is larger than the server is willing to process."
            })
        );
    }

    #[test]
    fn other_types() {
        for (kind, uri) in [
            (
                RequestErrorType::NotJson,
                "urn:ietf:params:jmap:error:notJSON",
            ),
            (
                RequestErrorType::NotRequest,
                "urn:ietf:params:jmap:error:notRequest",
            ),
        ] {
            let value = serde_json::to_value(RequestError::new(kind, "x")).unwrap();
            assert_eq!(value, json!({"type": uri, "status": 400, "detail": "x"}));
        }
        let value =
            serde_json::to_value(RequestError::new(RequestErrorType::OverQuota, "x")).unwrap();
        assert_eq!(
            value,
            json!({"type": "https://helixnebula.tech/jmap/bridge/error/overQuota", "status": 413, "detail": "x"})
        );
    }
}
