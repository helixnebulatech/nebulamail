// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ParticipantIdentity` (draft-ietf-jmap-calendars §3): who the user is
//! in events (organizer or attendee).

use serde::Serialize;

use crate::macros::string_enum;
use crate::types::ParticipantIdentityId;

string_enum! {
    /// `ParticipantIdentity` properties (for `properties`).
    pub enum ParticipantIdentityProperty ("ParticipantIdentity property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `calendarAddress`.
        CalendarAddress = "calendarAddress",
        /// `isDefault`.
        IsDefault = "isDefault",
    }
}

/// A `ParticipantIdentity` as returned by `/get`. `None`: not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantIdentity {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<ParticipantIdentityId>,
    /// `name` (default `""`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `calendarAddress`: a URI, usually `mailto:`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_address: Option<String>,
    /// `isDefault`: the one used for new events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;
    use crate::error::MethodErrorType;
    use crate::method::{GetArguments, GetResponse};
    use crate::types::State;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// `me` and `listOwnAddresses` in apps/web/src/calendar/api.ts, and
    /// Stalwart 1.0's answer (which has no `state`; the bridge sends one).
    #[test]
    fn web_get_and_stalwart_response() {
        let args = GetArguments::<ParticipantIdentityId, ParticipantIdentityProperty>::parse(
            map(json!({"accountId": "c"})),
            500,
        )
        .unwrap();
        assert!(args.ids.is_none() && args.properties.is_none());
        let response = GetResponse {
            account_id: "c".parse().unwrap(),
            state: State::new("p1"),
            list: vec![ParticipantIdentity {
                id: Some("a".parse().unwrap()),
                name: Some("Test account".to_owned()),
                calendar_address: Some("mailto:alice@nebulamail.test".to_owned()),
                is_default: Some(true),
            }],
            not_found: Vec::<ParticipantIdentityId>::new(),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "c", "state": "p1",
                "list": [{"id": "a", "name": "Test account",
                          "calendarAddress": "mailto:alice@nebulamail.test", "isDefault": true}],
                "notFound": []
            })
        );
        let error = GetArguments::<ParticipantIdentityId, ParticipantIdentityProperty>::parse(
            map(json!({"accountId": "c", "properties": ["email"]})),
            500,
        )
        .unwrap_err();
        assert_eq!(error.kind(), MethodErrorType::InvalidArguments);
    }
}
