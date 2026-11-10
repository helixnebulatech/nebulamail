// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar (draft-ietf-jmap-calendars §4), checked against Stalwart.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::bounded::{JsonLimits, check_object};
use super::map_patch::{MapMembers, MapPatch};
use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::parse::{PropertyErrors, take_argument};
use crate::method::patch::{Patch, Target};
use crate::method::{PatchObject, SetArguments};
use crate::references::IdOrRef;
use crate::types::{CalendarId, PrincipalId, TimeZoneId, UnsignedInt};

/// What a user may do with a calendar and its events
/// (draft-ietf-jmap-calendars §4, `CalendarRights`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire format is exactly these eight booleans"
)]
pub struct CalendarRights {
    /// May see the free/busy time of its events.
    pub may_read_free_busy: bool,
    /// May read the events.
    pub may_read_items: bool,
    /// May change any event.
    pub may_write_all: bool,
    /// May change events the user owns.
    pub may_write_own: bool,
    /// May change per-user properties (alerts, keywords…).
    pub may_update_private: bool,
    /// May reply to invitations as a participant.
    #[serde(rename = "mayRSVP")]
    pub may_rsvp: bool,
    /// May share it.
    pub may_share: bool,
    /// May delete it.
    pub may_delete: bool,
}

impl CalendarRights {
    /// Every right (the owner's).
    pub const ALL: Self = Self {
        may_read_free_busy: true,
        may_read_items: true,
        may_write_all: true,
        may_write_own: true,
        may_update_private: true,
        may_rsvp: true,
        may_share: true,
        may_delete: true,
    };
}

string_enum! {
    /// `includeInAvailability`: which events count as busy time.
    pub enum IncludeInAvailability ("includeInAvailability value") {
        /// Every event.
        All = "all",
        /// Only events the user attends.
        Attending = "attending",
        /// None.
        None = "none",
    }
}

string_enum! {
    /// Calendar properties (for `properties`).
    pub enum CalendarProperty ("Calendar property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `description`.
        Description = "description",
        /// `color`.
        Color = "color",
        /// `sortOrder`.
        SortOrder = "sortOrder",
        /// `isSubscribed`.
        IsSubscribed = "isSubscribed",
        /// `isVisible`.
        IsVisible = "isVisible",
        /// `isDefault`.
        IsDefault = "isDefault",
        /// `includeInAvailability`.
        IncludeInAvailability = "includeInAvailability",
        /// `defaultAlertsWithTime`.
        DefaultAlertsWithTime = "defaultAlertsWithTime",
        /// `defaultAlertsWithoutTime`.
        DefaultAlertsWithoutTime = "defaultAlertsWithoutTime",
        /// `timeZone`.
        TimeZone = "timeZone",
        /// `shareWith`.
        ShareWith = "shareWith",
        /// `myRights`.
        MyRights = "myRights",
        /// [`COMPONENTS_PROPERTY`] (the bridge's vendor property).
        Components = "https://helixnebula.tech/jmap/bridge/components",
        /// [`IS_PUBLISHED_PROPERTY`] (vendor).
        IsPublished = "https://helixnebula.tech/jmap/bridge/isPublished",
        /// [`PUBLIC_URL_PROPERTY`] (vendor).
        PublicUrl = "https://helixnebula.tech/jmap/bridge/publicUrl",
        /// [`SUBSCRIPTION_URL_PROPERTY`] (vendor).
        SubscriptionUrl = "https://helixnebula.tech/jmap/bridge/subscriptionUrl",
    }
}

/// Vendor property (Nextcloud's `oc-calendar-publishing`)
/// whether the calendar is published at a public link.
/// Only on calendars the user may publish (absent elsewhere); the only
/// one of the three that can be changed.
pub const IS_PUBLISHED_PROPERTY: &str = "https://helixnebula.tech/jmap/bridge/isPublished";

/// Vendor property, server-set: the public link of a published calendar
/// (`null` while not published; absent where it can't be). Only shown:
/// nobody fetches it.
pub const PUBLIC_URL_PROPERTY: &str = "https://helixnebula.tech/jmap/bridge/publicUrl";

