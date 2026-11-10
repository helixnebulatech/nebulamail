// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/query` (RFC 8620 §5.5): filters, sorting and paging.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::{from_arguments, take_argument};
use crate::error::MethodError;
use crate::macros::string_enum;
use crate::types::serde_util::null_as_default;
use crate::types::{AccountId, IdKind, Int, State, UnsignedInt};

/// Deepest filter tree accepted: deeper trees are `invalidArguments`.
/// Real clients nest two or three levels.
pub const MAX_FILTER_DEPTH: usize = 16;

/// Most nodes (operators and conditions) in one filter tree: more is
/// `unsupportedFilter` (RFC 8620 §5.5: valid, but this server won't run
/// it). The elements of a list inside a condition (`emailIds`,
/// `inMailboxOtherThan`…) are not nodes: they have their own cap,
/// [`MAX_FILTER_LIST`]. Every `/query`, `/queryChanges` and
/// `SearchSnippet/get` goes through [`Filter::parse`], after result
/// references are resolved, so `Core/echo` can't make a filter wider.
pub const MAX_FILTER_NODES: usize = 256;

/// Longest string inside a filter condition, in bytes.
pub const MAX_FILTER_TEXT: usize = 1024;

/// Most elements of one list inside a filter condition. It is at least
/// `maxObjectsInGet` (the bridge refuses a configuration above it), so a
/// client can filter by every id of one `/get` page.
pub const MAX_FILTER_LIST: usize = 1000;

string_enum! {
    /// The operator of a [`FilterOperator`].
    pub enum Operator ("filter operator") {
        /// All conditions must match.
        And = "AND",
        /// At least one condition must match.
        Or = "OR",
        /// No condition may match.
        Not = "NOT",
    }
}

/// A filter: a condition, or an operator over more filters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Filter<C> {
    /// `{"operator": "AND", "conditions": [...]}`.
    Operator(FilterOperator<C>),
    /// A condition object of the data type.
    Condition(C),
}

/// `{"operator": ..., "conditions": [...]}` (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterOperator<C> {
    /// How the conditions combine.
    pub operator: Operator,
    /// The sub-filters.
    pub conditions: Vec<Filter<C>>,
}

/// A data type's `FilterCondition`.
pub trait FilterCondition: DeserializeOwned {
    /// Every property of the condition. Any other key is an
    /// `unsupportedFilter` error (not `invalidArguments`), as RFC 8620
    /// §5.5 asks.
    const PROPERTIES: &'static [&'static str];
}

impl<C: FilterCondition> Filter<C> {
    /// Parses a filter.
    ///
    /// # Errors
    /// `unsupportedFilter` for unknown condition properties or a filter
    /// over [`MAX_FILTER_NODES`], [`MAX_FILTER_TEXT`] or
    /// [`MAX_FILTER_LIST`]; `invalidArguments` for wrong types or a tree
    /// deeper than [`MAX_FILTER_DEPTH`].
    pub fn parse(value: Value) -> Result<Self, MethodError> {
        let mut nodes = 0;
        parse_filter(value, 0, &mut nodes)
    }
}

fn too_large() -> MethodError {
    MethodError::unsupported_filter("filter is too large")
}

