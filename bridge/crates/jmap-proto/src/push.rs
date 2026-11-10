// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Push (RFC 8620 §7): `StateChange` objects, `CalendarAlert`s
//! (draft-ietf-jmap-calendars §6.4) and the `EventSource` URL parameters.

use std::collections::{BTreeMap, BTreeSet};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::macros::string_enum;
use crate::types::{AccountId, CalendarEventId, DataType, LocalDateTime, State, UnsignedInt};

/// A `StateChange` (RFC 8620 §7.1): new states per account and type.
///
/// ```json
/// {"@type": "StateChange", "changed": {"a1": {"Email": "s12", "Mailbox": "s5"}}}
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateChange {
    /// Account → data type → new state.
    pub changed: BTreeMap<AccountId, BTreeMap<DataType, State>>,
}

impl Serialize for StateChange {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("@type", "StateChange")?;
        map.serialize_entry("changed", &self.changed)?;
        map.end()
    }
}

/// An `EmailPush` (Stalwart 1.0, [`crate::Capability::EmailPush`]): the
/// delivered emails of an account, with the properties the subscription
/// asked for, sent instead of the delivery's `StateChange`.
///
/// ```json
/// {"@type": "EmailPush", "accountId": "a1",
///  "emails": [{"id": "e1", "subject": "Hi"}], "state": "s12"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailPush {
    /// The account.
    pub account_id: AccountId,
    /// One object per email (property → value).
    pub emails: Vec<serde_json::Map<String, serde_json::Value>>,
    /// The account's `Email` state after the delivery, if known.
    pub state: Option<State>,
}

impl Serialize for EmailPush {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("@type", "EmailPush")?;
        map.serialize_entry("accountId", &self.account_id)?;
        map.serialize_entry("emails", &self.emails)?;
        if let Some(state) = &self.state {
            map.serialize_entry("state", state)?;
        }
        map.end()
    }
}

/// Why a `CalendarAlert` can't be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CalendarAlertError {
    /// `alertId` is empty, longer than 255 bytes or has control
    /// characters.
    #[error("invalid alert id")]
    AlertId,
    /// `uid` is empty or has control characters.
    #[error("invalid uid")]
    Uid,
}

/// A `CalendarAlert` push object: an event's alert is due now. Sent to
/// clients that asked for the `CalendarAlert` type, on the event source
/// as a `calendarAlert` event (the name Stalwart uses and
/// apps/web/src/calendar/alerts.ts reads).
///
/// ```json
/// {"@type": "CalendarAlert", "accountId": "c", "calendarEventId": "l",
///  "uid": "u1", "recurrenceId": "2026-10-26T10:00:00", "alertId": "a1"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarAlert {
    account_id: AccountId,
    calendar_event_id: CalendarEventId,
    uid: String,
    recurrence_id: Option<LocalDateTime>,
    alert_id: String,
}

impl CalendarAlert {
    /// The `EventSource` event name.
    pub const EVENT_NAME: &'static str = "calendarAlert";

    /// An alert of `alert_id` in event `calendar_event_id` (`uid`), for the
    /// occurrence `recurrence_id` of a recurring event.
    ///
    /// # Errors
    /// [`CalendarAlertError`] for an empty or too long alert id, or an
    /// empty uid; neither may have control characters.
    pub fn new(
        account_id: AccountId,
        calendar_event_id: CalendarEventId,
        uid: impl Into<String>,
        recurrence_id: Option<LocalDateTime>,
        alert_id: impl Into<String>,
    ) -> Result<Self, CalendarAlertError> {
        let uid = uid.into();
        let alert_id = alert_id.into();
        if alert_id.is_empty() || alert_id.len() > 255 || alert_id.chars().any(char::is_control) {
            return Err(CalendarAlertError::AlertId);
        }
        if uid.is_empty() || uid.chars().any(char::is_control) {
            return Err(CalendarAlertError::Uid);
        }
        Ok(Self {
            account_id,
            calendar_event_id,
            uid,
            recurrence_id,
            alert_id,
        })
    }