/// Vendor property: where a webcal subscription comes from (only on
/// subscriptions, which are read-only; `null` if the server's value is
/// not a URL the bridge shows). Set only on create, to subscribe
/// (`https`, `webcal` or `webcals`, no credentials): the DAV server
/// fetches it, never the bridge.
pub const SUBSCRIPTION_URL_PROPERTY: &str = "https://helixnebula.tech/jmap/bridge/subscriptionUrl";

/// Longest subscription URL accepted, in bytes.
pub const MAX_SUBSCRIPTION_URL: usize = 2048;

/// The bridge's vendor property of a Calendar (RFC 8620 §1.8, under the
/// capability `https://helixnebula.tech/jmap/bridge`, only when the
/// request has it in `using`): which kinds of items the calendar takes,
/// from CalDAV's `supported-calendar-component-set`. `["VEVENT","VTODO"]`
/// takes events and tasks; `["VTODO"]` is a task list. On create, the
/// default is both.
pub const COMPONENTS_PROPERTY: &str = "https://helixnebula.tech/jmap/bridge/components";

string_enum! {
    /// An item a calendar takes ([`COMPONENTS_PROPERTY`]), by its
    /// iCalendar component name.
    pub enum CalendarComponent ("calendar component") {
        /// `VEVENT`: events.
        Event = "VEVENT",
        /// `VTODO`: tasks.
        Todo = "VTODO",
    }
}

/// One JSCalendar `Alert`, kept as JSON (bounded on input).
pub type Alert = Map<String, Value>;

/// Default alerts: alert id → `Alert`.
pub type DefaultAlerts = BTreeMap<String, Alert>;

/// Most default alerts per calendar.
const MAX_DEFAULT_ALERTS: usize = 64;

fn alert_is_bounded(alert: &Alert) -> bool {
    check_object(alert, &JsonLimits::SMALL).is_ok()
}

/// A Calendar as returned by `Calendar/get` (and in `/set` `created`).
/// `None`: not sent; `Some(None)`: `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<CalendarId>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    /// `color` (a CSS color).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Option<String>>,
    /// `sortOrder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<UnsignedInt>,
    /// `isSubscribed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// `isVisible`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
    /// `isDefault`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// `includeInAvailability`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_in_availability: Option<IncludeInAvailability>,
    /// `defaultAlertsWithTime`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_alerts_with_time: Option<Option<DefaultAlerts>>,
    /// `defaultAlertsWithoutTime`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_alerts_without_time: Option<Option<DefaultAlerts>>,
    /// `timeZone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<Option<TimeZoneId>>,
    /// `shareWith`: who else may use it (`null` in other people's
    /// calendars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_with: Option<Option<BTreeMap<PrincipalId, CalendarRights>>>,
    /// `myRights`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<CalendarRights>,
    /// [`COMPONENTS_PROPERTY`] (vendor).
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/components",
        skip_serializing_if = "Option::is_none"
    )]
    pub components: Option<Vec<CalendarComponent>>,
    /// [`IS_PUBLISHED_PROPERTY`] (vendor).
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/isPublished",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_published: Option<bool>,
    /// [`PUBLIC_URL_PROPERTY`] (vendor).
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/publicUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub public_url: Option<Option<String>>,
    /// [`SUBSCRIPTION_URL_PROPERTY`] (vendor).
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/subscriptionUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub subscription_url: Option<Option<String>>,
}

/// A subscription URL as typed: `https`, `webcal` or `webcals`, no
/// spaces or controls, at most [`MAX_SUBSCRIPTION_URL`] bytes (the rest
/// is checked where it is used).
#[expect(
    clippy::option_option,
    reason = "JMAP distinguishes missing from null, which is exactly this type"
)]
fn check_subscription_url(
    errors: &mut PropertyErrors,
    key: &str,
    url: Option<Option<String>>,
) -> Option<Option<String>> {
    match url? {
        None => Some(None),
        Some(text) => {
            let lower = text.to_ascii_lowercase();
            let scheme_ok = ["https://", "webcal://", "webcals://"]
                .iter()
                .any(|scheme| lower.starts_with(scheme));
            if !scheme_ok
                || text.len() > MAX_SUBSCRIPTION_URL
                || text.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                errors.invalid_because(
                    key,
                    "must be an https, webcal or webcals URL without spaces",
                );
                return None;
            }
            Some(Some(text))
        }
    }
}