fn parse_filter<C: FilterCondition>(
    value: Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<Filter<C>, MethodError> {
    if depth >= MAX_FILTER_DEPTH {
        return Err(MethodError::invalid_arguments(
            "filter is nested too deeply",
        ));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_FILTER_NODES {
        return Err(too_large());
    }
    let Value::Object(mut map) = value else {
        return Err(MethodError::invalid_arguments("filter must be an object"));
    };
    let Some(operator) = map.remove("operator") else {
        return parse_condition(map).map(Filter::Condition);
    };
    let operator: Operator = serde_json::from_value(operator)
        .map_err(|error| MethodError::invalid_arguments(format!("filter operator: {error}")))?;
    let Some(Value::Array(conditions)) = map.remove("conditions") else {
        return Err(MethodError::invalid_arguments(
            "filter operator needs a \"conditions\" array",
        ));
    };
    if let Some(key) = map.keys().next() {
        return Err(MethodError::invalid_arguments(format!(
            "unknown member {key:?} in filter operator"
        )));
    }
    // Too many children: refuse before parsing any of them.
    if nodes.saturating_add(conditions.len()) > MAX_FILTER_NODES {
        return Err(too_large());
    }
    let depth = depth.saturating_add(1);
    let conditions = conditions
        .into_iter()
        .map(|condition| parse_filter(condition, depth, nodes))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Filter::Operator(FilterOperator {
        operator,
        conditions,
    }))
}

fn parse_condition<C: FilterCondition>(map: Map<String, Value>) -> Result<C, MethodError> {
    if let Some(key) = map
        .keys()
        .find(|key| !C::PROPERTIES.contains(&key.as_str()))
    {
        return Err(MethodError::unsupported_filter(format!(
            "unsupported filter property {key:?}"
        )));
    }
    if !map.values().all(within_limits) {
        return Err(too_large());
    }
    serde_json::from_value(Value::Object(map))
        .map_err(|error| MethodError::invalid_arguments(format!("filter: {error}")))
}

/// Whether a condition value keeps to [`MAX_FILTER_TEXT`] and
/// [`MAX_FILTER_LIST`], checked before deserializing so an oversized value
/// costs nothing more. Condition values are at most two levels deep
/// (`header: ["Name", "value"]`); anything deeper is rejected later by the
/// condition's own type, and `serde_json` already caps the nesting.
fn within_limits(value: &Value) -> bool {
    match value {
        Value::String(text) => text.len() <= MAX_FILTER_TEXT,
        Value::Array(items) => items.len() <= MAX_FILTER_LIST && items.iter().all(within_limits),
        Value::Object(members) => members
            .iter()
            .all(|(key, member)| key.len() <= MAX_FILTER_TEXT && within_limits(member)),
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
    }
}

string_enum! {
    /// Collation algorithms (RFC 4790, IANA registry) for sorting text.
    pub enum Collation ("collation") {
        /// `i;ascii-casemap`: ASCII case-insensitive.
        AsciiCasemap = "i;ascii-casemap",
        /// `i;ascii-numeric`: numbers.
        AsciiNumeric = "i;ascii-numeric",
        /// `i;octet`: byte by byte.
        Octet = "i;octet",
        /// `i;unicode-casemap` (RFC 5051): Unicode case-insensitive.
        UnicodeCasemap = "i;unicode-casemap",
    }
}

/// A data type's sort properties.
pub trait SortProperty: Sized {
    /// Parses `property`. `extra` holds the comparator's other members
    /// (beyond `property`, `isAscending`, `collation`): take the ones this
    /// property uses (e.g. `keyword`); any left over are rejected.
    ///
    /// # Errors
    /// `unsupportedSort` for unknown properties; `invalidArguments` for
    /// bad extra members.
    fn parse(property: &str, extra: &mut Map<String, Value>) -> Result<Self, MethodError>;
}

/// A Comparator (RFC 8620 §5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparator<S> {
    /// What to sort by.
    pub property: S,
    /// Ascending (the default) or descending.
    pub is_ascending: bool,
    /// The collation for text, if the client chose one.
    pub collation: Option<Collation>,
}

impl<S: SortProperty> Comparator<S> {
    /// Parses a `sort` argument.
    ///
    /// # Errors
    /// `unsupportedSort` for unknown properties or collations;
    /// `invalidArguments` for wrong types.
    pub fn parse_list(value: Value) -> Result<Vec<Self>, MethodError> {
        let Value::Array(items) = value else {
            return Err(MethodError::invalid_arguments("sort must be an array"));
        };
        items.into_iter().map(Self::parse_one).collect()
    }

    fn parse_one(value: Value) -> Result<Self, MethodError> {
        let Value::Object(mut map) = value else {
            return Err(MethodError::invalid_arguments(
                "a comparator must be an object",
            ));
        };
        let property: String = take_argument(&mut map, "property")?
            .ok_or_else(|| MethodError::invalid_arguments("comparator needs a property"))?;
        let is_ascending = take_argument(&mut map, "isAscending")?.unwrap_or(true);
        let collation = match take_argument::<String>(&mut map, "collation")? {
            None => None,
            Some(name) => Some(
                name.parse::<Collation>()
                    .map_err(|error| MethodError::unsupported_sort(error.to_string()))?,
            ),
        };
        let property = S::parse(&property, &mut map)?;
        if let Some(key) = map.keys().next() {
            return Err(MethodError::invalid_arguments(format!(
                "unknown member {key:?} in comparator"
            )));
        }
        Ok(Self {
            property,
            is_ascending,
            collation,
        })
    }
}

/// The arguments every `/query` has. Data types with extra arguments
/// (`collapseThreads`, `sortAsTree`…) wrap this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryArguments<C, S, I> {
    /// The account.
    pub account_id: AccountId,
    /// Which objects; `None` means all.
    pub filter: Option<Filter<C>>,
    /// Sort order; empty means the server's default.
    pub sort: Vec<Comparator<S>>,
    /// Index of the first result (negative counts from the end). Ignored
    /// if `anchor` is given.
    pub position: Int,
    /// Start at this id instead of at `position`.
    pub anchor: Option<I>,
    /// Offset from the anchor.
    pub anchor_offset: Int,
    /// Most results to return.
    pub limit: Option<UnsignedInt>,
    /// Whether to return `total`.
    pub calculate_total: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "I: IdKind"))]
