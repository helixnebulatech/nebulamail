// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `PushSubscription` (RFC 8620 §7.2) and the `PushVerification` push
//! object (§7.2.2). A subscription belongs to the credentials, not to an
//! account: the methods take no `accountId` (one is ignored if sent) and
//! answer no `state`. `url` and `keys` can be set at creation only and are
//! never returned (RFC 8620 §7.2.1).

use std::collections::BTreeMap;

use serde::ser::SerializeMap as _;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::parse::from_arguments;
use crate::method::{Filter, PatchObject};
use crate::object::email::{EmailFilterCondition, EmailProperty};
use crate::references::IdOrRef;
use crate::types::serde_util::{empty_map_as_null, empty_vec_as_null, null_as_default};
use crate::types::{AccountId, CreationId, DataType, PushSubscriptionId, UtcDate};

/// Longest `deviceClientId` (Stalwart: 255).
pub(crate) const MAX_DEVICE_CLIENT_ID: usize = 255;
/// Longest `url` accepted here (the bridge's endpoint rules are stricter).
pub(crate) const MAX_URL: usize = 1024;
/// Most entries in `types`.
pub(crate) const MAX_TYPES: usize = 64;
/// Most accounts in `emailPush`.
pub(crate) const MAX_EMAIL_PUSH_ACCOUNTS: usize = 16;
/// Most entries in an `EmailPushConfig`'s `properties`.
pub(crate) const MAX_EMAIL_PUSH_PROPERTIES: usize = 64;

string_enum! {
    /// `PushSubscription` properties (for `properties`).
    pub enum PushSubscriptionProperty ("PushSubscription property") {
        /// `id`.
        Id = "id",
        /// `deviceClientId`.
        DeviceClientId = "deviceClientId",
        /// `url` (write-only).
        Url = "url",
        /// `keys` (write-only).
        Keys = "keys",
        /// `verificationCode`.
        VerificationCode = "verificationCode",
        /// `expires`.
        Expires = "expires",
        /// `types`.
        Types = "types",
        /// `emailPush` (Stalwart 1.0, [`crate::Capability::EmailPush`]).
        EmailPush = "emailPush",
    }
}

string_enum! {
    /// The Web Push `Urgency` (RFC 8030 §5.3) of an [`EmailPushConfig`].
    pub enum PushUrgency ("urgency") {
        /// `very-low`.
        VeryLow = "very-low",
        /// `low`.
        Low = "low",
        /// `normal` (the default).
        Normal = "normal",
        /// `high`.
        High = "high",
    }
}

/// One account's entry of `emailPush` (Stalwart 1.0): which `Email`
/// properties of each delivered email go in the push, for which emails (an
/// `Email/query` filter) and with which `Urgency`. Checked here for shape
/// only; the server says which properties and conditions it supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailPushConfig {
    /// The properties, in the order given (what doesn't fit is left out
    /// from the end).
    pub properties: Vec<EmailProperty>,
    /// `None`: every delivered email.
    pub filter: Option<Filter<EmailFilterCondition>>,
    /// `normal` if not given.
    pub urgency: PushUrgency,
}

/// `emailPush`: account id → what to push of its delivered mail.
pub type EmailPushMap = BTreeMap<AccountId, EmailPushConfig>;

fn email_push_error(description: &str) -> SetError {
    invalid("emailPush", description)
}

