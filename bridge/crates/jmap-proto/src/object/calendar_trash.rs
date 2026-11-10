// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The calendar trash bin, a vendor extension of the Nebula Mail bridge
//! (capability `https://helixnebula.tech/jmap/bridge`)
//! Nextcloud keeps deleted calendars and events for a while
//! (`nc-calendar-trashbin`), and JMAP has nothing for it.
//!
//! - `CalendarTrash/get` `{accountId}` → `{accountId, retentionSeconds,
//!   calendars: [{id, name, color, deletedAt}], events: [{id, title,
//!   utcStart, calendarId, deletedAt}]}`.
//! - `CalendarTrash/set` `{accountId, restore: [id], destroy: [id]}` →
//!   `{accountId, restored, notRestored, destroyed, notDestroyed}`
//!   (`destroy` deletes for good).
//!
//! Where the server has no trash bin, both are `accountNotSupportedByMethod`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::method::parse::from_arguments;
use crate::types::{AccountId, CalendarId, UtcDate};

/// What a trash bin item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrashKind {
    /// A deleted calendar (`c…`).
    Calendar,
    /// A deleted event or task (`e…`).
    Event,
}

/// The id of a trash bin item: `c` or `e` and 16 lowercase hex digits
/// (a number from the item's location; never the location itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrashId {
    /// Calendar or event.
    pub kind: TrashKind,
    /// The number.
    pub number: u64,
}

impl fmt::Display for TrashId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.kind {
            TrashKind::Calendar => 'c',
            TrashKind::Event => 'e',
        };
        write!(f, "{prefix}{:016x}", self.number)
    }
}

impl FromStr for TrashId {
    type Err = MethodError;

    fn from_str(text: &str) -> Result<Self, MethodError> {
        let invalid = || MethodError::invalid_arguments("not a trash bin id");
        let kind = match text.as_bytes().first() {
            Some(b'c') => TrashKind::Calendar,
            Some(b'e') => TrashKind::Event,
            _ => return Err(invalid()),
        };
        let hex = text.get(1..).ok_or_else(invalid)?;
        if hex.len() != 16 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(invalid());
        }
        let number = u64::from_str_radix(hex, 16).map_err(|_| invalid())?;
        Ok(Self { kind, number })
    }
}

impl Serialize for TrashId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TrashId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|_| serde::de::Error::custom("not a trash bin id"))
    }
}

/// `CalendarTrash/get` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarTrashGetArguments {
    /// The account.
    pub account_id: AccountId,
}

impl CalendarTrashGetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`.
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        from_arguments(arguments)
    }
}

/// A calendar in the trash bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedCalendar {
    /// Its id.
    pub id: TrashId,
    /// Its name.
    pub name: Option<String>,
    /// Its colour.
    pub color: Option<String>,
    /// When it was deleted.
    pub deleted_at: Option<UtcDate>,
}

/// An event or task in the trash bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedEvent {
    /// Its id.
    pub id: TrashId,
    /// Its title.
    pub title: Option<String>,
    /// Its first start.
    pub utc_start: Option<UtcDate>,
    /// The calendar it was in, if that one is still there.
    pub calendar_id: Option<CalendarId>,
    /// When it was deleted.
    pub deleted_at: Option<UtcDate>,
}

/// `CalendarTrash/get` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarTrashGetResponse {
    /// The account.
    pub account_id: AccountId,
    /// How long items stay before the server deletes them, in seconds.
    pub retention_seconds: Option<u64>,
    /// Deleted calendars.
    pub calendars: Vec<TrashedCalendar>,
    /// Deleted events and tasks.
    pub events: Vec<TrashedEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawSet {
    account_id: AccountId,
    #[serde(default)]
    restore: Vec<TrashId>,
    #[serde(default)]
    destroy: Vec<TrashId>,
}

/// `CalendarTrash/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarTrashSetArguments {
    /// The account.
    pub account_id: AccountId,
    /// Items to restore (each once).
    pub restore: Vec<TrashId>,
    /// Items to delete for good (each once).
    pub destroy: Vec<TrashId>,
}