struct QueryWire<I> {
    account_id: AccountId,
    #[serde(default, deserialize_with = "null_as_default")]
    position: Int,
    #[serde(default)]
    anchor: Option<I>,
    #[serde(default, deserialize_with = "null_as_default")]
    anchor_offset: Int,
    #[serde(default)]
    limit: Option<UnsignedInt>,
    #[serde(default, deserialize_with = "null_as_default")]
    calculate_total: bool,
}

impl<C, S, I> QueryArguments<C, S, I>
where
    C: FilterCondition,
    S: SortProperty,
    I: IdKind,
{
    /// Parses the standard arguments. Remove any type-specific arguments
    /// from `arguments` first: unknown ones are `invalidArguments`.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let (filter, sort) = take_filter_and_sort(&mut arguments)?;
        let wire: QueryWire<I> = from_arguments(arguments)?;
        Ok(Self {
            account_id: wire.account_id,
            filter,
            sort,
            position: wire.position,
            anchor: wire.anchor,
            anchor_offset: wire.anchor_offset,
            limit: wire.limit,
            calculate_total: wire.calculate_total,
        })
    }
}

/// Filter and sort, shared by `/query` and `/queryChanges`.
pub(crate) type FilterAndSort<C, S> = (Option<Filter<C>>, Vec<Comparator<S>>);

pub(crate) fn take_filter_and_sort<C: FilterCondition, S: SortProperty>(
    arguments: &mut Map<String, Value>,
) -> Result<FilterAndSort<C, S>, MethodError> {
    let filter = match arguments.remove("filter") {
        None | Some(Value::Null) => None,
        Some(value) => Some(Filter::parse(value)?),
    };
    let sort = match arguments.remove("sort") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => Comparator::parse_list(value)?,
    };
    Ok((filter, sort))
}