impl EmailPushConfig {
    /// Parses one entry of `emailPush`: `{properties, filter, urgency}`,
    /// each optional, nothing else.
    ///
    /// # Errors
    /// `invalidProperties` naming `emailPush`.
    pub fn parse(value: &Value) -> Result<Self, SetError> {
        let Value::Object(map) = value else {
            return Err(email_push_error("EmailPushConfig must be an object"));
        };
        let mut config = Self {
            properties: Vec::new(),
            filter: None,
            urgency: PushUrgency::Normal,
        };
        for (key, value) in map {
            match key.as_str() {
                "properties" => {
                    let Value::Array(items) = value else {
                        return Err(email_push_error("properties must be an array"));
                    };
                    if items.len() > MAX_EMAIL_PUSH_PROPERTIES {
                        return Err(email_push_error("too many properties"));
                    }
                    for item in items {
                        let property = item
                            .as_str()
                            .and_then(|name| name.parse::<EmailProperty>().ok())
                            .ok_or_else(|| email_push_error("unknown Email property"))?;
                        if !config.properties.contains(&property) {
                            config.properties.push(property);
                        }
                    }
                }
                "filter" => {
                    config.filter = match value {
                        Value::Null => None,
                        value => Some(
                            Filter::parse(value.clone())
                                .map_err(|_| email_push_error("invalid filter"))?,
                        ),
                    };
                }
                "urgency" => {
                    config.urgency = value
                        .as_str()
                        .and_then(|text| text.parse().ok())
                        .ok_or_else(|| email_push_error("invalid urgency"))?;
                }
                _ => return Err(email_push_error("unknown EmailPushConfig property")),
            }
        }
        Ok(config)
    }

    /// Parses `emailPush`: `null` (none) or account id → config.
    ///
    /// # Errors
    /// `invalidProperties` naming `emailPush`.
    pub fn parse_map(value: &Value) -> Result<Option<EmailPushMap>, SetError> {
        match value {
            Value::Null => Ok(None),
            Value::Object(map) if map.len() <= MAX_EMAIL_PUSH_ACCOUNTS => {
                let mut out = EmailPushMap::new();
                for (account, config) in map {
                    let account = account
                        .parse::<AccountId>()
                        .map_err(|_| email_push_error("invalid account id"))?;
                    out.insert(account, Self::parse(config)?);
                }
                Ok(Some(out))
            }
            _ => Err(email_push_error("must be an object or null")),
        }
    }
}

/// Arguments of `PushSubscription/get`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushSubscriptionGetArguments {
    /// Not used by this method (ignored if sent).
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// Ids to fetch; `None`: all.
    #[serde(default)]
    pub ids: Option<Vec<PushSubscriptionId>>,
    /// Properties to return; `None`: all readable ones.
    #[serde(default)]
    pub properties: Option<Vec<PushSubscriptionProperty>>,
}

impl PushSubscriptionGetArguments {
    /// Parses the arguments and checks `maxObjectsInGet`.
    ///
    /// # Errors
    /// `invalidArguments`, `requestTooLarge`, or `forbidden` when `url`
    /// or `keys` are asked for (as Stalwart: they are not readable).
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let mut parsed: Self = from_arguments(arguments)?;
        if parsed
            .ids
            .as_ref()
            .is_some_and(|ids| ids.len() > max_objects)
        {
            return Err(MethodError::request_too_large(format!(
                "more than {max_objects} ids"
            )));
        }
        if parsed.properties.as_ref().is_some_and(|list| {
            list.iter().any(|p| {
                matches!(
                    p,
                    PushSubscriptionProperty::Url | PushSubscriptionProperty::Keys
                )
            })
        }) {
            return Err(MethodError::new(crate::error::MethodErrorType::Forbidden)
                .with_description("url and keys are not readable"));
        }
        // RFC 8620 §5.1: a repeated id is answered once.
        if let Some(ids) = &mut parsed.ids {
            let mut seen = std::collections::BTreeSet::new();
            ids.retain(|id| seen.insert(id.clone()));
        }
        Ok(parsed)
    }
}

/// A `PushSubscription` as returned by `/get`. `None`: not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscription {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<PushSubscriptionId>,
    /// `deviceClientId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_client_id: Option<String>,
    /// `verificationCode`: the code, once the client sent it back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_code: Option<Option<String>>,
    /// `expires`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<Option<UtcDate>>,
    /// `types` (`null`: all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Option<Vec<DataType>>>,
    /// `emailPush` as the server keeps it (`null`: none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_push: Option<Option<Value>>,
}

/// Response of `PushSubscription/get` (no `accountId`, no `state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscriptionGetResponse {
    /// The subscriptions found.
    pub list: Vec<PushSubscription>,
    /// Requested ids that don't exist.
    pub not_found: Vec<PushSubscriptionId>,
}

