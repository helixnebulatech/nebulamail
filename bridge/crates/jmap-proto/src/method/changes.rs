// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/changes` (RFC 8620 §5.2).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::from_arguments;
use crate::error::MethodError;
use crate::object::MailboxProperty;
use crate::types::{AccountId, MailboxId, PositiveInt, State};

/// Arguments of a standard `Foo/changes`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangesArguments {
    /// The account.
    pub account_id: AccountId,
    /// The state the client has.
    pub since_state: State,
    /// Most ids to return (must be > 0); `None` means the server decides.
    #[serde(default)]
    pub max_changes: Option<PositiveInt>,
}

impl ChangesArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` (also for `maxChanges: 0`).
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        from_arguments(arguments)
    }
}

/// Response of a standard `Foo/changes`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResponse<I> {
    /// The account.
    pub account_id: AccountId,
    /// The `sinceState` of the request.
    pub old_state: State,
    /// The state the client has after applying these changes.
    pub new_state: State,
    /// Whether there are more changes after `newState`.
    pub has_more_changes: bool,
    /// Ids created since the old state.
    pub created: Vec<I>,
    /// Ids updated since the old state.
    pub updated: Vec<I>,
    /// Ids destroyed since the old state.
    pub destroyed: Vec<I>,
}

/// `Mailbox/changes` response: adds `updatedProperties` (RFC 8621 §2.2).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxChangesResponse {
    /// The standard part.
    #[serde(flatten)]
    pub changes: ChangesResponse<MailboxId>,
    /// If only counts changed, the count properties; `None` (`null`) if
    /// other properties may have changed too.
    pub updated_properties: Option<Vec<MailboxProperty>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_and_rejects_zero() {
        let args = ChangesArguments::parse(
            json!({"accountId": "a", "sinceState": "s1", "maxChanges": 50})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert_eq!(args.max_changes.unwrap().get().get(), 50);
        assert!(
            ChangesArguments::parse(
                json!({"accountId": "a", "sinceState": "s1", "maxChanges": 0})
                    .as_object()
                    .unwrap()
                    .clone()
            )
            .is_err()
        );
    }

    #[test]
    fn mailbox_response_shape() {
        let response = MailboxChangesResponse {
            changes: ChangesResponse {
                account_id: "a".parse().unwrap(),
                old_state: State::new("1"),
                new_state: State::new("2"),
                has_more_changes: false,
                created: vec![],
                updated: vec!["m1".parse().unwrap()],
                destroyed: vec![],
            },
            updated_properties: Some(vec![MailboxProperty::UnreadEmails]),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "a", "oldState": "1", "newState": "2", "hasMoreChanges": false,
                "created": [], "updated": ["m1"], "destroyed": [],
                "updatedProperties": ["unreadEmails"]
            })
        );
    }
}
