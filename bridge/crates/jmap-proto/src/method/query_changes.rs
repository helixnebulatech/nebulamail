// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/queryChanges` (RFC 8620 §5.6).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::from_arguments;
use super::query::{Comparator, Filter, FilterCondition, SortProperty, take_filter_and_sort};
use crate::error::MethodError;
use crate::types::serde_util::null_as_default;
use crate::types::{AccountId, IdKind, PositiveInt, State, UnsignedInt};

/// The arguments every `/queryChanges` has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryChangesArguments<C, S, I> {
    /// The account.
    pub account_id: AccountId,
    /// The filter of the original query.
    pub filter: Option<Filter<C>>,
    /// The sort of the original query.
    pub sort: Vec<Comparator<S>>,
    /// The `queryState` the client has.
    pub since_query_state: State,
    /// Most changes to return.
    pub max_changes: Option<PositiveInt>,
    /// The last id the client cares about.
    pub up_to_id: Option<I>,
    /// Whether to return `total`.
    pub calculate_total: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "I: IdKind"))]
struct QueryChangesWire<I> {
    account_id: AccountId,
    since_query_state: State,
    #[serde(default)]
    max_changes: Option<PositiveInt>,
    #[serde(default)]
    up_to_id: Option<I>,
    #[serde(default, deserialize_with = "null_as_default")]
    calculate_total: bool,
}

impl<C, S, I> QueryChangesArguments<C, S, I>
where
    C: FilterCondition,
    S: SortProperty,
    I: IdKind,
{
    /// Parses the standard arguments (remove type-specific ones first).
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let (filter, sort) = take_filter_and_sort(&mut arguments)?;
        let wire: QueryChangesWire<I> = from_arguments(arguments)?;
        Ok(Self {
            account_id: wire.account_id,
            filter,
            sort,
            since_query_state: wire.since_query_state,
            max_changes: wire.max_changes,
            up_to_id: wire.up_to_id,
            calculate_total: wire.calculate_total,
        })
    }
}

/// An id added to the results, and its new index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddedItem<I> {
    /// The id.
    pub id: I,
    /// Its index in the new results.
    pub index: UnsignedInt,
}

/// Response of a standard `Foo/queryChanges`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChangesResponse<I> {
    /// The account.
    pub account_id: AccountId,
    /// The `sinceQueryState` of the request.
    pub old_query_state: State,
    /// The current query state.
    pub new_query_state: State,
    /// Number of results, if `calculateTotal` was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<UnsignedInt>,
    /// Ids no longer in the results (or moved).
    pub removed: Vec<I>,
    /// Ids added (or moved), with their index, sorted by index.
    pub added: Vec<AddedItem<I>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::object::{EmailFilterCondition, EmailSort};
    use crate::types::EmailId;

    #[test]
    fn parses() {
        let args = QueryChangesArguments::<EmailFilterCondition, EmailSort, EmailId>::parse(
            json!({
                "accountId": "a", "sinceQueryState": "q1", "maxChanges": 10,
                "filter": {"inMailbox": "m1"}, "upToId": "e9"
            })
            .as_object()
            .unwrap()
            .clone(),
        )
        .unwrap();
        assert!(args.filter.is_some());
        assert_eq!(args.up_to_id.unwrap().as_str(), "e9");
    }

    #[test]
    fn response_shape() {
        let response = QueryChangesResponse::<EmailId> {
            account_id: "a".parse().unwrap(),
            old_query_state: State::new("1"),
            new_query_state: State::new("2"),
            total: None,
            removed: vec!["e1".parse().unwrap()],
            added: vec![AddedItem {
                id: "e2".parse().unwrap(),
                index: UnsignedInt::ZERO,
            }],
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"accountId": "a", "oldQueryState": "1", "newQueryState": "2",
                   "removed": ["e1"], "added": [{"id": "e2", "index": 0}]})
        );
    }
}
