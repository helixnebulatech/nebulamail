// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Principal` (RFC 9670 §2: users, groups, resources and rooms) and
//! `Principal/getAvailability` (draft-ietf-jmap-calendars, "Principal
//! Availability").
//!
//! Checked against Stalwart 0.16.27 (2026-10-27): `Principal/get` sends
//! `id`, `type`, `name`, `description`, `email` (and `timezone` instead of
//! `timeZone`, a bug), `capabilities` and `accounts`;
//! `Principal/getAvailability` answers `{"list": [...]}` with
//! `utcStart`, `utcEnd`, `busyStatus` and `"event": null` per period.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::MethodError;
use crate::macros::string_enum;
use crate::method::parse::from_arguments;
use crate::method::{FilterCondition, QueryArguments, SortProperty};
use crate::types::{AccountId, PrincipalId, UtcDate};

string_enum! {
    /// `Principal` properties (for `properties`).
    pub enum PrincipalProperty ("Principal property") {
        /// `id`.
        Id = "id",
        /// `type`.
        Type = "type",
        /// `name`.
        Name = "name",
        /// `description`.
        Description = "description",
        /// `email`.
        Email = "email",
        /// `timeZone`.
        TimeZone = "timeZone",
        /// `capabilities`.
        Capabilities = "capabilities",
        /// `accounts`.
        Accounts = "accounts",
    }
}

string_enum! {
    /// What a principal is (RFC 9670 §2).
    pub enum PrincipalType ("principal type") {
        /// A person.
        Individual = "individual",
        /// A group of principals.
        Group = "group",
        /// A resource (a projector…).
        Resource = "resource",
        /// A room or another place.
        Location = "location",
        /// Something else.
        Other = "other",
    }
}

/// The `urn:ietf:params:jmap:calendars` capability of a principal (the
/// fields Stalwart sends).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalCalendars {
    /// The account with the principal's calendars, if the user can see it.
    pub account_id: Option<AccountId>,
    /// Whether `Principal/getAvailability` may be asked about it.
    pub may_get_availability: bool,
    /// Whether calendars may be shared with it.
    pub may_share_with: bool,
    /// Its calendar address (`mailto:…`), for invitations.
    pub calendar_address: Option<String>,
}

/// `capabilities` of a principal: what it can do, by capability URI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PrincipalCapabilities {
    /// `urn:ietf:params:jmap:calendars`.
    #[serde(
        rename = "urn:ietf:params:jmap:calendars",
        skip_serializing_if = "Option::is_none"
    )]
    pub calendars: Option<PrincipalCalendars>,
}

/// A `Principal` as returned by `/get`. `None`: not sent; `Some(None)`:
/// `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<PrincipalId>,
    /// `type`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<PrincipalType>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    /// `email`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<Option<String>>,
    /// `timeZone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<Option<String>>,
    /// `capabilities`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PrincipalCapabilities>,
    /// `accounts` (`null`: none the user can see).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Option<Map<String, Value>>>,
}

/// `Principal/query` filter condition (RFC 9670 §3.3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalFilterCondition {
    /// The principal owns one of these accounts.
    #[serde(default)]
    pub account_ids: Option<Vec<AccountId>>,
    /// An address of the principal is this one (no case).
    #[serde(default)]
    pub email: Option<String>,
    /// The name contains this.
    #[serde(default)]
    pub name: Option<String>,
    /// The name, email or description contains this.
    #[serde(default)]
    pub text: Option<String>,
    /// The type is this.
    #[serde(default, rename = "type")]
    pub kind: Option<PrincipalType>,
    /// The time zone is this.
    #[serde(default)]
    pub time_zone: Option<String>,
}

impl FilterCondition for PrincipalFilterCondition {
    const PROPERTIES: &'static [&'static str] =
        &["accountIds", "email", "name", "text", "type", "timeZone"];
}

/// What `Principal/query` sorts by (RFC 9670 defines none; these are
/// jmap-client's, and Stalwart's `name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PrincipalSort {
    /// `name`.
    Name,
    /// `email`.
    Email,
    /// `type`.
    Type,
}

