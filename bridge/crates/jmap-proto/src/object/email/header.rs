// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Header field properties: `header:{name}[:as{Form}][:all]` (RFC 8621
//! §4.1.2, §4.1.3).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::address::{EmailAddress, EmailAddressGroup, is_single_line};
use crate::macros::{serde_via_str, string_enum};
use crate::types::Date;

string_enum! {
    /// How a header field value is parsed (RFC 8621 §4.1.2).
    pub enum HeaderForm ("header form") {
        /// `Raw`: the bytes as they are (the default).
        Raw = "Raw",
        /// `Text`: unstructured text, decoded and unfolded.
        Text = "Text",
        /// `Addresses`: `EmailAddress[]`.
        Addresses = "Addresses",
        /// `GroupedAddresses`: `EmailAddressGroup[]`.
        GroupedAddresses = "GroupedAddresses",
        /// `MessageIds`: `String[]|null`.
        MessageIds = "MessageIds",
        /// `Date`: `Date|null`.
        Date = "Date",
        /// `URLs`: `String[]|null`.
        Urls = "URLs",
    }
}

/// Which instances of the header field a property means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HeaderInstances {
    /// The last one (no suffix); `null` if there is none.
    Last,
    /// All of them, in order (`:all`); `[]` if there is none.
    All,
}

/// Why a string is not a valid header property.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HeaderPropertyError {
    /// Does not start with `header:`.
    #[error("not a header property")]
    NotHeader,
    /// The field name is empty or has characters outside `!`..`~`.
    #[error("invalid header field name")]
    InvalidName,
    /// The text after the name is not `:as{Form}` and/or `:all`.
    #[error("invalid header property suffix")]
    InvalidSuffix,
    /// RFC 8621 forbids this form for this field (e.g. `From` as `Date`).
    #[error("header field {name} can't be fetched or set as {form}")]
    FormNotAllowed {
        /// The field name.
        name: String,
        /// The form asked for.
        form: HeaderForm,
    },
}

/// A parsed `header:…` property, e.g. `header:List-Unsubscribe:asURLs`.
///
/// It keeps the exact text of the request: the response must use the same
/// capitalization (RFC 8621 §4.2). Field names match case-insensitively
/// ([`HeaderProperty::matches`]).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeaderProperty {
    raw: String,
    name: String,
    form: HeaderForm,
    instances: HeaderInstances,
}

impl HeaderProperty {
    /// The field name as written in the property.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The parsed form.
    #[must_use]
    pub fn form(&self) -> HeaderForm {
        self.form
    }

    /// Last instance or all of them.
    #[must_use]
    pub fn instances(&self) -> HeaderInstances {
        self.instances
    }

    /// Whether `field_name` (from a message) is this property's field.
    #[must_use]
    pub fn matches(&self, field_name: &str) -> bool {
        self.name.eq_ignore_ascii_case(field_name)
    }

    /// The property as sent, e.g. `header:Subject:asText`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

/// Fields defined in RFC 5322 or RFC 2369. Any other field may use every
/// form (RFC 8621 §4.1.2).
const STANDARD_FIELDS: &[&str] = &[
    "date",
    "from",
    "sender",
    "reply-to",
    "to",
    "cc",
    "bcc",
    "message-id",
    "in-reply-to",
    "references",
    "subject",
    "comments",
    "keywords",
    "resent-date",
    "resent-from",
    "resent-sender",
    "resent-to",
    "resent-cc",
    "resent-bcc",
    "resent-message-id",
    "return-path",
    "received",
    "list-help",
    "list-unsubscribe",
    "list-subscribe",
    "list-post",
    "list-owner",
    "list-archive",
];

/// The fields each parsed form allows, besides non-standard ones.
fn allowed_fields(form: HeaderForm) -> &'static [&'static str] {
    match form {
        HeaderForm::Raw => &[],
        HeaderForm::Text => &["subject", "comments", "keywords", "list-id"],
        HeaderForm::Addresses | HeaderForm::GroupedAddresses => &[
            "from",
            "sender",
            "reply-to",
            "to",
            "cc",
            "bcc",
            "resent-from",
            "resent-sender",
            "resent-reply-to",
            "resent-to",
            "resent-cc",
            "resent-bcc",
        ],
        HeaderForm::MessageIds => &[
            "message-id",
            "in-reply-to",
            "references",
            "resent-message-id",
        ],
        HeaderForm::Date => &["date", "resent-date"],
        HeaderForm::Urls => &[
            "list-help",
            "list-unsubscribe",
            "list-subscribe",
            "list-post",
            "list-owner",
            "list-archive",
        ],
    }
}