/// Response of a standard `Foo/query`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse<I> {
    /// The account.
    pub account_id: AccountId,
    /// State of this query's results.
    pub query_state: State,
    /// Whether `/queryChanges` can work from this state.
    pub can_calculate_changes: bool,
    /// Index of the first returned id in the full results.
    pub position: UnsignedInt,
    /// The ids.
    pub ids: Vec<I>,
    /// Number of results, if `calculateTotal` was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<UnsignedInt>,
    /// The limit used, if the server lowered the client's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<UnsignedInt>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::object::{EmailFilterCondition, EmailSort};
    use crate::types::EmailId;

    type EmailQuery = QueryArguments<EmailFilterCondition, EmailSort, EmailId>;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn nested_filters() {
        let args = EmailQuery::parse(map(json!({
            "accountId": "a",
            "filter": {"operator": "AND", "conditions": [
                {"inMailbox": "m1"},
                {"operator": "NOT", "conditions": [{"hasKeyword": "$seen"}]}
            ]},
            "sort": [{"property": "receivedAt", "isAscending": false}],
            "position": 0, "limit": 50, "calculateTotal": true
        })))
        .unwrap();
        let Some(Filter::Operator(and)) = args.filter else {
            panic!("expected an operator")
        };
        assert_eq!(and.operator, Operator::And);
        assert_eq!(and.conditions.len(), 2);
        assert_eq!(args.sort.len(), 1);
        assert!(!args.sort[0].is_ascending);
        assert_eq!(args.limit, UnsignedInt::new(50));
        assert!(args.calculate_total);
    }

    #[test]
    fn filter_errors() {
        let kind = |filter: Value| {
            EmailQuery::parse(map(json!({"accountId": "a", "filter": filter})))
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"fuzzy": "x"})),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(
            kind(json!({"minSize": "big"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"operator": "XOR", "conditions": []})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"operator": "AND"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(kind(json!([])), MethodErrorType::InvalidArguments);
        let mut deep = json!({"text": "x"});
        for _ in 0..MAX_FILTER_DEPTH {
            deep = json!({"operator": "OR", "conditions": [deep]});
        }
        assert_eq!(kind(deep), MethodErrorType::InvalidArguments);
    }

    #[test]
    fn filter_size_limits() {
        let parse =
            |filter: Value| EmailQuery::parse(map(json!({"accountId": "a", "filter": filter})));
        let kind = |filter: Value| parse(filter).unwrap_err().kind();
        // An OR plus 255 conditions is 256 nodes; one more is too many.
        let flat = |count: usize| {
            let conditions: Vec<Value> = (0..count)
                .map(|n| json!({"subject": format!("s{n}"), "hasKeyword": "$seen", "minSize": n}))
                .collect();
            json!({"operator": "OR", "conditions": conditions})
        };
        assert!(parse(flat(MAX_FILTER_NODES - 1)).is_ok());
        assert_eq!(
            kind(flat(MAX_FILTER_NODES)),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(kind(flat(300)), MethodErrorType::UnsupportedFilter);
        // Nodes count across the whole tree, not per level.
        let half = |count: usize| {
            let conditions: Vec<Value> = (0..count).map(|_| json!({"text": "x"})).collect();
            json!({"operator": "OR", "conditions": conditions})
        };
        // 1 AND + 2 OR + 253 conditions = 256 nodes.
        let split = json!({"operator": "AND", "conditions": [half(126), half(127)]});
        assert!(parse(split).is_ok());
        let split = json!({"operator": "AND", "conditions": [half(127), half(127)]});
        assert_eq!(kind(split), MethodErrorType::UnsupportedFilter);
        // Texts.
        let text = "a".repeat(MAX_FILTER_TEXT);
        assert!(parse(json!({"subject": text})).is_ok());
        let text = "a".repeat(MAX_FILTER_TEXT + 1);
        assert_eq!(
            kind(json!({"subject": text})),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(
            kind(json!({"header": ["X-Long", "b".repeat(MAX_FILTER_TEXT + 1)]})),
            MethodErrorType::UnsupportedFilter
        );
        // Lists: one /get page of ids always fits.
        let ids = |count: usize| -> Vec<String> { (0..count).map(|n| format!("m{n}")).collect() };
        assert!(parse(json!({"inMailboxOtherThan": ids(500)})).is_ok());
        assert!(parse(json!({"inMailboxOtherThan": ids(MAX_FILTER_LIST)})).is_ok());
        assert_eq!(
            kind(json!({"inMailboxOtherThan": ids(MAX_FILTER_LIST + 1)})),
            MethodErrorType::UnsupportedFilter
        );
    }

    #[test]
    fn submission_email_ids_list() {
        use crate::object::{EmailSubmissionFilterCondition, EmailSubmissionSort};
        use crate::types::EmailSubmissionId;
        type SubmissionQuery =
            QueryArguments<EmailSubmissionFilterCondition, EmailSubmissionSort, EmailSubmissionId>;
        let ids = |count: usize| -> Vec<String> { (0..count).map(|n| format!("e{n}")).collect() };
        let parse = |count: usize| {
            SubmissionQuery::parse(map(
                json!({"accountId": "a", "filter": {"emailIds": ids(count)}}),
            ))
        };
        assert!(parse(500).is_ok());
        assert_eq!(
            parse(MAX_FILTER_LIST + 1).unwrap_err().kind(),
            MethodErrorType::UnsupportedFilter
        );
    }

    #[test]
    fn filter_size_is_bounded() {
        let parse = |filter: Value| {
            EmailQuery::parse(map(json!({"accountId": "a", "filter": filter})))
                .map(|_| ())
                .map_err(|error| error.kind())
        };
        let conditions = |n: usize| vec![json!({"text": "x"}); n];
        // The operator is a node too: 255 conditions + 1 = 256.
        let and = |n: usize| json!({"operator": "AND", "conditions": conditions(n)});
        assert_eq!(parse(and(MAX_FILTER_NODES - 1)), Ok(()));
        assert_eq!(
            parse(and(MAX_FILTER_NODES)),
            Err(MethodErrorType::UnsupportedFilter)
        );
        // Nested operators count with their conditions.
        let half = MAX_FILTER_NODES / 2;
        let nested = json!({"operator": "OR", "conditions": [and(half), and(half)]});
        assert_eq!(parse(nested), Err(MethodErrorType::UnsupportedFilter));
        let fits = json!({"operator": "OR", "conditions": [and(half - 2), and(half - 2)]});
        assert_eq!(parse(fits), Ok(()));
    }

    #[test]
    fn sort_errors() {
        let kind = |sort: Value| {
            EmailQuery::parse(map(json!({"accountId": "a", "sort": sort})))
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!([{"property": "colour"}])),
            MethodErrorType::UnsupportedSort
        );
        assert_eq!(
            kind(json!([{"property": "subject", "collation": "i;klingon"}])),
            MethodErrorType::UnsupportedSort
        );
        assert_eq!(
            kind(json!([{"property": "subject", "extra": 1}])),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!([{"property": "hasKeyword"}])),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"property": "size"})),
            MethodErrorType::InvalidArguments
        );
    }

    #[test]
    fn response_shape() {
        let response = QueryResponse::<EmailId> {
            account_id: "a".parse().unwrap(),
            query_state: State::new("q1"),
            can_calculate_changes: false,
            position: UnsignedInt::ZERO,
            ids: vec!["e1".parse().unwrap()],
            total: Some(UnsignedInt::from(1_u32)),
            limit: None,
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"accountId": "a", "queryState": "q1", "canCalculateChanges": false,
                   "position": 0, "ids": ["e1"], "total": 1})
        );
    }
}
