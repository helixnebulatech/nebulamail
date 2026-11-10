// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Email/parse` (RFC 8621 §4.9).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use super::body::EmailBodyPartProperty;
use super::get::FetchBodyValues;
use super::property::{EmailField, EmailProperty};
use crate::error::MethodError;
use crate::method::GetArguments;
use crate::method::parse::take_argument;
use crate::types::serde_util::{empty_map_as_null, empty_vec_as_null};
use crate::types::{AccountId, BlobId, EmailId, UnsignedInt};

/// The `properties` when the client gives none (RFC 8621 §4.9).
pub const DEFAULT_PARSE_PROPERTIES: &[EmailField] = &[
    EmailField::MessageId,
    EmailField::InReplyTo,
    EmailField::References,
    EmailField::Sender,
    EmailField::From,
    EmailField::To,
    EmailField::Cc,
    EmailField::Bcc,
    EmailField::ReplyTo,
    EmailField::Subject,
    EmailField::SentAt,
    EmailField::HasAttachment,
    EmailField::Preview,
    EmailField::BodyValues,
    EmailField::TextBody,
    EmailField::HtmlBody,
    EmailField::Attachments,
];

/// `Email/parse` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailParseArguments {
    /// The account.
    pub account_id: AccountId,
    /// The blobs to parse as messages.
    pub blob_ids: Vec<BlobId>,
    /// Properties of each Email; `None` means [`DEFAULT_PARSE_PROPERTIES`].
    pub properties: Option<Vec<EmailProperty>>,
    /// Properties of each body part; `None` means the `Email/get` default.
    pub body_properties: Option<Vec<EmailBodyPartProperty>>,
    /// Which `bodyValues` to include.
    pub fetch_body_values: FetchBodyValues,
    /// Cut each body value to this many bytes (`None` or 0: no limit).
    pub max_body_value_bytes: Option<UnsignedInt>,
}

impl EmailParseArguments {
    /// Parses the arguments. `properties` is read like `Email/get`'s
    /// (header forms checked the same way, RFC 8621 §4.9: "parsed forms
    /// of headers may only be used on appropriate header fields").
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects` blobs.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        if arguments.contains_key("ids") {
            return Err(MethodError::invalid_arguments(
                "Email/parse takes blobIds, not ids",
            ));
        }
        let blob_ids: Vec<BlobId> = take_argument(&mut arguments, "blobIds")?
            .ok_or_else(|| MethodError::invalid_arguments("blobIds is required"))?;
        if blob_ids.len() > max_objects {
            return Err(MethodError::request_too_large("too many blobs"));
        }
        let body_properties = take_argument(&mut arguments, "bodyProperties")?;
        let fetch_body_values = FetchBodyValues {
            text: take_argument(&mut arguments, "fetchTextBodyValues")?.unwrap_or(false),
            html: take_argument(&mut arguments, "fetchHTMLBodyValues")?.unwrap_or(false),
            all: take_argument(&mut arguments, "fetchAllBodyValues")?.unwrap_or(false),
        };
        let max_body_value_bytes = take_argument(&mut arguments, "maxBodyValueBytes")?;
        // The rest (`accountId`, `properties`) as `Email/get` reads them.
        let get = GetArguments::<EmailId, EmailProperty>::parse(arguments, max_objects)?;
        Ok(Self {
            account_id: get.account_id,
            blob_ids,
            properties: get.properties,
            body_properties,
            fetch_body_values,
            max_body_value_bytes,
        })
    }
}

/// `Email/parse` response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailParseResponse {
    /// The account.
    pub account_id: AccountId,
    /// Blob → the Email it holds, as JSON: `id`, `mailboxIds`,
    /// `keywords`, `receivedAt` and `threadId` are `null` when asked for,
    /// which the [`Email`](super::Email) type can't say.
    #[serde(serialize_with = "empty_map_as_null")]
    pub parsed: BTreeMap<BlobId, Map<String, Value>>,
    /// Blobs that are not messages.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_parsable: Vec<BlobId>,
    /// Blobs that don't exist.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_found: Vec<BlobId>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn arguments() {
        let args = EmailParseArguments::parse(
            map(json!({
                "accountId": "a", "blobIds": ["bu1", "bp2_3"],
                "properties": ["subject", "header:X-Foo:asText", "bodyValues"],
                "fetchTextBodyValues": true, "maxBodyValueBytes": 10
            })),
            10,
        )
        .unwrap();
        assert_eq!(args.blob_ids.len(), 2);
        assert_eq!(args.properties.unwrap().len(), 3);
        assert!(args.fetch_body_values.text);
        let kind = |value| {
            EmailParseArguments::parse(map(value), 1)
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "a", "blobIds": ["x"], "ids": ["e1"]})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "a", "blobIds": ["x", "y"]})),
            MethodErrorType::RequestTooLarge
        );
        assert_eq!(
            kind(json!({"accountId": "a", "blobIds": ["x"], "nope": 1})),
            MethodErrorType::InvalidArguments
        );
    }
}