fn form_allowed(name: &str, form: HeaderForm) -> bool {
    let lower = name.to_ascii_lowercase();
    form == HeaderForm::Raw
        || !STANDARD_FIELDS.contains(&lower.as_str())
        || allowed_fields(form).contains(&lower.as_str())
}

impl FromStr for HeaderProperty {
    type Err = HeaderPropertyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s
            .strip_prefix("header:")
            .ok_or(HeaderPropertyError::NotHeader)?;
        let mut parts = rest.split(':');
        let name = parts.next().unwrap_or_default();
        if name.is_empty() || !name.bytes().all(|b| matches!(b, 33..=126)) {
            return Err(HeaderPropertyError::InvalidName);
        }
        let mut form = HeaderForm::Raw;
        let mut instances = HeaderInstances::Last;
        let mut next = parts.next();
        if let Some(form_name) = next.and_then(|part| part.strip_prefix("as")) {
            form = form_name
                .parse()
                .map_err(|_| HeaderPropertyError::InvalidSuffix)?;
            next = parts.next();
        }
        if next == Some("all") {
            instances = HeaderInstances::All;
            next = parts.next();
        }
        if next.is_some() {
            return Err(HeaderPropertyError::InvalidSuffix);
        }
        if !form_allowed(name, form) {
            return Err(HeaderPropertyError::FormNotAllowed {
                name: name.to_owned(),
                form,
            });
        }
        Ok(Self {
            raw: s.to_owned(),
            name: name.to_owned(),
            form,
            instances,
        })
    }
}

impl fmt::Display for HeaderProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl fmt::Debug for HeaderProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HeaderProperty({})", self.raw)
    }
}

serde_via_str!(HeaderProperty);

/// One header field value in a parsed form.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeaderFormValue {
    /// Raw form.
    Raw(String),
    /// Text form.
    Text(String),
    /// Addresses form.
    Addresses(Vec<EmailAddress>),
    /// `GroupedAddresses` form.
    GroupedAddresses(Vec<EmailAddressGroup>),
    /// `MessageIds` form (`None`: could not be parsed).
    MessageIds(Option<Vec<String>>),
    /// Date form (`None`: could not be parsed).
    Date(Option<Date>),
    /// URLs form (`None`: could not be parsed).
    Urls(Option<Vec<String>>),
}

impl HeaderFormValue {
    /// The form of this value.
    #[must_use]
    pub fn form(&self) -> HeaderForm {
        match self {
            Self::Raw(_) => HeaderForm::Raw,
            Self::Text(_) => HeaderForm::Text,
            Self::Addresses(_) => HeaderForm::Addresses,
            Self::GroupedAddresses(_) => HeaderForm::GroupedAddresses,
            Self::MessageIds(_) => HeaderForm::MessageIds,
            Self::Date(_) => HeaderForm::Date,
            Self::Urls(_) => HeaderForm::Urls,
        }
    }

    fn from_json(form: HeaderForm, value: Value) -> Result<Self, String> {
        let error = |e: serde_json::Error| e.to_string();
        let parsed = match form {
            HeaderForm::Raw => Self::Raw(serde_json::from_value(value).map_err(error)?),
            HeaderForm::Text => Self::Text(serde_json::from_value(value).map_err(error)?),
            HeaderForm::Addresses => Self::Addresses(serde_json::from_value(value).map_err(error)?),
            HeaderForm::GroupedAddresses => {
                Self::GroupedAddresses(serde_json::from_value(value).map_err(error)?)
            }
            HeaderForm::MessageIds => {
                Self::MessageIds(serde_json::from_value(value).map_err(error)?)
            }
            HeaderForm::Date => Self::Date(serde_json::from_value(value).map_err(error)?),
            HeaderForm::Urls => Self::Urls(serde_json::from_value(value).map_err(error)?),
        };
        if parsed.is_safe() {
            Ok(parsed)
        } else {
            Err("header value contains a line break or control character".to_owned())
        }
    }

    /// Whether the value can be written into a message without injecting
    /// header lines. Raw values may be folded (CRLF followed by a space or
    /// tab); everything else must be a single line.
    fn is_safe(&self) -> bool {
        match self {
            Self::Raw(raw) => is_folded_safely(raw),
            Self::Text(text) => is_single_line(text),
            Self::Addresses(addresses) => addresses.iter().all(EmailAddress::is_single_line),
            Self::GroupedAddresses(groups) => groups.iter().all(|group| {
                group.name.as_deref().is_none_or(is_single_line)
                    && group.addresses.iter().all(EmailAddress::is_single_line)
            }),
            Self::MessageIds(list) | Self::Urls(list) => list
                .as_deref()
                .is_none_or(|items| items.iter().all(|item| is_single_line(item))),
            Self::Date(_) => true,
        }
    }
}

