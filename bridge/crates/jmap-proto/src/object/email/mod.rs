// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Email (RFC 8621 §4).

mod address;
mod body;
mod create;
mod get;
mod header;
mod parse;
pub(crate) mod patch;
mod property;
mod query;

use std::collections::BTreeMap;

use serde::Serialize;

pub use address::{EmailAddress, EmailAddressGroup, EmailHeader};
pub use body::{
    BodyPartContent, BodyPartField, EmailBodyPart, EmailBodyPartCreate, EmailBodyPartProperty,
    EmailBodyValue, MAX_BODY_DEPTH, PartId, PartIdError,
};
pub use create::{EmailCreate, EmailCreateBody};
pub use get::{EmailGetArguments, FetchBodyValues};
pub use header::{
    HeaderForm, HeaderFormValue, HeaderInstances, HeaderProperties, HeaderProperty,
    HeaderPropertyError, HeaderValue,
};
pub use parse::{DEFAULT_PARSE_PROPERTIES, EmailParseArguments, EmailParseResponse};
pub use patch::{EmailPatch, SetPatch};
pub use property::{EmailField, EmailProperty};
pub use query::{
    EmailFilterCondition, EmailQueryArguments, EmailQueryChangesArguments, EmailSort,
    HeaderCondition,
};

use crate::types::{
    BlobId, Date, EmailId, Keyword, MailboxId, ThreadId, TrueSet, UnsignedInt, UtcDate,
};

/// An Email as returned by `Email/get` (and the server-set properties in
/// `Email/set` `created` / `Email/import`).
///
/// Only requested properties are set: `None` = not sent; for nullable
/// properties `Some(None)` = `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<EmailId>,
    /// `blobId` of the raw message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<BlobId>,
    /// `threadId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// `mailboxIds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<TrueSet<MailboxId>>,
    /// `keywords`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<TrueSet<Keyword>>,
    /// `size` of the raw message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<UnsignedInt>,
    /// `receivedAt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<UtcDate>,
    /// `messageId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Option<Vec<String>>>,
    /// `inReplyTo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Option<Vec<String>>>,
    /// `references`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<Option<Vec<String>>>,
    /// `sender`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<Option<Vec<EmailAddress>>>,
    /// `from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Option<Vec<EmailAddress>>>,
    /// `to`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Option<Vec<EmailAddress>>>,
    /// `cc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Option<Vec<EmailAddress>>>,
    /// `bcc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Option<Vec<EmailAddress>>>,
    /// `replyTo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Option<Vec<EmailAddress>>>,
    /// `subject`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Option<String>>,
    /// `sentAt` (the `Date` header).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<Option<Date>>,
    /// `hasAttachment`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// `preview` (up to 256 characters of plain text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// `bodyStructure`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_structure: Option<EmailBodyPart>,
    /// `bodyValues`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_values: Option<BTreeMap<PartId, EmailBodyValue>>,
    /// `textBody`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Vec<EmailBodyPart>>,
    /// `htmlBody`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Vec<EmailBodyPart>>,
    /// `attachments`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<EmailBodyPart>>,
    /// `headers` (all fields, Raw form).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// Requested `header:…` properties.
    #[serde(flatten)]
    pub header_properties: HeaderProperties,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 8621 §4.2.1 response.
    #[test]
    fn rfc_get_response_shape() {
        let mut header_properties = HeaderProperties::new();
        header_properties.insert(
            "header:List-POST:asURLs".parse().unwrap(),
            HeaderValue::Last(HeaderFormValue::Urls(Some(vec![
                "mailto:partytime@lists.example.com".to_owned(),
            ]))),
        );
        let part = |part_id: &str, blob_id: &str, size: u32, media_type: &str| EmailBodyPart {
            part_id: Some(Some(part_id.parse().unwrap())),
            blob_id: Some(Some(blob_id.parse().unwrap())),
            size: Some(UnsignedInt::from(size)),
            media_type: Some(media_type.to_owned()),
            ..EmailBodyPart::default()
        };
        let mut body_values = BTreeMap::new();
        body_values.insert(
            "1".parse().unwrap(),
            EmailBodyValue {
                value: "<html><body><p>Hello ...".to_owned(),
                is_encoding_problem: false,
                is_truncated: true,
            },
        );
        body_values.insert(
            "2".parse().unwrap(),
            EmailBodyValue {
                value: "-- Sent by your friendly mailing list ...".to_owned(),
                is_encoding_problem: false,
                is_truncated: false,
            },
        );
        let email = Email {
            id: Some("f123u457".parse().unwrap()),
            thread_id: Some("ef1314a".parse().unwrap()),
            mailbox_ids: Some(["f123".parse().unwrap()].into_iter().collect()),
            from: Some(Some(vec![EmailAddress {
                name: Some("Joe Bloggs".to_owned()),
                email: "joe@example.com".to_owned(),
            }])),
            subject: Some(Some("Dinner on Thursday?".to_owned())),
            received_at: Some("2013-10-13T14:12:00Z".parse().unwrap()),
            html_body: Some(vec![
                part("1", "B841623871", 283_331, "text/html"),
                part("2", "B319437193", 10_343, "text/plain"),
            ]),
            body_values: Some(body_values),
            header_properties,
            ..Email::default()
        };
        assert_eq!(
            serde_json::to_value(&email).unwrap(),
            json!({
                "id": "f123u457",
                "threadId": "ef1314a",
                "mailboxIds": {"f123": true},
                "from": [{"name": "Joe Bloggs", "email": "joe@example.com"}],
                "subject": "Dinner on Thursday?",
                "receivedAt": "2013-10-13T14:12:00Z",
                "header:List-POST:asURLs": ["mailto:partytime@lists.example.com"],
                "htmlBody": [{
                    "partId": "1", "blobId": "B841623871", "size": 283_331, "type": "text/html"
                }, {
                    "partId": "2", "blobId": "B319437193", "size": 10343, "type": "text/plain"
                }],
                "bodyValues": {
                    "1": {"isEncodingProblem": false, "isTruncated": true,
                          "value": "<html><body><p>Hello ..."},
                    "2": {"isEncodingProblem": false, "isTruncated": false,
                          "value": "-- Sent by your friendly mailing list ..."}
                }
            })
        );
    }

    #[test]
    fn nulls() {
        let email = Email {
            id: Some("e1".parse().unwrap()),
            subject: Some(None),
            sent_at: Some(None),
            header_properties: {
                let mut headers = HeaderProperties::new();
                headers.insert("header:X-A".parse().unwrap(), HeaderValue::Missing);
                headers.insert("header:X-B:all".parse().unwrap(), HeaderValue::All(vec![]));
                headers
            },
            ..Email::default()
        };
        assert_eq!(
            serde_json::to_value(&email).unwrap(),
            json!({"id": "e1", "subject": null, "sentAt": null, "header:X-A": null, "header:X-B:all": []})
        );
    }
}