/// Components of a create: at least one, each once.
fn check_components(
    errors: &mut PropertyErrors,
    key: &str,
    components: Option<Vec<CalendarComponent>>,
) -> Option<Vec<CalendarComponent>> {
    let list = components?;
    let unique = list
        .iter()
        .enumerate()
        .all(|(index, c)| !list.iter().skip(index.saturating_add(1)).any(|o| o == c));
    if list.is_empty() || !unique {
        errors.invalid_because(key, "must list VEVENT and/or VTODO, each once");
        return None;
    }
    Some(list)
}

/// Properties only the server sets.
const SERVER_SET: &[&str] = &["id", "isDefault", "myRights", PUBLIC_URL_PROPERTY];

/// Longest calendar name, in bytes (the same as mailboxes).
const MAX_NAME: usize = 255;
/// Longest color value.
const MAX_COLOR: usize = 64;
/// Longest description.
const MAX_DESCRIPTION: usize = 64 << 10;

pub(crate) fn check_name(
    errors: &mut PropertyErrors,
    key: &str,
    name: Option<String>,
) -> Option<String> {
    let name = name?;
    if name.trim().is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        errors.invalid_because(
            key,
            "must not be empty, longer than 255 bytes or have control characters",
        );
        return None;
    }
    Some(name)
}

#[expect(
    clippy::option_option,
    reason = "JMAP distinguishes missing from null, which is exactly this type"
)]
pub(crate) fn check_description(
    errors: &mut PropertyErrors,
    key: &str,
    description: Option<Option<String>>,
) -> Option<Option<String>> {
    match description {
        Some(Some(text)) if text.len() > MAX_DESCRIPTION => {
            errors.invalid_because(key, "too long");
            None
        }
        other => other,
    }
}

/// A CSS color: printable ASCII, 1 to 64 characters.
#[expect(
    clippy::option_option,
    reason = "JMAP distinguishes missing from null, which is exactly this type"
)]
fn check_color(
    errors: &mut PropertyErrors,
    key: &str,
    color: Option<Option<String>>,
) -> Option<Option<String>> {
    match color {
        Some(Some(text))
            if text.is_empty()
                || text.len() > MAX_COLOR
                || !text.bytes().all(|b| b.is_ascii_graphic() || b == b' ') =>
        {
            errors.invalid_because(key, "not a CSS color");
            None
        }
        other => other,
    }
}

#[expect(
    clippy::option_option,
    reason = "JMAP distinguishes missing from null, which is exactly this type"
)]
fn check_alerts(
    errors: &mut PropertyErrors,
    key: &str,
    alerts: Option<Option<DefaultAlerts>>,
) -> Option<Option<DefaultAlerts>> {
    match alerts {
        Some(Some(map)) => {
            let valid = map.len() <= MAX_DEFAULT_ALERTS && map.values().all(alert_is_bounded);
            if valid {
                Some(Some(map))
            } else {
                errors.invalid_because(key, "must map alert ids to Alert objects, within limits");
                None
            }
        }
        other => other,
    }
}

