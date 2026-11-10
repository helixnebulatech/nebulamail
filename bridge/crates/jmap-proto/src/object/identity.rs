// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Identity (RFC 8621 §6).

use serde::Serialize;
use serde_json::{Map, Value};

use super::email::EmailAddress;
use crate::error::SetError;
use crate::macros::string_enum;
use crate::method::PatchObject;
use crate::method::parse::PropertyErrors;
use crate::method::patch::{Patch, Target};
use crate::types::IdentityId;

string_enum! {
    /// Identity properties.
    pub enum IdentityProperty ("Identity property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `email`.
        Email = "email",
        /// `replyTo`.
        ReplyTo = "replyTo",
        /// `bcc`.
        Bcc = "bcc",
        /// `textSignature`.
        TextSignature = "textSignature",
        /// `htmlSignature`.
        HtmlSignature = "htmlSignature",
        /// `mayDelete`.
        MayDelete = "mayDelete",
    }
}

/// An Identity as returned by `Identity/get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<IdentityId>,
    /// `name` (display name, may be empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `email`: the From address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// `replyTo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Option<Vec<EmailAddress>>>,
    /// `bcc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Option<Vec<EmailAddress>>>,
    /// `textSignature`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    /// `htmlSignature` (clients must sanitize it before showing it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_signature: Option<String>,
    /// `mayDelete`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
}

/// Mutable Identity properties, shared by create and update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityFields {
    /// New `name`.
    pub name: Option<String>,
    /// New `replyTo` (`Some(None)` clears it).
    pub reply_to: Option<Option<Vec<EmailAddress>>>,
    /// New `bcc` (`Some(None)` clears it).
    pub bcc: Option<Option<Vec<EmailAddress>>>,
    /// New `textSignature`.
    pub text_signature: Option<String>,
    /// New `htmlSignature`.
    pub html_signature: Option<String>,
}

/// An Identity to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCreate {
    /// The From address (the server checks the user may use it).
    pub email: String,
    /// Other properties; missing ones take their defaults (`""`, `null`).
    pub fields: IdentityFields,
}

/// Longest `name`, in bytes (the same limit the bridge's store keeps).
pub const MAX_IDENTITY_NAME: usize = 256;
/// Longest `textSignature` or `htmlSignature`, in bytes.
pub const MAX_IDENTITY_SIGNATURE: usize = 64 * 1024;
/// Most addresses in `replyTo` or in `bcc`.
pub const MAX_IDENTITY_ADDRESSES: usize = 32;
/// Longest `email`, and longest `name` or `email` of a `replyTo`/`bcc`
/// address, in bytes.
pub const MAX_IDENTITY_ADDRESS: usize = 512;

/// Reads a mutable property; `false` if `key` is not one.
fn read_field(
    fields: &mut IdentityFields,
    errors: &mut PropertyErrors,
    key: &str,
    value: Value,
) -> bool {
    // `null` resets the string properties to their default, "".
    let text = |errors: &mut PropertyErrors, value, max: usize| {
        let text = errors
            .parse::<Option<String>>(key, value)
            .map(Option::unwrap_or_default);
        if text.as_ref().is_some_and(|text| text.len() > max) {
            errors.invalid_because(key, format!("is longer than {max} bytes"));
            return None;
        }
        text
    };
    let addresses = |errors: &mut PropertyErrors, value| {
        let list: Option<Option<Vec<EmailAddress>>> = errors.parse(key, value);
        let all = || list.iter().flatten().flatten();
        if !all().all(EmailAddress::is_single_line) {
            errors.invalid_because(key, "has a line break");
            None
        } else if list
            .iter()
            .flatten()
            .any(|l| l.len() > MAX_IDENTITY_ADDRESSES)
        {
            errors.invalid_because(
                key,
                format!("has more than {MAX_IDENTITY_ADDRESSES} addresses"),
            );
            None
        } else if all().any(|address| {
            address.email.len() > MAX_IDENTITY_ADDRESS
                || address
                    .name
                    .as_ref()
                    .is_some_and(|n| n.len() > MAX_IDENTITY_ADDRESS)
        }) {
            errors.invalid_because(key, "has an address that is too long");
            None
        } else {
            list
        }
    };
    match key {
        "name" => {
            fields.name = text(errors, value, MAX_IDENTITY_NAME);
            if fields
                .name
                .as_deref()
                .is_some_and(|n| n.contains(['\r', '\n']))
            {
                errors.invalid_because(key, "has a line break");
            }
        }
        "textSignature" => fields.text_signature = text(errors, value, MAX_IDENTITY_SIGNATURE),
        "htmlSignature" => fields.html_signature = text(errors, value, MAX_IDENTITY_SIGNATURE),
        "replyTo" => fields.reply_to = addresses(errors, value),
        "bcc" => fields.bcc = addresses(errors, value),
        _ => return false,
    }
    true
}

/// Mutable Identity properties.
const FIELDS: &[&str] = &["name", "textSignature", "htmlSignature", "replyTo", "bcc"];

impl IdentityCreate {
    /// Parses one object of `Identity/set` `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every problem.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut fields = IdentityFields::default();
        let mut email = None;
        for (key, value) in object {
            if key == "email" {
                email = errors.parse::<String>(&key, value);
            } else if FIELDS.contains(&key.as_str()) {
                read_field(&mut fields, &mut errors, &key, value);
            } else {
                errors.invalid(key);
            }
        }
        if email.as_deref().is_some_and(|e| {
            e.is_empty() || e.len() > MAX_IDENTITY_ADDRESS || e.chars().any(char::is_control)
        }) {
            errors.invalid_because("email", "must be an address");
        }
        let email = errors.required("email", email);
        errors.finish()?;
        Ok(Self {
            email: email.unwrap_or_default(),
            fields,
        })
    }
}