    /// The account.
    #[must_use]
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// The event (the series for a recurring one).
    #[must_use]
    pub fn calendar_event_id(&self) -> &CalendarEventId {
        &self.calendar_event_id
    }

    /// The event's `uid`.
    #[must_use]
    pub fn uid(&self) -> &str {
        &self.uid
    }

    /// The occurrence, for a recurring event.
    #[must_use]
    pub fn recurrence_id(&self) -> Option<LocalDateTime> {
        self.recurrence_id
    }

    /// The key of the alert in the event's `alerts`.
    #[must_use]
    pub fn alert_id(&self) -> &str {
        &self.alert_id
    }
}

impl Serialize for CalendarAlert {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("@type", "CalendarAlert")?;
        map.serialize_entry("accountId", &self.account_id)?;
        map.serialize_entry("calendarEventId", &self.calendar_event_id)?;
        map.serialize_entry("uid", &self.uid)?;
        map.serialize_entry("recurrenceId", &self.recurrence_id)?;
        map.serialize_entry("alertId", &self.alert_id)?;
        map.end()
    }
}

/// The data of a `ping` event: the interval the server uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PingEvent {
    /// Seconds between pings.
    pub interval: UnsignedInt,
}

/// Which types to push (`types` URL variable).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PushTypes {
    /// `*`: every type.
    All,
    /// Only these types.
    Only(BTreeSet<DataType>),
}

impl PushTypes {
    /// Whether changes of `data_type` should be pushed.
    #[must_use]
    pub fn includes(&self, data_type: &DataType) -> bool {
        match self {
            Self::All => true,
            Self::Only(types) => types.contains(data_type),
        }
    }
}

string_enum! {
    /// `closeafter` URL variable.
    pub enum CloseAfter ("closeafter value") {
        /// End the response after the first state event.
        State = "state",
        /// Keep the connection open.
        No = "no",
    }
}

/// Why `EventSource` query parameters are invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EventSourceError {
    /// `types` is not `*` or a comma-separated list of type names.
    #[error("invalid types")]
    Types,
    /// `closeafter` is not `state` or `no`.
    #[error("invalid closeafter")]
    CloseAfter,
    /// `ping` is not a non-negative integer.
    #[error("invalid ping")]
    Ping,
}

/// The parsed variables of an `EventSource` request (RFC 8620 §7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSourceParams {
    /// Types to push.
    pub types: PushTypes,
    /// When to close.
    pub close_after: CloseAfter,
    /// Seconds between pings; 0 means no pings. Clamp it before use.
    pub ping: UnsignedInt,
}