/// Every CR or LF is part of a CRLF that is followed by a space or tab
/// (RFC 5322 folding); no other control characters except tab.
fn is_folded_safely(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.iter().enumerate().all(|(index, &b)| match b {
        b'\r' => {
            bytes.get(index.saturating_add(1)) == Some(&b'\n')
                && matches!(bytes.get(index.saturating_add(2)), Some(b' ' | b'\t'))
        }
        b'\n' => {
            index > 0
                && bytes.get(index.saturating_sub(1)) == Some(&b'\r')
                && matches!(bytes.get(index.saturating_add(1)), Some(b' ' | b'\t'))
        }
        b'\t' => true,
        other => !other.is_ascii_control(),
    })
}

impl Serialize for HeaderFormValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Raw(text) | Self::Text(text) => text.serialize(serializer),
            Self::Addresses(addresses) => addresses.serialize(serializer),
            Self::GroupedAddresses(groups) => groups.serialize(serializer),
            Self::MessageIds(list) | Self::Urls(list) => list.serialize(serializer),
            Self::Date(date) => date.serialize(serializer),
        }
    }
}

/// The value of a header property.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeaderValue {
    /// No such field (last instance asked): `null`.
    Missing,
    /// The last instance.
    Last(HeaderFormValue),
    /// Every instance (`:all`), possibly empty.
    All(Vec<HeaderFormValue>),
}

impl HeaderValue {
    /// Parses the JSON value of `property` (for `Email/set` create).
    ///
    /// # Errors
    /// A description if the value doesn't match the property's form, or
    /// has characters that could inject header lines.
    pub fn from_json(property: &HeaderProperty, value: Value) -> Result<Self, String> {
        match (property.instances, value) {
            (HeaderInstances::Last, Value::Null) => Ok(Self::Missing),
            (HeaderInstances::Last, value) => {
                HeaderFormValue::from_json(property.form, value).map(Self::Last)
            }
            (HeaderInstances::All, Value::Array(items)) => items
                .into_iter()
                .map(|item| HeaderFormValue::from_json(property.form, item))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::All),
            (HeaderInstances::All, _) => Err(format!("{property} must be an array")),
        }
    }
}

impl Serialize for HeaderValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Last(value) => value.serialize(serializer),
            Self::All(values) => values.serialize(serializer),
        }
    }
}

/// `header:*` properties of an Email or body part, serialized as extra
/// members of the object (`#[serde(flatten)]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderProperties(BTreeMap<HeaderProperty, HeaderValue>);

impl HeaderProperties {
    /// No header properties.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a property.
    pub fn insert(&mut self, property: HeaderProperty, value: HeaderValue) {
        self.0.insert(property, value);
    }

    /// The value of a property.
    #[must_use]
    pub fn get(&self, property: &HeaderProperty) -> Option<&HeaderValue> {
        self.0.get(property)
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// All properties and values.
    pub fn iter(&self) -> impl Iterator<Item = (&HeaderProperty, &HeaderValue)> {
        self.0.iter()
    }
}

impl Serialize for HeaderProperties {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (property, value) in &self.0 {
            map.serialize_entry(property.as_str(), value)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn parse(s: &str) -> Result<HeaderProperty, HeaderPropertyError> {
        s.parse()
    }

    #[test]
    fn parses_suffixes() {
        let plain = parse("header:subject").unwrap();
        assert_eq!(
            (plain.form(), plain.instances()),
            (HeaderForm::Raw, HeaderInstances::Last)
        );
        let all = parse("header:Resent-To:asAddresses:all").unwrap();
        assert_eq!(all.name(), "Resent-To");
        assert_eq!(
            (all.form(), all.instances()),
            (HeaderForm::Addresses, HeaderInstances::All)
        );
        let raw_all = parse("header:X-Spam:all").unwrap();
        assert_eq!(raw_all.instances(), HeaderInstances::All);
        let urls = parse("header:List-Unsubscribe:asURLs").unwrap();
        assert_eq!(urls.form(), HeaderForm::Urls);
        assert!(urls.matches("list-unsubscribe"));
        assert_eq!(urls.to_string(), "header:List-Unsubscribe:asURLs");
        // Explicit Raw is kept as written.
        assert_eq!(
            parse("header:To:asRaw").unwrap().as_str(),
            "header:To:asRaw"
        );
    }

