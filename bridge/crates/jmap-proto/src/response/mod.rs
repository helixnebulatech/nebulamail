// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Response object (RFC 8620 §3.4).
//!
//! Each typed method response becomes a [`ResponseInvocation`] holding its
//! arguments as JSON: that is what result references (`#ids`) of later
//! calls point into, and what is finally sent.

mod method_response;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use method_response::MethodResponse;

use crate::error::MethodError;
use crate::macros::serde_via_str;
use crate::references::CreatedIds;
use crate::request::MethodName;
use crate::types::{CallId, State};

/// The name of a method response: the method's name, or `"error"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResponseName {
    /// A method's response (same name as the method).
    Method(MethodName),
    /// A method-level error.
    Error,
}

impl ResponseName {
    /// The name as sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Method(name) => name.as_str(),
            Self::Error => "error",
        }
    }
}

impl FromStr for ResponseName {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "error" {
            return Ok(Self::Error);
        }
        s.parse().map(Self::Method)
    }
}

impl fmt::Display for ResponseName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(ResponseName);

/// One entry of `methodResponses`: `[name, arguments, callId]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    from = "(ResponseName, Map<String, Value>, CallId)",
    into = "(ResponseName, Map<String, Value>, CallId)"
)]
pub struct ResponseInvocation {
    /// The method name, or `"error"`.
    pub name: ResponseName,
    /// The response arguments.
    pub arguments: Map<String, Value>,
    /// The call id of the method call this answers.
    pub call_id: CallId,
}

impl ResponseInvocation {
    /// A method-level error response for `call_id`.
    #[must_use]
    pub fn error(error: &MethodError, call_id: CallId) -> Self {
        let arguments = match serde_json::to_value(error) {
            Ok(Value::Object(map)) => map,
            // A MethodError always serializes to an object.
            Ok(_) | Err(_) => Map::new(),
        };
        Self {
            name: ResponseName::Error,
            arguments,
            call_id,
        }
    }
}

impl From<(ResponseName, Map<String, Value>, CallId)> for ResponseInvocation {
    fn from((name, arguments, call_id): (ResponseName, Map<String, Value>, CallId)) -> Self {
        Self {
            name,
            arguments,
            call_id,
        }
    }
}

impl From<ResponseInvocation> for (ResponseName, Map<String, Value>, CallId) {
    fn from(invocation: ResponseInvocation) -> Self {
        (invocation.name, invocation.arguments, invocation.call_id)
    }
}

/// A JMAP response (RFC 8620 §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Responses in the order the calls were processed. One call can have
    /// several (e.g. `EmailSubmission/set` followed by its implicit
    /// `Email/set`).
    pub method_responses: Vec<ResponseInvocation>,
    /// The final creation id map; only present if the request had
    /// `createdIds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<CreatedIds>,
    /// The current session state, so the client knows when to refetch the
    /// session.
    pub session_state: State,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 8620 §3.4.1.
    #[test]
    fn rfc_example_round_trips() {
        let example = json!({
            "methodResponses": [
                ["method1", {"arg1": 3, "arg2": "foo"}, "c1"],
                ["method2", {"isBlah": true}, "c2"],
                ["anotherResponseFromMethod2", {"data": 10, "yetmoredata": "Hello"}, "c2"],
                ["error", {"type": "unknownMethod"}, "c3"]
            ],
            "sessionState": "75128aab4b1b"
        });
        let response: Response = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(response.method_responses[3].name, ResponseName::Error);
        assert_eq!(serde_json::to_value(&response).unwrap(), example);
    }

    #[test]
    fn error_invocation() {
        let invocation =
            ResponseInvocation::error(&MethodError::unknown_method(), CallId::new("c3"));
        assert_eq!(
            serde_json::to_value(&invocation).unwrap(),
            json!(["error", {"type": "unknownMethod"}, "c3"])
        );
    }
}