/// An `Identity/set` update (`email` is immutable).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityPatch {
    /// The changed properties.
    pub fields: IdentityFields,
}

impl IdentityPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad paths; `invalidProperties` for immutable,
    /// unknown or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut fields = IdentityFields::default();
        for Patch { key, path, value } in patch.patches()? {
            match Target::of(&path) {
                Target::Property(property) => {
                    if !read_field(&mut fields, &mut errors, property, value) {
                        errors.invalid(key);
                    }
                }
                Target::Member(..) | Target::Deeper(..) => {
                    return Err(SetError::invalid_patch(format!(
                        "{key:?}: Identity properties have no members to patch"
                    )));
                }
            }
        }
        errors.finish()?;
        Ok(Self { fields })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    /// RFC 8621 §6.4 (the Identity/get response).
    #[test]
    fn rfc_shape() {
        let identity = Identity {
            id: Some("XD-3301-222-11_22AAz".parse().unwrap()),
            name: Some("Joe Bloggs".to_owned()),
            email: Some("joe@example.com".to_owned()),
            reply_to: Some(None),
            bcc: Some(Some(vec![EmailAddress {
                name: None,
                email: "joe+archive@example.com".to_owned(),
            }])),
            text_signature: Some("-- \nCheers!".to_owned()),
            html_signature: Some("<p>Cheers!</p>".to_owned()),
            may_delete: Some(false),
        };
        assert_eq!(
            serde_json::to_value(&identity).unwrap(),
            json!({
                "id": "XD-3301-222-11_22AAz",
                "name": "Joe Bloggs",
                "email": "joe@example.com",
                "replyTo": null,
                "bcc": [{"name": null, "email": "joe+archive@example.com"}],
                "textSignature": "-- \nCheers!",
                "htmlSignature": "<p>Cheers!</p>",
                "mayDelete": false
            })
        );
    }

    #[test]
    fn create_and_patch() {
        let created = IdentityCreate::parse(
            json!({"email": "a@b", "name": "A", "replyTo": null})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert_eq!(created.email, "a@b");
        assert_eq!(created.fields.reply_to, Some(None));
        let error = IdentityCreate::parse(
            json!({"name": "A", "mayDelete": true})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap_err();
        assert_eq!(error.properties(), ["mayDelete", "email"]);
        let patch: PatchObject =
            serde_json::from_value(json!({"textSignature": null, "name": "B"})).unwrap();
        let parsed = IdentityPatch::parse(&patch).unwrap();
        assert_eq!(parsed.fields.text_signature.as_deref(), Some(""));
        let immutable: PatchObject = serde_json::from_value(json!({"email": "x@y"})).unwrap();
        assert_eq!(
            IdentityPatch::parse(&immutable).unwrap_err().kind(),
            SetErrorType::InvalidProperties
        );
        let bad: PatchObject =
            serde_json::from_value(json!({"bcc": [{"email": "a@b\r\nX: y"}]})).unwrap();
        assert!(IdentityPatch::parse(&bad).is_err());
    }

    #[test]
    fn sizes_are_limited() {
        let create = |extra: Value| {
            let mut object = json!({"email": "a@b"}).as_object().unwrap().clone();
            object.extend(extra.as_object().unwrap().clone());
            IdentityCreate::parse(object)
        };
        let patch = |value: Value| {
            let patch: PatchObject = serde_json::from_value(value).unwrap();
            IdentityPatch::parse(&patch)
        };
        let address = |n: usize| json!({"email": format!("u{n}@b")});
        let many = |count: usize| Value::Array((0..count).map(address).collect());
        // At the limits.
        assert!(
            create(json!({
                "name": "a".repeat(MAX_IDENTITY_NAME),
                "textSignature": "s".repeat(MAX_IDENTITY_SIGNATURE),
                "htmlSignature": "h".repeat(MAX_IDENTITY_SIGNATURE),
                "replyTo": many(MAX_IDENTITY_ADDRESSES),
                "bcc": many(MAX_IDENTITY_ADDRESSES),
            }))
            .is_ok()
        );
        // Bytes, not characters: 129 "é" are 258 bytes.
        let error = create(json!({"name": "é".repeat(129)})).unwrap_err();
        assert_eq!(error.properties(), ["name"]);
        for (key, value) in [
            ("name", json!("a".repeat(MAX_IDENTITY_NAME + 1))),
            (
                "textSignature",
                json!("s".repeat(MAX_IDENTITY_SIGNATURE + 1)),
            ),
            (
                "htmlSignature",
                json!("h".repeat(MAX_IDENTITY_SIGNATURE + 1)),
            ),
            ("replyTo", many(MAX_IDENTITY_ADDRESSES + 1)),
            (
                "bcc",
                json!([{"email": "x".repeat(MAX_IDENTITY_ADDRESS + 1)}]),
            ),
            (
                "bcc",
                json!([{"name": "n".repeat(MAX_IDENTITY_ADDRESS + 1), "email": "a@b"}]),
            ),
        ] {
            let error = create(json!({key: value.clone()})).unwrap_err();
            assert_eq!(error.properties(), [key], "{key}");
            let error = patch(json!({key: value})).unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{key}");
        }
        let long = format!("{}@b", "a".repeat(MAX_IDENTITY_ADDRESS));
        let error =
            IdentityCreate::parse(json!({"email": long}).as_object().unwrap().clone()).unwrap_err();
        assert_eq!(error.properties(), ["email"]);
    }
}
