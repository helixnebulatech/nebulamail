// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Method-level errors (RFC 8620 §3.6.2): `["error", {...}, callId]`.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::macros::string_enum;

string_enum! {
    /// The `type` of a method-level error (RFC 8620 §3.6.2, §5, RFC 8621).
    pub enum MethodErrorType ("method error type") {
        /// Some internal server resource was temporarily unavailable.
        ServerUnavailable = "serverUnavailable",
        /// An unexpected error; the method had no effect.
        ServerFail = "serverFail",
        /// Some, but not all, changes were made.
        ServerPartialFail = "serverPartialFail",
        /// The method name is not known (or its capability is not in
        /// `using`).
        UnknownMethod = "unknownMethod",
        /// An argument has the wrong type or value, or a required one is
        /// missing.
        InvalidArguments = "invalidArguments",
        /// A result reference (`#ids`) could not be resolved.
        InvalidResultReference = "invalidResultReference",
        /// Not allowed by the ACL or another policy.
        Forbidden = "forbidden",
        /// The `accountId` does not exist.
        AccountNotFound = "accountNotFound",
        /// The account doesn't support this method's data type.
        AccountNotSupportedByMethod = "accountNotSupportedByMethod",
        /// The account is read-only.
        AccountReadOnly = "accountReadOnly",
        /// Too many ids or objects in one call (`maxObjectsInGet`/`Set`).
        RequestTooLarge = "requestTooLarge",
        /// `ifInState` does not match the current state.
        StateMismatch = "stateMismatch",
        /// `/changes` or `/queryChanges` can't compute changes from that
        /// state.
        CannotCalculateChanges = "cannotCalculateChanges",
        /// The `anchor` id is not in the query results.
        AnchorNotFound = "anchorNotFound",
        /// A sort property or collation is not supported.
        UnsupportedSort = "unsupportedSort",
        /// A filter condition is not supported.
        UnsupportedFilter = "unsupportedFilter",
        /// More changes than `maxChanges` in `/queryChanges`.
        TooManyChanges = "tooManyChanges",
        /// `/copy`: the source account does not exist.
        FromAccountNotFound = "fromAccountNotFound",
        /// `/copy`: the source account doesn't support the data type.
        FromAccountNotSupportedByMethod = "fromAccountNotSupportedByMethod",
        /// `CalendarEvent/query` with `expandRecurrences`: the range is
        /// longer than `maxExpandedQueryDuration`.
        ExpandDurationTooLarge = "expandDurationTooLarge",
        /// `CalendarEvent/query` with `expandRecurrences`: the server
        /// can't expand some event (e.g. too many occurrences).
        CannotCalculateOccurrences = "cannotCalculateOccurrences",
        /// `Principal/getAvailability`: no principal with this id (or the
        /// user may not know it exists).
        NotFound = "notFound",
        /// `Principal/getAvailability`: `utcStart`–`utcEnd` is longer than
        /// `maxAvailabilityDuration`.
        TooLarge = "tooLarge",
    }
}

/// A method-level error: replaces the method's response.
///
/// Serializes to `{"type": "...", "description": "..."}` (the description
/// is left out when there is none).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct MethodError {
    kind: MethodErrorType,
    description: Option<String>,
}

impl MethodError {
    /// An error of this type without a description.
    #[must_use]
    pub fn new(kind: MethodErrorType) -> Self {
        Self {
            kind,
            description: None,
        }
    }

    /// Adds a description for developers (not shown to users).
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The error type.
    #[must_use]
    pub fn kind(&self) -> MethodErrorType {
        self.kind
    }

    /// The description, if any.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// `invalidArguments` with a description.
    #[must_use]
    pub fn invalid_arguments(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::InvalidArguments).with_description(description)
    }

    /// `invalidResultReference` with a description.
    #[must_use]
    pub fn invalid_result_reference(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::InvalidResultReference).with_description(description)
    }

    /// `unknownMethod`.
    #[must_use]
    pub fn unknown_method() -> Self {
        Self::new(MethodErrorType::UnknownMethod)
    }

    /// `requestTooLarge` with a description.
    #[must_use]
    pub fn request_too_large(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::RequestTooLarge).with_description(description)
    }

    /// `unsupportedFilter` with a description.
    #[must_use]
    pub fn unsupported_filter(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::UnsupportedFilter).with_description(description)
    }

    /// `unsupportedSort` with a description.
    #[must_use]
    pub fn unsupported_sort(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::UnsupportedSort).with_description(description)
    }

    /// `serverFail` with a description.
    #[must_use]
    pub fn server_fail(description: impl Into<String>) -> Self {
        Self::new(MethodErrorType::ServerFail).with_description(description)
    }
}

impl From<MethodErrorType> for MethodError {
    fn from(kind: MethodErrorType) -> Self {
        Self::new(kind)
    }
}

impl Serialize for MethodError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &self.kind)?;
        if let Some(description) = &self.description {
            map.serialize_entry("description", description)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_shape() {
        assert_eq!(
            serde_json::to_value(MethodError::unknown_method()).unwrap(),
            json!({"type": "unknownMethod"})
        );
        assert_eq!(
            serde_json::to_value(MethodError::invalid_arguments("missing accountId")).unwrap(),
            json!({"type": "invalidArguments", "description": "missing accountId"})
        );
        for kind in MethodErrorType::ALL {
            let value = serde_json::to_value(MethodError::new(*kind)).unwrap();
            assert_eq!(value["type"], kind.as_str());
        }
    }
}