    #[test]
    fn the_web_clients_properties_parse() {
        for property in [
            "header:Authentication-Results:asText:all",
            "header:DKIM-Signature:asText:all",
            "header:List-Unsubscribe:asURLs",
            "header:List-Unsubscribe-Post:asText",
            "header:List-Id:asText",
            "header:Content-Language",
        ] {
            assert!(parse(property).is_ok(), "{property}");
        }
    }

    #[test]
    fn rejects_bad_syntax_and_forbidden_forms() {
        assert_eq!(parse("subject"), Err(HeaderPropertyError::NotHeader));
        assert_eq!(parse("header:"), Err(HeaderPropertyError::InvalidName));
        assert_eq!(parse("header:a b"), Err(HeaderPropertyError::InvalidName));
        assert_eq!(
            parse("header:Subject:asNumber"),
            Err(HeaderPropertyError::InvalidSuffix)
        );
        assert_eq!(
            parse("header:Subject:all:asText"),
            Err(HeaderPropertyError::InvalidSuffix)
        );
        assert_eq!(
            parse("header:Subject:astext"),
            Err(HeaderPropertyError::InvalidSuffix)
        );
        assert_eq!(
            parse("header:Subject:asText:all:x"),
            Err(HeaderPropertyError::InvalidSuffix)
        );
        assert!(matches!(
            parse("header:From:asDate"),
            Err(HeaderPropertyError::FormNotAllowed { .. })
        ));
        assert!(matches!(
            parse("header:subject:asAddresses"),
            Err(HeaderPropertyError::FormNotAllowed { .. })
        ));
        assert!(matches!(
            parse("header:list-post:asText"),
            Err(HeaderPropertyError::FormNotAllowed { .. })
        ));
        // Non-standard fields allow any form.
        assert!(parse("header:X-Foo:asDate").is_ok());
        assert!(parse("header:X-Foo:asGroupedAddresses:all").is_ok());
    }

    #[test]
    fn values() {
        let addresses = parse("header:To:asAddresses").unwrap();
        let value = HeaderValue::from_json(
            &addresses,
            json!([{"name": "Joe", "email": "joe@example.com"}]),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            json!([{"name": "Joe", "email": "joe@example.com"}])
        );
        let all = parse("header:X-A:asText:all").unwrap();
        assert!(HeaderValue::from_json(&all, json!("x")).is_err());
        assert_eq!(
            HeaderValue::from_json(&all, json!(["a", "b"])).unwrap(),
            HeaderValue::All(vec![
                HeaderFormValue::Text("a".into()),
                HeaderFormValue::Text("b".into())
            ])
        );
        let date = parse("header:Date:asDate").unwrap();
        assert!(HeaderValue::from_json(&date, json!("2014-10-30T14:12:00+08:00")).is_ok());
        assert!(HeaderValue::from_json(&date, json!("yesterday")).is_err());
    }

    #[test]
    fn rejects_header_injection() {
        let text = parse("header:X-A:asText").unwrap();
        assert!(HeaderValue::from_json(&text, json!("a\r\nBcc: evil@example.com")).is_err());
        let raw = parse("header:X-A").unwrap();
        assert!(HeaderValue::from_json(&raw, json!(" folded\r\n line")).is_ok());
        assert!(HeaderValue::from_json(&raw, json!(" a\r\nBcc: x")).is_err());
        assert!(HeaderValue::from_json(&raw, json!(" a\nb")).is_err());
        assert!(HeaderValue::from_json(&raw, json!(" a\0b")).is_err());
        let to = parse("header:To:asAddresses").unwrap();
        assert!(HeaderValue::from_json(&to, json!([{"email": "a@b\r\nBcc: c@d"}])).is_err());
    }

    proptest! {
        #[test]
        fn parse_never_panics(s in "header:\\PC{0,30}") {
            let _ = s.parse::<HeaderProperty>();
        }

        #[test]
        fn valid_properties_round_trip(
            name in "X-[!-9;-~]{1,20}",
            form in proptest::sample::select(HeaderForm::ALL),
            all in any::<bool>(),
        ) {
            let text = format!(
                "header:{name}:as{}{}",
                form.as_str(),
                if all { ":all" } else { "" }
            );
            let property: HeaderProperty = text.parse().unwrap();
            prop_assert_eq!(property.form(), form);
            prop_assert_eq!(property.to_string(), text);
        }
    }
}