/// A calendar to create (`Calendar/set` create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarCreate {
    /// The name (not blank, no control characters).
    pub name: String,
    /// A description.
    pub description: Option<String>,
    /// A CSS color.
    pub color: Option<String>,
    /// Sort order (default 0).
    pub sort_order: UnsignedInt,
    /// Whether subscribed; `None` lets the server choose.
    pub is_subscribed: Option<bool>,
    /// Whether its events are shown; `None`: the default (`true`).
    pub is_visible: Option<bool>,
    /// Which events count as busy; `None`: the default (`all`).
    pub include_in_availability: Option<IncludeInAvailability>,
    /// Default alerts for events with a time.
    pub default_alerts_with_time: Option<DefaultAlerts>,
    /// Default alerts for all-day events.
    pub default_alerts_without_time: Option<DefaultAlerts>,
    /// The calendar's time zone.
    pub time_zone: Option<TimeZoneId>,
    /// Who else may use it.
    pub share_with: Option<BTreeMap<PrincipalId, CalendarRights>>,
    /// What it takes ([`COMPONENTS_PROPERTY`]); `None`: the server's
    /// default.
    pub components: Option<Vec<CalendarComponent>>,
    /// Subscribe to this URL ([`SUBSCRIPTION_URL_PROPERTY`]) instead of
    /// creating an empty calendar.
    pub subscription_url: Option<String>,
    /// Publish it right away ([`IS_PUBLISHED_PROPERTY`]).
    pub is_published: Option<bool>,
}

impl CalendarCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every invalid, unknown or server-set
    /// property.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = Calendar::default();
        for (key, value) in object {
            parse_member(&mut errors, &mut out, &key, value);
        }
        let name = errors.required("name", out.name);
        errors.finish()?;
        Ok(Self {
            name: name.unwrap_or_default(),
            description: out.description.flatten(),
            color: out.color.flatten(),
            sort_order: out.sort_order.unwrap_or_default(),
            is_subscribed: out.is_subscribed,
            is_visible: out.is_visible,
            include_in_availability: out.include_in_availability,
            default_alerts_with_time: out.default_alerts_with_time.flatten(),
            default_alerts_without_time: out.default_alerts_without_time.flatten(),
            time_zone: out.time_zone.flatten(),
            share_with: out.share_with.flatten(),
            components: out.components,
            subscription_url: out.subscription_url.flatten(),
            is_published: out.is_published,
        })
    }
}

/// Parses one client-set member into `out`; server-set and unknown
/// members are recorded as invalid.
fn parse_member(errors: &mut PropertyErrors, out: &mut Calendar, key: &str, value: Value) {
    match key {
        "name" => {
            let name = errors.parse(key, value);
            out.name = check_name(errors, key, name);
        }
        "description" => {
            let description = errors.parse(key, value);
            out.description = check_description(errors, key, description);
        }
        "color" => {
            let color = errors.parse(key, value);
            out.color = check_color(errors, key, color);
        }
        "sortOrder" => {
            out.sort_order = errors
                .parse::<Option<UnsignedInt>>(key, value)
                .map(Option::unwrap_or_default);
        }
        // Booleans without a meaningful null.
        "isSubscribed" => out.is_subscribed = errors.parse(key, value),
        "isVisible" => out.is_visible = errors.parse(key, value),
        "includeInAvailability" => out.include_in_availability = errors.parse(key, value),
        "defaultAlertsWithTime" => {
            let alerts = errors.parse(key, value);
            out.default_alerts_with_time = check_alerts(errors, key, alerts);
        }
        "defaultAlertsWithoutTime" => {
            let alerts = errors.parse(key, value);
            out.default_alerts_without_time = check_alerts(errors, key, alerts);
        }
        "timeZone" => out.time_zone = errors.parse(key, value),
        "shareWith" => out.share_with = errors.parse(key, value),
        COMPONENTS_PROPERTY => {
            let components = errors.parse(key, value);
            out.components = check_components(errors, key, components);
        }
        IS_PUBLISHED_PROPERTY => out.is_published = errors.parse(key, value),
        SUBSCRIPTION_URL_PROPERTY => {
            let url = errors.parse(key, value);
            out.subscription_url = check_subscription_url(errors, key, url);
        }
        _ => errors.invalid(key),
    }
}

