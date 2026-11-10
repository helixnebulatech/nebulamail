// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Request object (RFC 8620 §3.3) and its parsing, with the limits of
//! the core capability.
//!
//! Processing a request on the server:
//!
//! 1. [`Request::parse`] the body (request-level errors: not JSON, not a
//!    request, unknown capability, too big, too many calls).
//! 2. For each [`Invocation`], in order:
//!    - if [`Request::is_method_enabled`] is false → `unknownMethod`;
//!    - [`resolve_result_references`](crate::resolve_result_references)
//!      on its arguments, with the responses so far;
//!    - [`MethodCall::parse`] the arguments into typed ones
//!      (`invalidArguments`, `requestTooLarge`… on failure);
//!    - run it and append the response.
//!
//! **Decision:** unknown top-level members of the Request object are
//! ignored (the RFC only lists required ones); unknown capabilities in
//! `using` are rejected, as RFC 8620 §3.6.1 requires.

mod capability;
mod method_call;
mod method_name;
mod strict;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use capability::{Capability, CapabilityError, OtherCapability};
pub use method_call::{MethodCall, MethodLimits};
pub use method_name::{MethodName, UnknownMethodName};
pub use strict::{StrictValue, from_slice_strict};

use crate::error::{LimitName, RequestError, RequestErrorType};
use crate::references::CreatedIds;
use crate::types::CallId;

/// A JMAP request (RFC 8620 §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// Capabilities the client uses.
    pub using: Vec<Capability>,
    /// Method calls, processed in order.
    pub method_calls: Vec<Invocation>,
    /// Creation ids from earlier requests (for proxies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<CreatedIds>,
}

/// One method call: `[name, arguments, callId]` (RFC 8620 §3.2).
///
/// Arguments stay as JSON until result references are resolved; then
/// [`MethodCall::parse`] turns them into typed arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    from = "(MethodName, Map<String, Value>, CallId)",
    into = "(MethodName, Map<String, Value>, CallId)"
)]
pub struct Invocation {
    /// The method name.
    pub name: MethodName,
    /// The arguments object.
    pub arguments: Map<String, Value>,
    /// The client's id for this call.
    pub call_id: CallId,
}

impl From<(MethodName, Map<String, Value>, CallId)> for Invocation {
    fn from((name, arguments, call_id): (MethodName, Map<String, Value>, CallId)) -> Self {
        Self {
            name,
            arguments,
            call_id,
        }
    }
}

impl From<Invocation> for (MethodName, Map<String, Value>, CallId) {
    fn from(invocation: Invocation) -> Self {
        (invocation.name, invocation.arguments, invocation.call_id)
    }
}

/// Request-level limits from the core capability (RFC 8620 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimits {
    /// `maxSizeRequest`, in bytes.
    pub max_size_request: usize,
    /// `maxCallsInRequest`.
    pub max_calls_in_request: usize,
}

impl Request {
    /// Parses a request body.
    ///
    /// `supported` lists the capabilities the server offers to this user;
    /// any other capability in `using` is an `unknownCapability` error.
    ///
    /// # Errors
    /// A [`RequestError`] to send back as problem details: `limit`
    /// (`maxSizeRequest`, `maxCallsInRequest`), `notJSON`, `notRequest`
    /// or `unknownCapability`.
    pub fn parse(
        body: &[u8],
        limits: &RequestLimits,
        supported: &[Capability],
    ) -> Result<Self, RequestError> {
        if body.len() > limits.max_size_request {
            return Err(RequestError::new(
                RequestErrorType::Limit(LimitName::MaxSizeRequest),
                format!(
                    "The request is larger than {} bytes.",
                    limits.max_size_request
                ),
            ));
        }
        // Duplicate member names are not I-JSON (RFC 7493 §2.3): `notJSON`.
        let value: Value = from_slice_strict(body).map_err(|error| {
            RequestError::new(
                RequestErrorType::NotJson,
                format!("The request is not valid JSON: {error}"),
            )
        })?;
        let request: Self = serde_json::from_value(value).map_err(|error| {
            RequestError::new(
                RequestErrorType::NotRequest,
                format!("The request is not a JMAP Request object: {error}"),
            )
        })?;
        if request.method_calls.len() > limits.max_calls_in_request {
            return Err(RequestError::new(
                RequestErrorType::Limit(LimitName::MaxCallsInRequest),
                format!(
                    "The request has more than {} method calls.",
                    limits.max_calls_in_request
                ),
            ));
        }
        if let Some(unknown) = request.using.iter().find(|c| !supported.contains(c)) {
            return Err(RequestError::new(
                RequestErrorType::UnknownCapability,
                format!(
                    "The Request object used capability '{unknown}', which is not supported by this server."
                ),
            ));
        }
        Ok(request)
    }

