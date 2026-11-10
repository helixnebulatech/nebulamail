// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `EmailAddress`, `EmailAddressGroup` and `EmailHeader` (RFC 8621 §4.1.2,
//! §4.1.3).

use serde::{Deserialize, Serialize};

/// A parsed address: `{"name": "Joe Bloggs", "email": "joe@example.com"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailAddress {
    /// The display name, or `None` (`null`) if there is none.
    #[serde(default)]
    pub name: Option<String>,
    /// The address (`addr-spec`). Kept as given: a half-written draft may
    /// hold an invalid one; it is checked when sending.
    pub email: String,
}

/// An address group (`Friends: a@x, b@y;`), or the addresses outside any
/// group (`name` is `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailAddressGroup {
    /// The group's display name, or `None` (`null`).
    #[serde(default)]
    pub name: Option<String>,
    /// The members.
    pub addresses: Vec<EmailAddress>,
}

/// A header field in Raw form: `{"name": "Subject", "value": " Hi"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailHeader {
    /// The field name, with the message's capitalization.
    pub name: String,
    /// The raw field value (after the colon, unfolded is not guaranteed).
    pub value: String,
}

/// Whether `text` is safe on one header line: no control characters
/// (tabs allowed). Stops CR/LF header injection through JMAP values.
pub(crate) fn is_single_line(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() && c != '\t')
}

impl EmailAddress {
    /// Whether name and email have no line breaks or other control
    /// characters (needed before writing them into a message).
    #[must_use]
    pub fn is_single_line(&self) -> bool {
        is_single_line(&self.email) && self.name.as_deref().is_none_or(is_single_line)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_shapes() {
        let address: EmailAddress =
            serde_json::from_value(json!({"email": "joe@example.com"})).unwrap();
        assert_eq!(address.name, None);
        assert_eq!(
            serde_json::to_value(&address).unwrap(),
            json!({"name": null, "email": "joe@example.com"})
        );
        let group: EmailAddressGroup = serde_json::from_value(json!({
            "name": "Friends",
            "addresses": [{"name": "John Smith", "email": "john@example.com"}]
        }))
        .unwrap();
        assert_eq!(group.addresses.len(), 1);
        assert!(serde_json::from_value::<EmailAddress>(json!({"email": "a", "x": 1})).is_err());
    }

    #[test]
    fn single_line() {
        assert!(
            EmailAddress {
                name: Some("A\tB".into()),
                email: "a@b".into()
            }
            .is_single_line()
        );
        assert!(
            !EmailAddress {
                name: None,
                email: "a@b\r\nBcc: x".into()
            }
            .is_single_line()
        );
    }
}