/// A `Calendar/set` update. `None` fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarPatch {
    /// New name.
    pub name: Option<String>,
    /// New description (`Some(None)`: remove it).
    pub description: Option<Option<String>>,
    /// New color (`Some(None)`: remove it).
    pub color: Option<Option<String>>,
    /// New sort order (`null` resets to 0).
    pub sort_order: Option<UnsignedInt>,
    /// Subscribe or unsubscribe.
    pub is_subscribed: Option<bool>,
    /// Show or hide its events.
    pub is_visible: Option<bool>,
    /// Which events count as busy.
    pub include_in_availability: Option<IncludeInAvailability>,
    /// Default alerts with a time (whole map or one alert).
    pub default_alerts_with_time: Option<MapPatch<String, Alert>>,
    /// Default alerts for all-day events.
    pub default_alerts_without_time: Option<MapPatch<String, Alert>>,
    /// New time zone (`Some(None)`: none).
    pub time_zone: Option<Option<TimeZoneId>>,
    /// Sharing changes (whole map or `shareWith/<principal>`).
    pub share_with: Option<MapPatch<PrincipalId, CalendarRights>>,
    /// Publish or stop publishing ([`IS_PUBLISHED_PROPERTY`]).
    pub is_published: Option<bool>,
}

impl CalendarPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad paths; `invalidProperties` for unknown,
    /// server-set or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut whole = Calendar::default();
        let mut share_members = MapMembers::<PrincipalId, CalendarRights>::default();
        let mut with_time = MapMembers::<String, Alert>::default();
        let mut without_time = MapMembers::<String, Alert>::default();
        let mut share_whole = None;
        let mut with_time_whole = None;
        let mut without_time_whole = None;
        for Patch { key, path, value } in patch.patches()? {
            match Target::of(&path) {
                Target::Property("shareWith") => {
                    share_whole = errors.parse(&key, value).map(MapPatch::Replace);
                }
                Target::Property(
                    property @ ("defaultAlertsWithTime" | "defaultAlertsWithoutTime"),
                ) => {
                    let alerts = errors.parse(&key, value);
                    let checked = check_alerts(&mut errors, &key, alerts).map(MapPatch::Replace);
                    if property == "defaultAlertsWithTime" {
                        with_time_whole = checked;
                    } else {
                        without_time_whole = checked;
                    }
                }
                Target::Property(property) if SERVER_SET.contains(&property) => {
                    errors.invalid_because(&key, "server-set");
                }
                // CalDAV's `supported-calendar-component-set` is set once,
                // by MKCALENDAR (RFC 4791 §5.2.3).
                Target::Property(COMPONENTS_PROPERTY | SUBSCRIPTION_URL_PROPERTY) => {
                    errors.invalid_because(&key, "can't be changed after creation");
                }
                Target::Property(property) => {
                    parse_member(&mut errors, &mut whole, property, value);
                }
                Target::Member("shareWith", member) => {
                    share_members.record(&mut errors, &key, member, value)?;
                }
                Target::Member(
                    property @ ("defaultAlertsWithTime" | "defaultAlertsWithoutTime"),
                    member,
                ) => {
                    let bounded = match &value {
                        Value::Null => true,
                        Value::Object(alert) => alert_is_bounded(alert),
                        _ => false,
                    };
                    if !bounded {
                        errors.invalid_because(&key, "must be an Alert (within limits) or null");
                    } else if property == "defaultAlertsWithTime" {
                        with_time.record(&mut errors, &key, member, value)?;
                    } else {
                        without_time.record(&mut errors, &key, member, value)?;
                    }
                }
                Target::Member(property, _) | Target::Deeper(property) => {
                    if CalendarProperty::ALL.iter().any(|p| p.as_str() == property) {
                        return Err(SetError::invalid_patch(format!(
                            "{key:?}: cannot patch inside {property}"
                        )));
                    }
                    errors.invalid(key);
                }
            }
        }
        errors.finish()?;
        Ok(Self {
            name: whole.name,
            description: whole.description,
            color: whole.color,
            sort_order: whole.sort_order,
            is_subscribed: whole.is_subscribed,
            is_visible: whole.is_visible,
            include_in_availability: whole.include_in_availability,
            default_alerts_with_time: with_time_whole.or_else(|| with_time.into_patch()),
            default_alerts_without_time: without_time_whole.or_else(|| without_time.into_patch()),
            time_zone: whole.time_zone,
            share_with: share_whole.or_else(|| share_members.into_patch()),
            is_published: whole.is_published,
        })
    }
}