    /// Whether the method's capability is in `using`. If not, the call must
    /// be answered with `unknownMethod` (RFC 8620 §3.6.2).
    #[must_use]
    pub fn is_method_enabled(&self, name: &MethodName) -> bool {
        name.capability()
            .is_some_and(|capability| self.using.contains(&capability))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::RequestErrorType;

    const LIMITS: RequestLimits = RequestLimits {
        max_size_request: 10_000_000,
        max_calls_in_request: 16,
    };
    const SUPPORTED: &[Capability] = &[Capability::Core, Capability::Mail, Capability::Submission];

    fn parse(value: &Value) -> Result<Request, RequestError> {
        Request::parse(value.to_string().as_bytes(), &LIMITS, SUPPORTED)
    }

    /// RFC 8620 §3.3.1.
    #[test]
    fn rfc_example_round_trips() {
        let example = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                ["method1", {"arg1": "arg1data", "arg2": "arg2data"}, "c1"],
                ["method2", {"arg1": "arg1data"}, "c2"],
                ["method3", {}, "c3"]
            ]
        });
        let request = parse(&example).unwrap();
        assert_eq!(request.method_calls.len(), 3);
        assert_eq!(request.method_calls[1].call_id.as_str(), "c2");
        assert!(matches!(
            request.method_calls[0].name,
            MethodName::Unknown(_)
        ));
        assert!(!request.is_method_enabled(&request.method_calls[0].name));
        assert_eq!(serde_json::to_value(&request).unwrap(), example);
    }

    #[test]
    fn created_ids_and_unknown_members() {
        let request = parse(&json!({
            "using": ["urn:ietf:params:jmap:mail"],
            "methodCalls": [["Email/get", {"accountId": "a"}, "0"]],
            "createdIds": {"k1": "m1"},
            "someVendorField": true
        }))
        .unwrap();
        assert!(request.is_method_enabled(&MethodName::EmailGet));
        assert!(!request.is_method_enabled(&MethodName::CoreEcho));
        assert!(!request.is_method_enabled(&MethodName::IdentityGet));
        assert_eq!(
            request
                .created_ids
                .unwrap()
                .get(&"k1".parse().unwrap())
                .unwrap()
                .as_str(),
            "m1"
        );
    }

    #[test]
    fn request_errors() {
        let kind = |body: &[u8]| Request::parse(body, &LIMITS, SUPPORTED).unwrap_err().kind();
        assert_eq!(kind(b"{"), RequestErrorType::NotJson);
        assert_eq!(kind(b"\xff"), RequestErrorType::NotJson);
        assert_eq!(kind(b"[]"), RequestErrorType::NotRequest);
        assert_eq!(
            kind(br#"{"using": [], "methodCalls": [["Core/echo", {"a": 1, "a": 2}, "c"]]}"#),
            RequestErrorType::NotJson
        );
        assert_eq!(kind(br#"{"using": []}"#), RequestErrorType::NotRequest);
        assert_eq!(
            kind(br#"{"using": [], "methodCalls": [["a", {}]]}"#),
            RequestErrorType::NotRequest
        );
        assert_eq!(
            kind(br#"{"using": [], "methodCalls": [["a", [], "c"]]}"#),
            RequestErrorType::NotRequest
        );
        assert_eq!(
            kind(br#"{"using": [], "methodCalls": [["a", {}, "c", 1]]}"#),
            RequestErrorType::NotRequest
        );
        assert_eq!(
            kind(br#"{"using": ["https://example.com/apis/foobar"], "methodCalls": []}"#),
            RequestErrorType::UnknownCapability
        );
        // Known to this crate but not offered by this server.
        assert_eq!(
            kind(br#"{"using": ["urn:ietf:params:jmap:calendars"], "methodCalls": []}"#),
            RequestErrorType::UnknownCapability
        );
    }

    #[test]
    fn limits() {
        let small = RequestLimits {
            max_size_request: 10,
            max_calls_in_request: 1,
        };
        let error = Request::parse(&[b' '; 11], &small, SUPPORTED).unwrap_err();
        assert_eq!(
            error.kind(),
            RequestErrorType::Limit(LimitName::MaxSizeRequest)
        );
        let two_calls = br#"{"using":[],"methodCalls":[["a",{},"1"],["b",{},"2"]]}"#;
        let roomy = RequestLimits {
            max_size_request: 1000,
            max_calls_in_request: 1,
        };
        let error = Request::parse(two_calls, &roomy, SUPPORTED).unwrap_err();
        assert_eq!(
            error.kind(),
            RequestErrorType::Limit(LimitName::MaxCallsInRequest)
        );
        assert_eq!(
            serde_json::to_value(&error).unwrap()["limit"],
            "maxCallsInRequest"
        );
    }
}