impl EventSourceParams {
    /// Parses the three query values (already percent-decoded).
    ///
    /// # Errors
    /// [`EventSourceError`] naming the bad parameter.
    pub fn parse(types: &str, close_after: &str, ping: &str) -> Result<Self, EventSourceError> {
        let types = if types == "*" {
            PushTypes::All
        } else {
            PushTypes::Only(
                types
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| EventSourceError::Types)?,
            )
        };
        let close_after = close_after
            .parse()
            .map_err(|_| EventSourceError::CloseAfter)?;
        let ping = ping
            .parse::<u64>()
            .ok()
            .and_then(UnsignedInt::new)
            .ok_or(EventSourceError::Ping)?;
        Ok(Self {
            types,
            close_after,
            ping,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// RFC 8620 §7.1.1.
    #[test]
    fn rfc_state_change() {
        let change = StateChange {
            changed: [
                (
                    "a3123".parse().unwrap(),
                    [
                        (DataType::Email, State::new("d35ecb040aab")),
                        (DataType::EmailDelivery, State::new("428d565f2440")),
                        ("CalendarEvent".parse().unwrap(), State::new("87accfac587a")),
                    ]
                    .into(),
                ),
                (
                    "a43461d".parse().unwrap(),
                    [
                        (DataType::Mailbox, State::new("0af7a512ce70")),
                        ("CalendarEvent".parse().unwrap(), State::new("7a4297cecd76")),
                    ]
                    .into(),
                ),
            ]
            .into(),
        };
        assert_eq!(
            serde_json::to_value(&change).unwrap(),
            json!({
                "@type": "StateChange",
                "changed": {
                    "a3123": {
                        "Email": "d35ecb040aab",
                        "EmailDelivery": "428d565f2440",
                        "CalendarEvent": "87accfac587a"
                    },
                    "a43461d": {
                        "Mailbox": "0af7a512ce70",
                        "CalendarEvent": "7a4297cecd76"
                    }
                }
            })
        );
        assert_eq!(
            serde_json::to_value(PingEvent {
                interval: UnsignedInt::from(30_u32)
            })
            .unwrap(),
            json!({"interval": 30})
        );
    }

    /// The shape `CalendarAlertSchema` (packages/jmap-client) accepts.
    #[test]
    fn calendar_alert() {
        let alert = CalendarAlert::new(
            "c".parse().unwrap(),
            "l".parse().unwrap(),
            "nebula-seed-weekly-sync",
            Some("2026-10-26T10:00:00".parse().unwrap()),
            "a1",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&alert).unwrap(),
            json!({
                "@type": "CalendarAlert", "accountId": "c", "calendarEventId": "l",
                "uid": "nebula-seed-weekly-sync", "recurrenceId": "2026-10-26T10:00:00",
                "alertId": "a1"
            })
        );
        assert_eq!(alert.alert_id(), "a1");
        let single = CalendarAlert::new(
            "c".parse().unwrap(),
            "m".parse().unwrap(),
            "u",
            None,
            "snz1",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&single).unwrap()["recurrenceId"],
            Value::Null
        );
        assert_eq!(CalendarAlert::EVENT_NAME, "calendarAlert");
        let new = |uid: &str, alert: &str| {
            CalendarAlert::new("c".parse().unwrap(), "m".parse().unwrap(), uid, None, alert)
        };
        assert_eq!(new("u", ""), Err(CalendarAlertError::AlertId));
        assert_eq!(new("u", &"a".repeat(256)), Err(CalendarAlertError::AlertId));
        assert_eq!(new("u", "a\n"), Err(CalendarAlertError::AlertId));
        assert_eq!(new("", "a1"), Err(CalendarAlertError::Uid));
        // The web subscribes with `types=CalendarAlert`; calendar changes
        // come as `Calendar` and `CalendarEvent` states.
        let params = EventSourceParams::parse("CalendarAlert", "no", "0").unwrap();
        assert!(params.types.includes(&DataType::CalendarAlert));
        let params = EventSourceParams::parse("Calendar,CalendarEvent", "no", "0").unwrap();
        assert!(params.types.includes(&DataType::CalendarEvent));
        assert!(!params.types.includes(&DataType::ContactCard));
    }

    #[test]
    fn event_source_params() {
        let params = EventSourceParams::parse("Email,Mailbox,Thread", "no", "30").unwrap();
        assert!(params.types.includes(&DataType::Email));
        assert!(!params.types.includes(&DataType::Identity));
        assert_eq!(params.close_after, CloseAfter::No);
        assert_eq!(params.ping, UnsignedInt::from(30_u32));
        // A client may ask for types this server lacks (calendar push).
        assert!(EventSourceParams::parse("Calendar,CalendarEvent", "state", "0").is_ok());
        assert!(
            EventSourceParams::parse("*", "no", "0")
                .unwrap()
                .types
                .includes(&DataType::Quota)
        );
        assert_eq!(
            EventSourceParams::parse("Email,,Mailbox", "no", "0"),
            Err(EventSourceError::Types)
        );
        assert_eq!(
            EventSourceParams::parse("Email", "maybe", "0"),
            Err(EventSourceError::CloseAfter)
        );
        assert_eq!(
            EventSourceParams::parse("Email", "no", "-1"),
            Err(EventSourceError::Ping)
        );
    }
}
