// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/set` (RFC 8620 §5.3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::PatchObject;
use super::parse::from_arguments;
use crate::error::{MethodError, SetError};
use crate::references::IdOrRef;
use crate::types::serde_util::{empty_map_as_null, empty_vec_as_null, null_as_default};
use crate::types::{AccountId, CreationId, IdKind, State};

/// Arguments of a standard `Foo/set`.
///
/// Objects to create and patches stay as JSON here: a bad object must fail
/// only that object (a `SetError` in `notCreated`/`notUpdated`), not the
/// whole call. Each object type parses them (`MailboxCreate::parse`,
/// `EmailPatch::parse`…).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "I: IdKind"))]
pub struct SetArguments<I> {
    /// The account.
    pub account_id: AccountId,
    /// If given, the call fails with `stateMismatch` unless this is the
    /// current state.
    #[serde(default)]
    pub if_in_state: Option<State>,
    /// Creation id → object to create.
    #[serde(default, deserialize_with = "null_as_default")]
    pub create: BTreeMap<CreationId, Map<String, Value>>,
    /// Id (or `#creationId`) → patch.
    #[serde(default, deserialize_with = "null_as_default")]
    pub update: BTreeMap<IdOrRef<I>, PatchObject>,
    /// Ids (or `#creationId`s) to destroy.
    #[serde(default, deserialize_with = "null_as_default")]
    pub destroy: Vec<IdOrRef<I>>,
}

impl<I: IdKind> SetArguments<I> {
    /// Parses the arguments and checks `maxObjectsInSet`.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` if create + update +
    /// destroy is more than `max_objects`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        parsed.check_limit(max_objects)?;
        Ok(parsed)
    }

    pub(crate) fn check_limit(&self, max_objects: usize) -> Result<(), MethodError> {
        let total = self
            .create
            .len()
            .saturating_add(self.update.len())
            .saturating_add(self.destroy.len());
        if total > max_objects {
            return Err(MethodError::request_too_large(format!(
                "more than {max_objects} objects to create, update or destroy"
            )));
        }
        Ok(())
    }
}

/// Response of a standard `Foo/set`. Empty maps and lists are written as
/// `null`, as RFC 8620 §5.3 says.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(bound(serialize = "T: Serialize, I: Serialize + Ord + std::fmt::Display"))]
pub struct SetResponse<T, I> {
    /// The account.
    pub account_id: AccountId,
    /// The state before the call, or `None` if unknown.
    pub old_state: Option<State>,
    /// The state after the call.
    pub new_state: State,
    /// Creation id → the server-set properties of the new object
    /// (always with `id`).
    #[serde(serialize_with = "empty_map_as_null")]
    pub created: BTreeMap<CreationId, T>,
    /// Id → properties the server changed beyond the patch, or `None`
    /// (`null`) if nothing else changed.
    #[serde(serialize_with = "empty_map_as_null")]
    pub updated: BTreeMap<I, Option<T>>,
    /// Ids destroyed.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub destroyed: Vec<I>,
    /// Creation id → why it failed.
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_created: BTreeMap<CreationId, SetError>,
    /// Id → why it failed. Keys are ids as sent (`#k1` for a creation
    /// reference that could not be resolved).
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_updated: BTreeMap<IdOrRef<I>, SetError>,
    /// Id → why it failed (keys as sent, like `not_updated`).
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_destroyed: BTreeMap<IdOrRef<I>, SetError>,
}

impl<T, I: Ord> SetResponse<T, I> {
    /// A response with nothing done yet.
    #[must_use]
    pub fn new(account_id: AccountId, old_state: Option<State>, new_state: State) -> Self {
        Self {
            account_id,
            old_state,
            new_state,
            created: BTreeMap::new(),
            updated: BTreeMap::new(),
            destroyed: Vec::new(),
            not_created: BTreeMap::new(),
            not_updated: BTreeMap::new(),
            not_destroyed: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::types::{EmailId, MailboxId};

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn parses_create_update_destroy() {
        let args = SetArguments::<EmailId>::parse(
            map(json!({
                "accountId": "a",
                "ifInState": "s1",
                "create": {"k1": {"mailboxIds": {"#mb": true}}},
                "update": {"e1": {"keywords/$seen": true}, "#k0": {"keywords": {}}},
                "destroy": ["e2"]
            })),
            10,
        )
        .unwrap();
        assert_eq!(args.create.len(), 1);
        assert_eq!(args.update.len(), 2);
        assert_eq!(args.destroy, [IdOrRef::Id("e2".parse().unwrap())]);
        let nulls = SetArguments::<EmailId>::parse(
            map(json!({"accountId": "a", "create": null, "update": null, "destroy": null})),
            10,
        )
        .unwrap();
        assert!(nulls.create.is_empty() && nulls.update.is_empty() && nulls.destroy.is_empty());
    }

    #[test]
    fn errors() {
        let kind = |value: Value, max| {
            SetArguments::<MailboxId>::parse(map(value), max)
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "a", "create": {"k1": 1}}), 10),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "update": {"m1": []}}), 10),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(
                json!({"accountId": "a", "destroy": ["a", "b"], "create": {"k": {}}}),
                2
            ),
            MethodErrorType::RequestTooLarge
        );
    }

    #[test]
    fn response_writes_empty_as_null() {
        let mut response: SetResponse<Value, EmailId> =
            SetResponse::new("a".parse().unwrap(), Some(State::new("1")), State::new("2"));
        response.destroyed.push("e1".parse().unwrap());
        response
            .not_updated
            .insert("e2".parse().unwrap(), SetError::not_found());
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "a", "oldState": "1", "newState": "2",
                "created": null, "updated": null, "destroyed": ["e1"],
                "notCreated": null, "notUpdated": {"e2": {"type": "notFound"}},
                "notDestroyed": null
            })
        );
    }
}