/// `Calendar/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarSetArguments {
    /// The standard arguments (creates: [`CalendarCreate::parse`];
    /// updates: [`CalendarPatch::parse`]).
    pub set: SetArguments<CalendarId>,
    /// Destroy calendars that have events (and the events), instead of
    /// failing with `calendarHasEvent`.
    pub on_destroy_remove_events: bool,
    /// After everything succeeds, make this the default calendar.
    pub on_success_set_is_default: Option<IdOrRef<CalendarId>>,
}

impl CalendarSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let on_destroy_remove_events =
            take_argument(&mut arguments, "onDestroyRemoveEvents")?.unwrap_or(false);
        let on_success_set_is_default = take_argument(&mut arguments, "onSuccessSetIsDefault")?;
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            on_destroy_remove_events,
            on_success_set_is_default,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// Stalwart 1.0's `Calendar/get` (2026-10-27), field by field.
    #[test]
    fn stalwart_get_shape() {
        let calendar = Calendar {
            id: Some("b".parse().unwrap()),
            name: Some("Stalwart Calendar (alice@nebulamail.test)".to_owned()),
            description: Some(None),
            color: Some(None),
            sort_order: Some(UnsignedInt::ZERO),
            is_subscribed: Some(true),
            is_visible: Some(true),
            is_default: Some(true),
            include_in_availability: Some(IncludeInAvailability::All),
            default_alerts_with_time: Some(Some(BTreeMap::new())),
            default_alerts_without_time: Some(Some(BTreeMap::new())),
            time_zone: Some(None),
            share_with: Some(Some(BTreeMap::new())),
            my_rights: Some(CalendarRights::ALL),
            components: None,
            is_published: None,
            public_url: None,
            subscription_url: None,
        };
        assert_eq!(
            serde_json::to_value(&calendar).unwrap(),
            json!({
                "id": "b",
                "name": "Stalwart Calendar (alice@nebulamail.test)",
                "description": null,
                "color": null,
                "sortOrder": 0,
                "isSubscribed": true,
                "isVisible": true,
                "isDefault": true,
                "includeInAvailability": "all",
                "defaultAlertsWithTime": {},
                "defaultAlertsWithoutTime": {},
                "timeZone": null,
                "shareWith": {},
                "myRights": {
                    "mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": true,
                    "mayWriteOwn": true, "mayUpdatePrivate": true, "mayRSVP": true,
                    "mayShare": true, "mayDelete": true
                }
            })
        );
    }

    /// `createCalendar` in apps/web/src/calendar/api.ts.
    #[test]
    fn create_from_the_web() {
        let created = CalendarCreate::parse(map(json!({
            "color": "#3b82f6", "isSubscribed": true, "isVisible": true, "name": "Trabajo"
        })))
        .unwrap();
        assert_eq!(created.name, "Trabajo");
        assert_eq!(created.color.as_deref(), Some("#3b82f6"));
        assert_eq!(created.is_visible, Some(true));
        assert_eq!(created.sort_order, UnsignedInt::ZERO);

        let full = CalendarCreate::parse(map(json!({
            "name": "x", "description": null, "timeZone": "Europe/Madrid",
            "includeInAvailability": "attending",
            "defaultAlertsWithTime": {"a1": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}}},
            "shareWith": {"p1": {
                "mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": false,
                "mayWriteOwn": false, "mayUpdatePrivate": false, "mayRSVP": false,
                "mayShare": false, "mayDelete": false
            }}
        })))
        .unwrap();
        assert_eq!(full.time_zone.unwrap().as_str(), "Europe/Madrid");
        assert_eq!(full.share_with.unwrap().len(), 1);
        assert_eq!(full.components, None);
    }

    /// The bridge's vendor property: a task list.
    #[test]
    fn components_vendor_property() {
        let list = CalendarCreate::parse(map(json!({
            "name": "Recados", COMPONENTS_PROPERTY: ["VTODO"]
        })))
        .unwrap();
        assert_eq!(list.components, Some(vec![CalendarComponent::Todo]));
        for bad in [
            json!([]),
            json!(["VTODO", "VTODO"]),
            json!(["VJOURNAL"]),
            json!("VTODO"),
        ] {
            let error = CalendarCreate::parse(map(json!({"name": "x", COMPONENTS_PROPERTY: bad})))
                .unwrap_err();
            assert_eq!(error.properties(), [COMPONENTS_PROPERTY]);
        }
        let patch: PatchObject =
            serde_json::from_value(json!({COMPONENTS_PROPERTY: ["VEVENT"]})).unwrap();
        let error = CalendarPatch::parse(&patch).unwrap_err();
        assert_eq!(error.properties(), [COMPONENTS_PROPERTY]);
        let calendar = Calendar {
            components: Some(vec![CalendarComponent::Event, CalendarComponent::Todo]),
            ..Calendar::default()
        };
        assert_eq!(
            serde_json::to_value(&calendar).unwrap(),
            json!({"https://helixnebula.tech/jmap/bridge/components": ["VEVENT", "VTODO"]})
        );
        assert_eq!(
            "https://helixnebula.tech/jmap/bridge/components".parse::<CalendarProperty>(),
            Ok(CalendarProperty::Components)
        );
    }

    #[test]
    fn create_errors_list_every_property() {
        let error = CalendarCreate::parse(map(json!({
            "name": "  ", "id": "c1", "isDefault": true, "myRights": {}, "color": "red\n",
            "timeZone": "Mars/Base", "includeInAvailability": "sometimes",
            "shareWith": {"p1": {"mayRead": true}}, "defaultAlertsWithTime": {"a": 1},
            "isVisible": null, "unknown": 1
        })))
        .unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        let mut properties = error.properties().to_vec();
        properties.sort();
        assert_eq!(
            properties,
            [
                "color",
                "defaultAlertsWithTime",
                "id",
                "includeInAvailability",
                "isDefault",
                "isVisible",
                "myRights",
                "name",
                "shareWith",
                "unknown"
            ]
        );
        // Mars/Base has the shape of a zone id: whether it exists is
        // checked later. A missing name is an error too.
        assert!(CalendarCreate::parse(map(json!({"color": "#fff"}))).is_err());
    }

    /// `updateCalendar` in apps/web/src/calendar/api.ts, and sharing.
    #[test]
    fn patches() {
        let parse = |value: Value| CalendarPatch::parse(&serde_json::from_value(value).unwrap());
        let patch = parse(json!({"isVisible": false, "color": "#ff0000", "name": "Casa"})).unwrap();
        assert_eq!(patch.is_visible, Some(false));
        assert_eq!(patch.color, Some(Some("#ff0000".to_owned())));
        assert_eq!(patch.name.as_deref(), Some("Casa"));

        let share = parse(json!({
            "shareWith/p1": null,
            "shareWith/p2": {
                "mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": true,
                "mayWriteOwn": true, "mayUpdatePrivate": true, "mayRSVP": true,
                "mayShare": false, "mayDelete": false
            },
            "defaultAlertsWithTime/a1": null,
            "sortOrder": null,
            "timeZone": null
        }))
        .unwrap();
        let Some(MapPatch::Change { set, remove }) = share.share_with else {
            panic!("expected a change")
        };
        assert_eq!(set.len(), 1);
        assert_eq!(remove.len(), 1);
        assert!(matches!(
            share.default_alerts_with_time,
            Some(MapPatch::Change { .. })
        ));
        assert_eq!(share.sort_order, Some(UnsignedInt::ZERO));
        assert_eq!(share.time_zone, Some(None));

        let kind = |value: Value| parse(value).unwrap_err().kind();
        assert_eq!(
            kind(json!({"isDefault": true})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"myRights": {}})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"name": null})), SetErrorType::InvalidProperties);
        assert_eq!(
            kind(json!({"isVisible": null})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"shareWith/a b": null})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"shareWith/p1": {"mayRead": true}})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"name/x": "y"})), SetErrorType::InvalidPatch);
        assert_eq!(
            kind(json!({"shareWith/p1/mayShare": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(
            kind(json!({"shareWith": {}, "shareWith/p1": null})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(kind(json!({"nope": 1})), SetErrorType::InvalidProperties);
    }

    /// `deleteCalendar` in apps/web/src/calendar/api.ts.
    #[test]
    fn set_arguments() {
        let args = CalendarSetArguments::parse(
            map(json!({"accountId": "c", "destroy": ["b"], "onDestroyRemoveEvents": true})),
            10,
        )
        .unwrap();
        assert!(args.on_destroy_remove_events);
        assert_eq!(args.on_success_set_is_default, None);
        let args = CalendarSetArguments::parse(
            map(json!({"accountId": "c", "create": {"k": {"name": "x"}}, "onSuccessSetIsDefault": "#k"})),
            10,
        )
        .unwrap();
        assert_eq!(
            args.on_success_set_is_default,
            Some(IdOrRef::Ref("k".parse().unwrap()))
        );
        assert!(
            CalendarSetArguments::parse(
                map(json!({"accountId": "c", "onDestroyRemoveEvents": "yes"})),
                10
            )
            .is_err()
        );
        assert!(
            CalendarSetArguments::parse(map(json!({"accountId": "c", "extra": 1})), 10).is_err()
        );
    }

    /// Nextcloud's extras: subscribe on create, publish
    /// on create or update; the link is server-set.
    #[test]
    fn publishing_and_subscriptions() {
        let created = CalendarCreate::parse(map(json!({
            "name": "Festivos",
            SUBSCRIPTION_URL_PROPERTY: "webcal://example.com/h.ics"
        })))
        .unwrap();
        assert_eq!(
            created.subscription_url.as_deref(),
            Some("webcal://example.com/h.ics")
        );
        let published = CalendarCreate::parse(map(json!({
            "name": "Público", IS_PUBLISHED_PROPERTY: true
        })))
        .unwrap();
        assert_eq!(published.is_published, Some(true));
        for bad in [
            "http://example.com/h.ics",
            "file:///etc/passwd",
            "https://example.com/a b",
            "javascript:alert(1)",
        ] {
            let error = CalendarCreate::parse(map(json!({
                "name": "x", SUBSCRIPTION_URL_PROPERTY: bad
            })))
            .unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_SUBSCRIPTION_URL));
        assert!(
            CalendarCreate::parse(map(json!({"name": "x", SUBSCRIPTION_URL_PROPERTY: long})))
                .is_err()
        );
        assert!(
            CalendarCreate::parse(map(json!({"name": "x", PUBLIC_URL_PROPERTY: "https://x/"})))
                .is_err()
        );

        // A patch key is a JSON pointer: the slashes of the name are `~1`.
        let pointer = |name: &str| name.replace('/', "~1");
        let patch = CalendarPatch::parse(
            &serde_json::from_value(json!({pointer(IS_PUBLISHED_PROPERTY): false})).unwrap(),
        )
        .unwrap();
        assert_eq!(patch.is_published, Some(false));
        for bad in [
            json!({pointer(SUBSCRIPTION_URL_PROPERTY): "https://example.com/x.ics"}),
            json!({pointer(PUBLIC_URL_PROPERTY): null}),
            json!({IS_PUBLISHED_PROPERTY: true}),
        ] {
            assert!(
                CalendarPatch::parse(&serde_json::from_value(bad.clone()).unwrap()).is_err(),
                "{bad}"
            );
        }

        let shown = Calendar {
            is_published: Some(true),
            public_url: Some(Some("https://cloud.example.com/p/X".to_owned())),
            subscription_url: Some(None),
            ..Calendar::default()
        };
        assert_eq!(
            serde_json::to_value(&shown).unwrap(),
            json!({
                IS_PUBLISHED_PROPERTY: true,
                PUBLIC_URL_PROPERTY: "https://cloud.example.com/p/X",
                SUBSCRIPTION_URL_PROPERTY: null
            })
        );
    }
}