impl CalendarTrashSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` (also for an id in both lists);
    /// `requestTooLarge` past `max_objects`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let raw: RawSet = from_arguments(arguments)?;
        if raw.restore.len().saturating_add(raw.destroy.len()) > max_objects {
            return Err(MethodError::request_too_large("too many ids"));
        }
        let mut restore = raw.restore;
        let mut destroy = raw.destroy;
        restore.sort_unstable();
        restore.dedup();
        destroy.sort_unstable();
        destroy.dedup();
        if restore.iter().any(|id| destroy.contains(id)) {
            return Err(MethodError::invalid_arguments(
                "an id is in both restore and destroy",
            ));
        }
        Ok(Self {
            account_id: raw.account_id,
            restore,
            destroy,
        })
    }
}

/// `CalendarTrash/set` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarTrashSetResponse {
    /// The account.
    pub account_id: AccountId,
    /// Restored items.
    pub restored: Vec<TrashId>,
    /// Items not restored, and why.
    pub not_restored: BTreeMap<TrashId, SetError>,
    /// Items deleted for good.
    pub destroyed: Vec<TrashId>,
    /// Items not deleted, and why.
    pub not_destroyed: BTreeMap<TrashId, SetError>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn ids() {
        let id: TrashId = "c00000000000000ff".parse().unwrap();
        assert_eq!(id.kind, TrashKind::Calendar);
        assert_eq!(id.number, 255);
        assert_eq!(id.to_string(), "c00000000000000ff");
        for bad in [
            "",
            "c",
            "x00000000000000ff",
            "c00000000000000FF",
            "c00000000000000f",
            "c00000000000000fff",
            "e-0000000000000ff",
        ] {
            assert!(bad.parse::<TrashId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn set_arguments() {
        let parsed = CalendarTrashSetArguments::parse(
            map(
                json!({"accountId": "a", "restore": ["e0000000000000001", "e0000000000000001"],
                       "destroy": ["c0000000000000002"]}),
            ),
            10,
        )
        .unwrap();
        assert_eq!(parsed.restore.len(), 1);
        assert_eq!(parsed.destroy.len(), 1);
        for bad in [
            json!({"accountId": "a", "restore": ["e0000000000000001"], "destroy": ["e0000000000000001"]}),
            json!({"accountId": "a", "restore": ["nope"]}),
            json!({"accountId": "a", "other": 1}),
        ] {
            assert!(
                CalendarTrashSetArguments::parse(map(bad.clone()), 10).is_err(),
                "{bad}"
            );
        }
        assert!(
            CalendarTrashSetArguments::parse(
                map(
                    json!({"accountId": "a", "restore": ["e0000000000000001", "e0000000000000002"]})
                ),
                1
            )
            .is_err()
        );
        assert!(CalendarTrashGetArguments::parse(map(json!({"accountId": "a", "x": 1}))).is_err());
    }

    #[test]
    fn response_shape() {
        let response = CalendarTrashGetResponse {
            account_id: "a".parse().unwrap(),
            retention_seconds: Some(2_592_000),
            calendars: vec![TrashedCalendar {
                id: "c0000000000000001".parse().unwrap(),
                name: Some("Old".to_owned()),
                color: None,
                deleted_at: UtcDate::from_unix_timestamp(0).ok(),
            }],
            events: vec![TrashedEvent {
                id: "e0000000000000002".parse().unwrap(),
                title: Some("Lunch".to_owned()),
                utc_start: None,
                calendar_id: Some("b".parse().unwrap()),
                deleted_at: None,
            }],
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "a", "retentionSeconds": 2_592_000,
                "calendars": [{"id": "c0000000000000001", "name": "Old", "color": null,
                               "deletedAt": "1970-01-01T00:00:00Z"}],
                "events": [{"id": "e0000000000000002", "title": "Lunch", "utcStart": null,
                            "calendarId": "b", "deletedAt": null}]
            })
        );
    }
}
