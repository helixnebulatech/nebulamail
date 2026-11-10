// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/get` (RFC 8620 §5.1).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::from_arguments;
use crate::error::MethodError;
use crate::references::{CreatedIds, IdOrRef};
use crate::types::{AccountId, IdKind, State};

/// Arguments of a standard `Foo/get`.
///
/// `I` is the object's id type and `P` its property enum (unknown
/// properties are `invalidArguments`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "I: IdKind, P: Deserialize<'de>"))]
pub struct GetArguments<I, P> {
    /// The account.
    pub account_id: AccountId,
    /// Ids to fetch; `None` (`null`) means all objects.
    #[serde(default)]
    pub ids: Option<Vec<IdOrRef<I>>>,
    /// Properties to return; `None` means all (or the type's default
    /// list). `id` is always returned.
    #[serde(default)]
    pub properties: Option<Vec<P>>,
}

impl<I, P> GetArguments<I, P>
where
    I: IdKind,
    P: for<'de> Deserialize<'de>,
{
    /// Parses the arguments and checks `maxObjectsInGet`.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` if more ids than
    /// `max_objects` are asked for.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        parsed.check_limit(max_objects)?;
        Ok(parsed)
    }

    pub(crate) fn check_limit(&self, max_objects: usize) -> Result<(), MethodError> {
        match &self.ids {
            Some(ids) if ids.len() > max_objects => Err(MethodError::request_too_large(format!(
                "more than {max_objects} ids"
            ))),
            Some(_) | None => Ok(()),
        }
    }
}

impl<I: IdKind, P> GetArguments<I, P> {
    /// Turns each `#creationId` that `created` knows into its id and drops
    /// repeated ids, keeping the first: "If an identical id is included
    /// more than once in the request, the server MUST only include it once
    /// in either the list or the notFound argument of the response" (RFC
    /// 8620 §5.1). A `#creationId` and the id it stands for are the same
    /// id. Unknown creation ids stay as they are.
    pub fn resolve_ids(&mut self, created: &CreatedIds) {
        if let Some(ids) = &mut self.ids {
            dedup_ids(ids, created);
        }
    }
}

/// [`GetArguments::resolve_ids`] on a list of ids.
fn dedup_ids<I: IdKind>(ids: &mut Vec<IdOrRef<I>>, created: &CreatedIds) {
    let mut seen = BTreeSet::new();
    ids.retain_mut(|id| {
        if let Ok(resolved) = id.resolve(created) {
            *id = IdOrRef::Id(resolved);
        }
        seen.insert(id.clone())
    });
}

/// Response of a standard `Foo/get`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResponse<T, I> {
    /// The account.
    pub account_id: AccountId,
    /// The current state of this type.
    pub state: State,
    /// The objects found, with only the requested properties.
    pub list: Vec<T>,
    /// Requested ids that don't exist.
    pub not_found: Vec<I>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::object::{MailboxProperty, ThreadProperty};
    use crate::types::{MailboxId, ThreadId};

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn parses_ids_and_properties() {
        let args = GetArguments::<MailboxId, MailboxProperty>::parse(
            map(json!({"accountId": "a", "ids": ["m1", "#new"], "properties": ["name", "role"]})),
            10,
        )
        .unwrap();
        assert_eq!(args.ids.unwrap().len(), 2);
        assert_eq!(
            args.properties.unwrap(),
            [MailboxProperty::Name, MailboxProperty::Role]
        );
        let all = GetArguments::<ThreadId, ThreadProperty>::parse(
            map(json!({"accountId": "a", "ids": null})),
            10,
        )
        .unwrap();
        assert!(all.ids.is_none() && all.properties.is_none());
    }

    #[test]
    fn repeated_ids_are_returned_once() {
        let mut args = GetArguments::<MailboxId, MailboxProperty>::parse(
            map(json!({"accountId": "a", "ids": ["m1", "m2", "m1", "#new", "#gone", "m3", "#gone"]})),
            10,
        )
        .unwrap();
        let mut created = CreatedIds::default();
        created.insert(
            "new".parse().unwrap(),
            "m3".parse::<crate::types::Id>().unwrap(),
        );
        args.resolve_ids(&created);
        let ids: Vec<String> = args.ids.unwrap().iter().map(ToString::to_string).collect();
        assert_eq!(ids, ["m1", "m2", "m3", "#gone"]);
    }

    #[test]
    fn errors() {
        let kind = |args: Value, max| {
            GetArguments::<MailboxId, MailboxProperty>::parse(map(args), max)
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"ids": []}), 5),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "properties": ["nope"]}), 5),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "ids": ["a b"]}), 5),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "extra": 1}), 5),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "ids": ["a", "b", "c"]}), 2),
            MethodErrorType::RequestTooLarge
        );
    }
}
