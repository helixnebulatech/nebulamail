// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Email/get` arguments (RFC 8621 §4.2).

use serde_json::{Map, Value};

use super::body::EmailBodyPartProperty;
use super::property::EmailProperty;
use crate::error::MethodError;
use crate::method::GetArguments;
use crate::method::parse::take_argument;
use crate::types::{EmailId, UnsignedInt};

/// Which body values to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FetchBodyValues {
    /// `fetchTextBodyValues`: parts in `textBody`.
    pub text: bool,
    /// `fetchHTMLBodyValues`: parts in `htmlBody`.
    pub html: bool,
    /// `fetchAllBodyValues`: every `text/*` part.
    pub all: bool,
}

/// `Email/get` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailGetArguments {
    /// The standard arguments.
    pub get: GetArguments<EmailId, EmailProperty>,
    /// Properties of each body part; `None` means the default list
    /// ([`EmailBodyPartProperty::DEFAULT`]).
    pub body_properties: Option<Vec<EmailBodyPartProperty>>,
    /// Which `bodyValues` to include.
    pub fetch_body_values: FetchBodyValues,
    /// Cut each body value to this many bytes (`None` or 0: no limit).
    pub max_body_value_bytes: Option<UnsignedInt>,
}

impl EmailGetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` (unknown properties, forbidden header forms…) or
    /// `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let body_properties = take_argument(&mut arguments, "bodyProperties")?;
        let fetch_body_values = FetchBodyValues {
            text: take_argument(&mut arguments, "fetchTextBodyValues")?.unwrap_or(false),
            html: take_argument(&mut arguments, "fetchHTMLBodyValues")?.unwrap_or(false),
            all: take_argument(&mut arguments, "fetchAllBodyValues")?.unwrap_or(false),
        };
        let max_body_value_bytes = take_argument(&mut arguments, "maxBodyValueBytes")?;
        Ok(Self {
            get: GetArguments::parse(arguments, max_objects)?,
            body_properties,
            fetch_body_values,
            max_body_value_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::object::email::BodyPartField;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// RFC 8621 §4.2.1 (with the `accountId` the example forgets).
    #[test]
    fn rfc_example() {
        let args = EmailGetArguments::parse(
            map(json!({
                "accountId": "abc",
                "ids": ["f123u456", "f123u457"],
                "properties": ["threadId", "mailboxIds", "from", "subject",
                    "receivedAt", "header:List-POST:asURLs", "htmlBody", "bodyValues"],
                "bodyProperties": ["partId", "blobId", "size", "type"],
                "fetchHTMLBodyValues": true,
                "maxBodyValueBytes": 256
            })),
            100,
        )
        .unwrap();
        assert_eq!(args.get.properties.unwrap().len(), 8);
        assert_eq!(
            args.body_properties.unwrap()[3],
            EmailBodyPartProperty::Field(BodyPartField::Type)
        );
        assert!(args.fetch_body_values.html && !args.fetch_body_values.text);
        assert_eq!(args.max_body_value_bytes, UnsignedInt::new(256));
    }

    #[test]
    fn errors() {
        let kind = |value: Value| {
            EmailGetArguments::parse(map(value), 100)
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "a", "properties": ["header:From:asDate"]})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "bodyProperties": ["nope"]})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "fetchAllBodyValues": "yes"})),
            MethodErrorType::InvalidArguments
        );
    }
}
