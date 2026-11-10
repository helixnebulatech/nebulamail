// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Quota (RFC 9425): `Quota/get`, `Quota/changes`, `Quota/query` and
//! `Quota/queryChanges`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::MethodError;
use crate::macros::string_enum;
use crate::method::{
    ChangesResponse, FilterCondition, QueryArguments, QueryChangesArguments, SortProperty,
};
use crate::types::{DataType, QuotaId, UnsignedInt};

string_enum! {
    /// What a quota applies to (RFC 9425 §3.1).
    pub enum QuotaScope ("quota scope") {
        /// This account.
        Account = "account",
        /// Every account of the domain.
        Domain = "domain",
        /// Every account of the server.
        Global = "global",
    }
}

string_enum! {
    /// The unit of a quota (RFC 9425 §3.2).
    pub enum ResourceType ("resource type") {
        /// Number of objects.
        Count = "count",
        /// Size in bytes.
        Octets = "octets",
    }
}

string_enum! {
    /// Quota properties.
    pub enum QuotaProperty ("Quota property") {
        /// `id`.
        Id = "id",
        /// `resourceType`.
        ResourceType = "resourceType",
        /// `used`.
        Used = "used",
        /// `hardLimit`.
        HardLimit = "hardLimit",
        /// `scope`.
        Scope = "scope",
        /// `name`.
        Name = "name",
        /// `types`.
        Types = "types",
        /// `warnLimit`.
        WarnLimit = "warnLimit",
        /// `softLimit`.
        SoftLimit = "softLimit",
        /// `description`.
        Description = "description",
    }
}

/// A Quota as returned by `Quota/get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<QuotaId>,
    /// `resourceType`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<ResourceType>,
    /// `used`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<UnsignedInt>,
    /// `hardLimit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_limit: Option<UnsignedInt>,
    /// `scope`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<QuotaScope>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `types` it counts (`Email`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<DataType>>,
    /// `warnLimit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_limit: Option<Option<UnsignedInt>>,
    /// `softLimit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_limit: Option<Option<UnsignedInt>>,
    /// `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
}

/// Quota `FilterCondition` (RFC 9425 §4.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaFilterCondition {
    /// The name contains this (case-insensitive).
    #[serde(default)]
    pub name: Option<String>,
    /// The scope is this.
    #[serde(default)]
    pub scope: Option<QuotaScope>,
    /// The resource type is this.
    #[serde(default)]
    pub resource_type: Option<ResourceType>,
    /// `types` contains this data type.
    #[serde(default, rename = "type")]
    pub data_type: Option<DataType>,
}

impl FilterCondition for QuotaFilterCondition {
    const PROPERTIES: &'static [&'static str] = &["name", "scope", "resourceType", "type"];
}

/// Quota sort properties (RFC 9425 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuotaSort {
    /// `name`.
    Name,
    /// `used`.
    Used,
}

impl SortProperty for QuotaSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        match property {
            "name" => Ok(Self::Name),
            "used" => Ok(Self::Used),
            other => Err(MethodError::unsupported_sort(format!(
                "cannot sort quotas by {other:?}"
            ))),
        }
    }
}

/// `Quota/query` arguments: only the standard ones.
pub type QuotaQueryArguments = QueryArguments<QuotaFilterCondition, QuotaSort, QuotaId>;

/// `Quota/queryChanges` arguments (RFC 9425 §4.5): only the standard ones.
pub type QuotaQueryChangesArguments =
    QueryChangesArguments<QuotaFilterCondition, QuotaSort, QuotaId>;

/// `Quota/changes` response: the standard one and `updatedProperties`
/// (RFC 9425 §4.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaChangesResponse {
    /// The standard part.
    #[serde(flatten)]
    pub changes: ChangesResponse<QuotaId>,
    /// `["used"]` if only `used` changed in the updated quotas; `None`
    /// (`null`) if the server can't tell that.
    pub updated_properties: Option<Vec<QuotaProperty>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 9425 §5.1 (one quota of the response).
    #[test]
    fn rfc_shape() {
        let quota = Quota {
            id: Some("2a06df0d-9865-4e74-a92f-74dcc814270e".parse().unwrap()),
            resource_type: Some(ResourceType::Count),
            used: Some(UnsignedInt::from(1_u32)),
            hard_limit: Some(UnsignedInt::from(50_u32)),
            scope: Some(QuotaScope::Account),
            name: Some("bob@example.com".to_owned()),
            types: Some(vec![DataType::Mailbox, DataType::Email]),
            warn_limit: Some(Some(UnsignedInt::from(45_u32))),
            soft_limit: Some(None),
            description: Some(Some("Personal account usage".to_owned())),
        };
        assert_eq!(
            serde_json::to_value(&quota).unwrap(),
            json!({
                "id": "2a06df0d-9865-4e74-a92f-74dcc814270e",
                "resourceType": "count",
                "used": 1,
                "hardLimit": 50,
                "scope": "account",
                "name": "bob@example.com",
                "types": ["Mailbox", "Email"],
                "warnLimit": 45,
                "softLimit": null,
                "description": "Personal account usage"
            })
        );
    }

    /// RFC 9425 §5.2.
    #[test]
    fn changes_shape() {
        let response = QuotaChangesResponse {
            changes: ChangesResponse {
                account_id: "u33084183".parse().unwrap(),
                old_state: crate::types::State::new("78540"),
                new_state: crate::types::State::new("78542"),
                has_more_changes: false,
                created: vec![],
                updated: vec!["2a06df0d-9865-4e74-a92f-74dcc814270e".parse().unwrap()],
                destroyed: vec![],
            },
            updated_properties: Some(vec![QuotaProperty::Used]),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "u33084183", "oldState": "78540", "newState": "78542",
                "hasMoreChanges": false, "updatedProperties": ["used"],
                "created": [], "updated": ["2a06df0d-9865-4e74-a92f-74dcc814270e"],
                "destroyed": []
            })
        );
        let unknown = QuotaChangesResponse {
            updated_properties: None,
            ..response
        };
        assert_eq!(
            serde_json::to_value(&unknown).unwrap()["updatedProperties"],
            Value::Null
        );
    }

    #[test]
    fn query_arguments() {
        use crate::method::Filter;
        let args = QuotaQueryArguments::parse(
            json!({
                "accountId": "a",
                "filter": {"resourceType": "octets", "type": "Email", "scope": "account"},
                "sort": [{"property": "used", "isAscending": false}]
            })
            .as_object()
            .unwrap()
            .clone(),
        )
        .unwrap();
        let Some(Filter::Condition(condition)) = args.filter else {
            panic!("expected a condition")
        };
        assert_eq!(condition.resource_type, Some(ResourceType::Octets));
        assert_eq!(condition.data_type, Some(DataType::Email));
        assert_eq!(args.sort.len(), 1);
        let bad = |value: Value| QuotaQueryArguments::parse(value.as_object().unwrap().clone());
        assert!(bad(json!({"accountId": "a", "filter": {"usedOver": 1}})).is_err());
        assert!(bad(json!({"accountId": "a", "sort": [{"property": "hardLimit"}]})).is_err());
    }
}