impl SortProperty for PrincipalSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        Ok(match property {
            "name" => Self::Name,
            "email" => Self::Email,
            "type" => Self::Type,
            other => {
                return Err(MethodError::unsupported_sort(format!(
                    "cannot sort principals by {other:?}"
                )));
            }
        })
    }
}

/// `Principal/query` arguments (no extras).
pub type PrincipalQueryArguments =
    QueryArguments<PrincipalFilterCondition, PrincipalSort, PrincipalId>;

/// `Principal/getAvailability` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrincipalGetAvailabilityArguments {
    /// The account.
    pub account_id: AccountId,
    /// Whose busy times.
    pub id: PrincipalId,
    /// From (UTC).
    pub utc_start: UtcDate,
    /// Until (UTC).
    pub utc_end: UtcDate,
    /// Whether to include the events (only where the user may read
    /// them: in the bridge, the user's own public events).
    #[serde(default)]
    pub show_details: bool,
    /// Event properties for `showDetails` (`None`: all).
    #[serde(default)]
    pub event_properties: Option<Vec<String>>,
}

impl PrincipalGetAvailabilityArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`.
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        from_arguments(arguments)
    }
}

string_enum! {
    /// How busy a period is.
    pub enum BusyStatus ("busy status") {
        /// Busy.
        Confirmed = "confirmed",
        /// Maybe busy.
        Tentative = "tentative",
        /// Not available (out of office…).
        Unavailable = "unavailable",
    }
}

/// One busy period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyPeriod {
    /// Start (UTC).
    pub utc_start: UtcDate,
    /// End (UTC).
    pub utc_end: UtcDate,
    /// How busy.
    pub busy_status: BusyStatus,
    /// The event (`showDetails`), else `null`.
    pub event: Option<Value>,
}

/// `Principal/getAvailability` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrincipalGetAvailabilityResponse {
    /// The busy periods, by start.
    pub list: Vec<BusyPeriod>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::method::{Filter, GetArguments, GetResponse, QueryResponse};
    use crate::types::{State, UnsignedInt};

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// `getAvailability` in apps/web/src/calendar/api.ts and
    /// `listPeople` in apps/web/src/mail/sharing.ts.
    #[test]
    fn web_requests() {
        let query = PrincipalQueryArguments::parse(map(
            json!({"accountId": "c", "filter": {"email": "bob@nebulamail.test"}, "limit": 1}),
        ))
        .unwrap();
        let Some(Filter::Condition(condition)) = &query.filter else {
            panic!("{query:?}");
        };
        assert_eq!(condition.email.as_deref(), Some("bob@nebulamail.test"));
        assert_eq!(query.limit, Some(UnsignedInt::from(1_u32)));
        let all =
            PrincipalQueryArguments::parse(map(json!({"accountId": "c", "limit": 500}))).unwrap();
        assert!(all.filter.is_none());
        let get = GetArguments::<PrincipalId, PrincipalProperty>::parse(
            map(json!({"accountId": "c", "ids": ["r1"], "properties": ["name", "email", "type"]})),
            500,
        )
        .unwrap();
        assert_eq!(
            get.properties.unwrap(),
            [
                PrincipalProperty::Name,
                PrincipalProperty::Email,
                PrincipalProperty::Type
            ]
        );
        let availability = PrincipalGetAvailabilityArguments::parse(map(json!({
            "accountId": "c", "id": "r1",
            "utcStart": "2026-10-28T10:00:00Z", "utcEnd": "2026-10-28T11:00:00Z"
        })))
        .unwrap();
        assert_eq!(availability.id.as_str(), "r1");
        assert!(!availability.show_details);
        assert_eq!(
            availability.utc_end.unix_timestamp() - availability.utc_start.unix_timestamp(),
            3600
        );
    }

    #[test]
    fn bad_arguments() {
        let kind = |value: Value| {
            PrincipalGetAvailabilityArguments::parse(map(value))
                .unwrap_err()
                .kind()
        };
        // Not UTC, missing end, unknown member.
        assert_eq!(
            kind(json!({"accountId": "c", "id": "r1",
                "utcStart": "2026-10-28T10:00:00", "utcEnd": "2026-10-28T11:00:00Z"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "id": "r1", "utcStart": "2026-10-28T10:00:00Z"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(
                json!({"accountId": "c", "id": "r1", "utcStart": "2026-10-28T10:00:00Z",
                "utcEnd": "2026-10-28T11:00:00Z", "extra": 1})
            ),
            MethodErrorType::InvalidArguments
        );
        let filter = PrincipalQueryArguments::parse(map(
            json!({"accountId": "c", "filter": {"nickname": "bob"}}),
        ))
        .unwrap_err();
        assert_eq!(filter.kind(), MethodErrorType::UnsupportedFilter);
        let sort = PrincipalQueryArguments::parse(map(
            json!({"accountId": "c", "sort": [{"property": "created"}]}),
        ))
        .unwrap_err();
        assert_eq!(sort.kind(), MethodErrorType::UnsupportedSort);
        let kind = PrincipalQueryArguments::parse(map(
            json!({"accountId": "c", "filter": {"type": "robot"}}),
        ))
        .unwrap_err();
        assert_eq!(kind.kind(), MethodErrorType::InvalidArguments);
    }

    /// The answers in Stalwart's shape.
    #[test]
    fn responses_like_stalwart() {
        let get = GetResponse {
            account_id: "c".parse().unwrap(),
            state: State::new("r1"),
            list: vec![Principal {
                id: Some("r7".parse().unwrap()),
                kind: Some(PrincipalType::Individual),
                name: Some("bob@nebulamail.test".to_owned()),
                description: Some(None),
                email: Some(Some("bob@nebulamail.test".to_owned())),
                time_zone: Some(None),
                capabilities: Some(PrincipalCapabilities {
                    calendars: Some(PrincipalCalendars {
                        account_id: None,
                        may_get_availability: true,
                        may_share_with: false,
                        calendar_address: Some("mailto:bob@nebulamail.test".to_owned()),
                    }),
                }),
                accounts: Some(None),
            }],
            not_found: vec!["r9".parse::<PrincipalId>().unwrap()],
        };
        assert_eq!(
            serde_json::to_value(&get).unwrap(),
            json!({
                "accountId": "c", "state": "r1", "notFound": ["r9"],
                "list": [{
                    "id": "r7", "type": "individual", "name": "bob@nebulamail.test",
                    "description": null, "email": "bob@nebulamail.test", "timeZone": null,
                    "capabilities": {"urn:ietf:params:jmap:calendars": {
                        "accountId": null, "mayGetAvailability": true, "mayShareWith": false,
                        "calendarAddress": "mailto:bob@nebulamail.test"
                    }},
                    "accounts": null
                }]
            })
        );
        let query = QueryResponse {
            account_id: "c".parse().unwrap(),
            query_state: State::new("r1"),
            can_calculate_changes: false,
            position: UnsignedInt::ZERO,
            ids: vec!["r7".parse::<PrincipalId>().unwrap()],
            total: None,
            limit: None,
        };
        assert_eq!(serde_json::to_value(&query).unwrap()["ids"], json!(["r7"]));
        let availability = PrincipalGetAvailabilityResponse {
            list: vec![BusyPeriod {
                utc_start: "2026-10-20T13:30:00Z".parse().unwrap(),
                utc_end: "2026-10-20T14:30:00Z".parse().unwrap(),
                busy_status: BusyStatus::Confirmed,
                event: None,
            }],
        };
        assert_eq!(
            serde_json::to_value(&availability).unwrap(),
            json!({"list": [{
                "utcStart": "2026-10-20T13:30:00Z", "utcEnd": "2026-10-20T14:30:00Z",
                "busyStatus": "confirmed", "event": null
            }]})
        );
    }
}