/// Arguments of `PushSubscription/set`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushSubscriptionSetArguments {
    /// Not used by this method (ignored if sent).
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// Must be absent or `null`: subscriptions have no state.
    #[serde(default)]
    pub if_in_state: Option<String>,
    /// Creation id → object.
    #[serde(default, deserialize_with = "null_as_default")]
    pub create: BTreeMap<CreationId, Map<String, Value>>,
    /// Id → patch.
    #[serde(default, deserialize_with = "null_as_default")]
    pub update: BTreeMap<IdOrRef<PushSubscriptionId>, PatchObject>,
    /// Ids to destroy.
    #[serde(default, deserialize_with = "null_as_default")]
    pub destroy: Vec<IdOrRef<PushSubscriptionId>>,
}

impl PushSubscriptionSetArguments {
    /// Parses the arguments and checks `maxObjectsInSet`.
    ///
    /// # Errors
    /// `invalidArguments` (also for `ifInState`), or `requestTooLarge`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.if_in_state.is_some() {
            return Err(MethodError::invalid_arguments(
                "PushSubscription has no state: ifInState is not allowed",
            ));
        }
        let total = parsed
            .create
            .len()
            .saturating_add(parsed.update.len())
            .saturating_add(parsed.destroy.len());
        if total > max_objects {
            return Err(MethodError::request_too_large(format!(
                "more than {max_objects} objects to create, update or destroy"
            )));
        }
        Ok(parsed)
    }
}

/// Response of `PushSubscription/set` (no `accountId`, no states).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscriptionSetResponse {
    /// Creation id → server-set properties (`id`, `expires`, `keys: null`).
    #[serde(serialize_with = "empty_map_as_null")]
    pub created: BTreeMap<CreationId, Value>,
    /// Id → `null` (or what the server changed).
    #[serde(serialize_with = "empty_map_as_null")]
    pub updated: BTreeMap<PushSubscriptionId, Option<Value>>,
    /// Ids destroyed.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub destroyed: Vec<PushSubscriptionId>,
    /// Creation id → why it failed.
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_created: BTreeMap<CreationId, SetError>,
    /// Id → why it failed (keys as sent).
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_updated: BTreeMap<IdOrRef<PushSubscriptionId>, SetError>,
    /// Id → why it failed (keys as sent).
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_destroyed: BTreeMap<IdOrRef<PushSubscriptionId>, SetError>,
}

impl PushSubscriptionSetResponse {
    /// A response with nothing done yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            created: BTreeMap::new(),
            updated: BTreeMap::new(),
            destroyed: Vec::new(),
            not_created: BTreeMap::new(),
            not_updated: BTreeMap::new(),
            not_destroyed: BTreeMap::new(),
        }
    }
}

impl Default for PushSubscriptionSetResponse {
    fn default() -> Self {
        Self::new()
    }
}

/// The browser keys as sent (base64url; checked by the caller, which
/// knows the crypto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushKeys {
    /// `p256dh`.
    pub p256dh: String,
    /// `auth`.
    pub auth: String,
}

/// A subscription to create, with each property checked for shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscriptionCreate {
    /// `deviceClientId` (1 to 255 bytes, no control characters).
    pub device_client_id: String,
    /// `url` (at most 1024 bytes; the caller applies its rules).
    pub url: String,
    /// `keys`; `None`: unencrypted pushes (the bridge refuses those).
    pub keys: Option<PushKeys>,
    /// `expires`; `None`: the server's default.
    pub expires: Option<UtcDate>,
    /// `types`; `None`: every type.
    pub types: Option<Vec<DataType>>,
    /// `emailPush`; `None`: none.
    pub email_push: Option<EmailPushMap>,
}

fn invalid(property: &str, description: &str) -> SetError {
    SetError::invalid_properties([property]).with_description(description)
}

fn text(value: &Value, property: &str, max: usize) -> Result<String, SetError> {
    match value {
        Value::String(text)
            if !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control) =>
        {
            Ok(text.clone())
        }
        _ => Err(invalid(property, "must be a non-empty string")),
    }
}

/// `types`: an array of type names (or `null`).
fn types(value: &Value) -> Result<Option<Vec<DataType>>, SetError> {
    match value {
        Value::Null => Ok(None),
        Value::Array(items) if items.len() <= MAX_TYPES => items
            .iter()
            .map(|item| {
                item.as_str()
                    .and_then(|name| name.parse::<DataType>().ok())
                    .ok_or_else(|| invalid("types", "invalid data type"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err(invalid("types", "must be an array of type names or null")),
    }
}

fn expires(value: &Value) -> Result<Option<UtcDate>, SetError> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => text
            .parse()
            .map(Some)
            .map_err(|_| invalid("expires", "must be a UTCDate")),
        _ => Err(invalid("expires", "must be a UTCDate or null")),
    }
}

impl PushSubscriptionCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` naming the first bad (or server-set) property.
    pub fn parse(object: &Map<String, Value>) -> Result<Self, SetError> {
        let mut device_client_id = None;
        let mut url = None;
        let mut keys = None;
        let mut expiry = None;
        let mut type_list = None;
        let mut email_push = None;
        for (property, value) in object {
            match property.as_str() {
                "deviceClientId" => {
                    device_client_id = Some(text(value, property, MAX_DEVICE_CLIENT_ID)?);
                }
                "url" => url = Some(text(value, property, MAX_URL)?),
                "keys" => {
                    keys = match value {
                        Value::Null => None,
                        Value::Object(map) if map.len() == 2 => {
                            let get = |name: &str| {
                                map.get(name)
                                    .and_then(Value::as_str)
                                    .filter(|v| !v.is_empty() && v.len() <= 256)
                                    .map(str::to_owned)
                                    .ok_or_else(|| invalid("keys", "needs p256dh and auth"))
                            };
                            Some(PushKeys {
                                p256dh: get("p256dh")?,
                                auth: get("auth")?,
                            })
                        }
                        _ => return Err(invalid("keys", "needs p256dh and auth")),
                    };
                }
                "expires" => expiry = expires(value)?,
                "types" => type_list = types(value)?,
                "emailPush" => email_push = EmailPushConfig::parse_map(value)?,
                // Set by the server; `null` is the same as absent.
                "verificationCode" if value.is_null() => {}
                _ => {
                    return Err(invalid(property, "cannot be set"));
                }
            }
        }
        let missing = || {
            SetError::invalid_properties(["deviceClientId", "url"])
                .with_description("deviceClientId and url are required")
        };
        Ok(Self {
            device_client_id: device_client_id.ok_or_else(missing)?,
            url: url.ok_or_else(missing)?,
            keys,
            expires: expiry,
            types: type_list,
            email_push,
        })
    }
}

/// An update of a subscription (`None`: unchanged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushSubscriptionPatch {
    /// The code from the `PushVerification`.
    pub verification_code: Option<String>,
    /// New `expires` (`Some(None)`: the server's default).
    pub expires: Option<Option<UtcDate>>,
    /// New `types` (`Some(None)`: every type).
    pub types: Option<Option<Vec<DataType>>>,
    /// New `emailPush` (`Some(None)`: none). Only as a whole.
    pub email_push: Option<Option<EmailPushMap>>,
}

impl PushSubscriptionPatch {
    /// Parses one patch of `update`: only `verificationCode`, `expires`,
    /// `types` and (as a whole) `emailPush` can change.
    ///
    /// # Errors
    /// `invalidPatch` or `invalidProperties`.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut out = Self::default();
        for item in patch.patches()? {
            match item.path.tokens() {
                [one] if one == "verificationCode" => {
                    out.verification_code = match &item.value {
                        Value::Null => None,
                        value => Some(text(value, "verificationCode", 255)?),
                    };
                }
                [one] if one == "expires" => out.expires = Some(expires(&item.value)?),
                [one] if one == "types" => out.types = Some(types(&item.value)?),
                [one] if one == "emailPush" => {
                    out.email_push = Some(EmailPushConfig::parse_map(&item.value)?);
                }
                _ => return Err(invalid(&item.key, "cannot be changed")),
            }
        }
        Ok(out)
    }
}

/// The `PushVerification` object (RFC 8620 §7.2.2): the first push to a
/// new subscription; the client proves it got it by sending the code back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushVerification {
    /// The subscription.
    pub push_subscription_id: PushSubscriptionId,
    /// The code.
    pub verification_code: String,
}

impl Serialize for PushVerification {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("@type", "PushVerification")?;
        map.serialize_entry("pushSubscriptionId", &self.push_subscription_id)?;
        map.serialize_entry("verificationCode", &self.verification_code)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::{MethodErrorType, SetErrorType};

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// What `lib/web-push.ts` sends.
    #[test]
    fn web_requests() {
        let get = PushSubscriptionGetArguments::parse(map(json!({})), 10).unwrap();
        assert!(get.ids.is_none());
        // RFC 8620 §5.1: repeated ids once.
        let repeated =
            PushSubscriptionGetArguments::parse(map(json!({"ids": ["p1", "p2", "p1"]})), 10)
                .unwrap();
        assert_eq!(repeated.ids.unwrap().len(), 2);
        assert!(PushSubscriptionGetArguments::parse(map(json!({"accountId": "a1"})), 10).is_ok());
        assert_eq!(
            PushSubscriptionGetArguments::parse(map(json!({"properties": ["url"]})), 10)
                .unwrap_err()
                .kind(),
            MethodErrorType::Forbidden
        );
        let set = PushSubscriptionSetArguments::parse(
            map(json!({
                "create": {"push": {
                    "deviceClientId": "nebula-1234",
                    "expires": "2026-11-03T10:00:00Z",
                    "keys": {"auth": "BTBZMqHH6r4Tts7J_aSIgg", "p256dh": "BCVx"},
                    "types": ["EmailDelivery", "CalendarAlert"],
                    "url": "https://fcm.googleapis.com/fcm/send/x"
                }},
                "destroy": ["ps1"]
            })),
            10,
        )
        .unwrap();
        let create = PushSubscriptionCreate::parse(&set.create[&"push".parse().unwrap()]).unwrap();
        assert_eq!(create.device_client_id, "nebula-1234");
        assert_eq!(
            create.types,
            Some(vec![DataType::EmailDelivery, DataType::CalendarAlert])
        );
        assert_eq!(create.keys.unwrap().auth, "BTBZMqHH6r4Tts7J_aSIgg");
        assert_eq!(set.destroy.len(), 1);
        let update = PushSubscriptionSetArguments::parse(
            map(json!({"update": {"ps1": {"verificationCode": "abc"}}})),
            10,
        )
        .unwrap();
        let patch =
            PushSubscriptionPatch::parse(&update.update[&IdOrRef::Id("ps1".parse().unwrap())])
                .unwrap();
        assert_eq!(patch.verification_code.as_deref(), Some("abc"));
        assert!(PushSubscriptionSetArguments::parse(map(json!({"ifInState": "x"})), 10).is_err());
    }

    #[test]
    fn bad_objects() {
        let kind = |value: Value| {
            PushSubscriptionCreate::parse(&map(value))
                .unwrap_err()
                .kind()
        };
        let invalid = SetErrorType::InvalidProperties;
        assert_eq!(kind(json!({"url": "https://x/"})), invalid);
        assert_eq!(kind(json!({"deviceClientId": "d"})), invalid);
        assert_eq!(
            kind(json!({"deviceClientId": "d", "url": "u", "id": "x"})),
            invalid
        );
        assert_eq!(
            kind(json!({"deviceClientId": "d", "url": "u", "verificationCode": "x"})),
            invalid
        );
        assert_eq!(
            kind(json!({"deviceClientId": "d", "url": "u", "types": ["a b"]})),
            invalid
        );
        assert_eq!(kind(json!({"deviceClientId": "d\n", "url": "u"})), invalid);
        assert_eq!(
            kind(json!({"deviceClientId": "d", "url": "u", "keys": {"auth": "a"}})),
            invalid
        );
        let long = "x".repeat(MAX_URL + 1);
        assert_eq!(kind(json!({"deviceClientId": "d", "url": long})), invalid);
        let patch = |value: Value| {
            PushSubscriptionPatch::parse(&PatchObject::new(map(value)))
                .unwrap_err()
                .kind()
        };
        assert_eq!(patch(json!({"url": "https://x/"})), invalid);
        assert_eq!(patch(json!({"keys": null})), invalid);
        assert_eq!(patch(json!({"expires": 5})), invalid);
    }

    /// What `mail/email-push.ts` sends (and Stalwart's checks of shape).
    #[test]
    fn email_push() {
        let create = PushSubscriptionCreate::parse(&map(json!({
            "deviceClientId": "d",
            "url": "https://fcm.googleapis.com/fcm/send/x",
            "types": ["EmailDelivery"],
            "emailPush": {"a1": {
                "filter": {"inMailbox": "m1", "notKeyword": "$seen"},
                "properties": ["id", "from", "subject", "receivedAt", "keywords",
                               "mailboxIds", "id"],
                "urgency": "normal"
            }}
        })))
        .unwrap();
        let push = create.email_push.unwrap();
        let config = &push[&"a1".parse().unwrap()];
        assert_eq!(config.properties.len(), 6);
        assert_eq!(config.urgency, PushUrgency::Normal);
        assert!(matches!(config.filter, Some(Filter::Condition(_))));
        // Defaults: no filter, normal, no properties.
        let config = EmailPushConfig::parse(&json!({})).unwrap();
        assert!(config.filter.is_none() && config.properties.is_empty());
        assert_eq!(
            EmailPushConfig::parse(&json!({"urgency": "very-low"}))
                .unwrap()
                .urgency,
            PushUrgency::VeryLow
        );
        for bad in [
            json!([]),
            json!({"x": {}}),
            json!({"a1": []}),
            json!({"a1": {"other": 1}}),
            json!({"a1": {"properties": ["nope"]}}),
            json!({"a1": {"properties": "id"}}),
            json!({"a1": {"urgency": "urgent"}}),
            json!({"a1": {"filter": {"unknown": 1}}}),
            json!({"a1": {"filter": {"inMailbox": 5}}}),
            json!({"a b": {}}),
        ] {
            let error = EmailPushConfig::parse_map(&bad);
            let error = match error {
                Ok(_) if bad == json!({"x": {}}) => continue,
                other => other.unwrap_err(),
            };
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{bad}");
        }
        assert!(EmailPushConfig::parse_map(&Value::Null).unwrap().is_none());
        let update =
            PushSubscriptionPatch::parse(&PatchObject::new(map(json!({"emailPush": null}))))
                .unwrap();
        assert_eq!(update.email_push, Some(None));
        // Only as a whole.
        assert!(
            PushSubscriptionPatch::parse(&PatchObject::new(map(
                json!({"emailPush/a1/urgency": "high"}),
            )))
            .is_err()
        );
    }

    #[test]
    fn responses() {
        let verification = PushVerification {
            push_subscription_id: "ps1".parse().unwrap(),
            verification_code: "abc".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&verification).unwrap(),
            json!({"@type": "PushVerification", "pushSubscriptionId": "ps1",
                   "verificationCode": "abc"})
        );
        let item = PushSubscription {
            id: Some("ps1".parse().unwrap()),
            device_client_id: Some("d".to_owned()),
            verification_code: Some(None),
            expires: Some(Some("2026-11-03T10:00:00Z".parse().unwrap())),
            types: Some(None),
            email_push: None,
        };
        assert_eq!(
            serde_json::to_value(PushSubscriptionGetResponse {
                list: vec![item],
                not_found: vec![],
            })
            .unwrap(),
            json!({"list": [{"id": "ps1", "deviceClientId": "d", "verificationCode": null,
                             "expires": "2026-11-03T10:00:00Z", "types": null}],
                   "notFound": []})
        );
        assert_eq!(
            serde_json::to_value(PushSubscriptionSetResponse::new()).unwrap(),
            json!({"created": null, "updated": null, "destroyed": null,
                   "notCreated": null, "notUpdated": null, "notDestroyed": null})
        );
    }
}
