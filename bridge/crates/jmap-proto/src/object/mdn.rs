// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Message Disposition Notifications (RFC 9007): `MDN/send` and
//! `MDN/parse`.
//!
//! Every text that ends up in a header field of the report (`subject`,
//! `reportingUA`, `finalRecipient`, extension fields) is rejected if it has
//! CR, LF or other control characters: no header injection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::PatchObject;
use crate::method::parse::{from_arguments, take_argument};
use crate::references::IdOrRef;
use crate::types::serde_util::empty_map_as_null;
use crate::types::{AccountId, BlobId, CreationId, EmailId, IdentityId};

string_enum! {
    /// `actionMode` (RFC 8098 §3.2.6.1).
    pub enum ActionMode ("MDN action mode") {
        /// The user asked for it.
        ManualAction = "manual-action",
        /// Done automatically.
        AutomaticAction = "automatic-action",
    }
}

string_enum! {
    /// `sendingMode` (RFC 8098 §3.2.6.2).
    pub enum SendingMode ("MDN sending mode") {
        /// The user confirmed sending the MDN.
        MdnSentManually = "mdn-sent-manually",
        /// Sent without asking.
        MdnSentAutomatically = "mdn-sent-automatically",
    }
}

string_enum! {
    /// The disposition `type` (RFC 8098 §3.2.6.3).
    pub enum DispositionType ("MDN disposition type") {
        /// Deleted without being displayed.
        Deleted = "deleted",
        /// Sent somewhere else (printed, faxed, forwarded).
        Dispatched = "dispatched",
        /// Shown to the user.
        Displayed = "displayed",
        /// Processed in some other way.
        Processed = "processed",
    }
}

/// `disposition` of an MDN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Disposition {
    /// Action mode.
    pub action_mode: ActionMode,
    /// Sending mode.
    pub sending_mode: SendingMode,
    /// What happened.
    #[serde(rename = "type")]
    pub kind: DispositionType,
}

/// An MDN as `MDN/parse` returns it (every property, `null` if absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mdn {
    /// The email it is about, if it is in the account.
    pub for_email_id: Option<EmailId>,
    /// Subject of the report message.
    pub subject: Option<String>,
    /// Human-readable part of the report.
    pub text_body: Option<String>,
    /// Whether the report included the original message.
    pub include_original_message: bool,
    /// `Reporting-UA`.
    #[serde(rename = "reportingUA")]
    pub reporting_ua: Option<String>,
    /// `Disposition`.
    pub disposition: Disposition,
    /// `MDN-Gateway`.
    pub mdn_gateway: Option<String>,
    /// `Original-Recipient`.
    pub original_recipient: Option<String>,
    /// `Final-Recipient`.
    pub final_recipient: Option<String>,
    /// `Original-Message-ID`.
    pub original_message_id: Option<String>,
    /// `Error` fields.
    pub error: Option<Vec<String>>,
    /// Other fields of the report.
    pub extension_fields: Option<BTreeMap<String, String>>,
}

/// What `MDN/send` returns for a sent MDN: the properties the server set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdnSent {
    /// `finalRecipient`, if the client left it out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_recipient: Option<String>,
    /// `originalRecipient` (`null`: unknown).
    pub original_recipient: Option<String>,
    /// `originalMessageId` (`null`: the email has no Message-ID).
    pub original_message_id: Option<String>,
    /// `mdnGateway` (always `null`: no gateway).
    pub mdn_gateway: Option<String>,
    /// `reportingUA`, if the client left it out.
    #[serde(rename = "reportingUA", skip_serializing_if = "Option::is_none")]
    pub reporting_ua: Option<String>,
}

/// An MDN to send, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnCreate {
    /// The email to report on.
    pub for_email_id: IdOrRef<EmailId>,
    /// Subject of the report (`None`: the server picks one).
    pub subject: Option<String>,
    /// Text of the report (`None`: the server writes one).
    pub text_body: Option<String>,
    /// Attach the original message's header.
    pub include_original_message: bool,
    /// `Reporting-UA` (`None`: the server's).
    pub reporting_ua: Option<String>,
    /// What happened to the email.
    pub disposition: Disposition,
    /// `Final-Recipient` (`None`: the identity's address).
    pub final_recipient: Option<String>,
    /// Extra fields for the report.
    pub extension_fields: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MdnCreateWire {
    for_email_id: IdOrRef<EmailId>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    text_body: Option<String>,
    #[serde(default)]
    include_original_message: Option<bool>,
    #[serde(default, rename = "reportingUA")]
    reporting_ua: Option<String>,
    disposition: Disposition,
    #[serde(default)]
    final_recipient: Option<String>,
    #[serde(default)]
    extension_fields: Option<BTreeMap<String, String>>,
}

/// Properties only the server sets.
const SERVER_SET: [&str; 4] = [
    "mdnGateway",
    "originalRecipient",
    "originalMessageId",
    "error",
];

/// A header field value: one line, no control characters, not too long.
fn field_ok(text: &str) -> bool {
    !text.is_empty() && text.len() <= 998 && !text.chars().any(char::is_control)
}

/// A header field name (RFC 5322 `ftext`), not one the report sets itself.
fn field_name_ok(name: &str) -> bool {
    const OWN: [&str; 6] = [
        "reporting-ua",
        "mdn-gateway",
        "original-recipient",
        "final-recipient",
        "original-message-id",
        "disposition",
    ];
    !name.is_empty()
        && name.len() <= 76
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
        && !OWN.contains(&name.to_ascii_lowercase().as_str())
}

impl MdnCreate {
    /// Parses one MDN of `MDN/send`.
    ///
    /// # Errors
    /// `invalidProperties` naming the bad properties.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        if let Some(key) = object.keys().find(|k| SERVER_SET.contains(&k.as_str())) {
            return Err(
                SetError::invalid_properties([key.clone()]).with_description("set by the server")
            );
        }
        let wire: MdnCreateWire =
            serde_json::from_value(Value::Object(object)).map_err(|error| {
                SetError::invalid_properties(Vec::<String>::new())
                    .with_description(error.to_string())
            })?;
        let mut bad = Vec::new();
        if wire.subject.as_deref().is_some_and(|s| !field_ok(s)) {
            bad.push("subject");
        }
        if wire.reporting_ua.as_deref().is_some_and(|s| !field_ok(s)) {
            bad.push("reportingUA");
        }
        // `type;address`, like `rfc822;bob@example.org`.
        let recipient_ok = |s: &str| field_ok(s) && s.contains(';') && s.len() <= 500;
        if wire
            .final_recipient
            .as_deref()
            .is_some_and(|s| !recipient_ok(s))
        {
            bad.push("finalRecipient");
        }
        let fields = wire.extension_fields.unwrap_or_default();
        if fields.len() > 32
            || fields
                .iter()
                .any(|(k, v)| !field_name_ok(k) || !field_ok(v))
        {
            bad.push("extensionFields");
        }
        if wire
            .text_body
            .as_deref()
            .is_some_and(|t| t.len() > 100_000 || t.contains('\0'))
        {
            bad.push("textBody");
        }
        if !bad.is_empty() {
            return Err(SetError::invalid_properties(bad));
        }
        Ok(Self {
            for_email_id: wire.for_email_id,
            subject: wire.subject,
            text_body: wire.text_body,
            include_original_message: wire.include_original_message.unwrap_or(false),
            reporting_ua: wire.reporting_ua,
            disposition: wire.disposition,
            final_recipient: wire.final_recipient,
            extension_fields: fields,
        })
    }
}

/// `MDN/send` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnSendArguments {
    /// The account.
    pub account_id: AccountId,
    /// The identity that sends the MDNs.
    pub identity_id: IdOrRef<IdentityId>,
    /// Creation id → MDN (parse each with [`MdnCreate::parse`]).
    pub send: BTreeMap<CreationId, Map<String, Value>>,
    /// After success: `#creationId` → patch for the MDN's email (usually
    /// `keywords/$mdnsent: true`).
    pub on_success_update_email: BTreeMap<IdOrRef<EmailId>, PatchObject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MdnSendWire {
    account_id: AccountId,
    identity_id: IdOrRef<IdentityId>,
    send: BTreeMap<CreationId, Map<String, Value>>,
}

impl MdnSendArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let on_success_update_email =
            take_argument(&mut arguments, "onSuccessUpdateEmail")?.unwrap_or_default();
        let wire: MdnSendWire = from_arguments(arguments)?;
        if wire.send.len() > max_objects {
            return Err(MethodError::request_too_large("too many MDNs"));
        }
        Ok(Self {
            account_id: wire.account_id,
            identity_id: wire.identity_id,
            send: wire.send,
            on_success_update_email,
        })
    }
}

/// `MDN/send` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdnSendResponse {
    /// The account.
    pub account_id: AccountId,
    /// Sent MDNs, with what the server set.
    #[serde(serialize_with = "empty_map_as_null")]
    pub sent: BTreeMap<CreationId, MdnSent>,
    /// MDNs not sent, and why.
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_sent: BTreeMap<CreationId, SetError>,
}

/// `MDN/parse` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MdnParseArguments {
    /// The account.
    pub account_id: AccountId,
    /// The blobs (whole messages) to read.
    pub blob_ids: Vec<BlobId>,
}

impl MdnParseArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.blob_ids.len() > max_objects {
            return Err(MethodError::request_too_large("too many blobs"));
        }
        Ok(parsed)
    }
}

/// `MDN/parse` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdnParseResponse {
    /// The account.
    pub account_id: AccountId,
    /// Blob → its MDN.
    #[serde(serialize_with = "empty_map_as_null")]
    pub parsed: BTreeMap<BlobId, Mdn>,
    /// Blobs that are not MDNs.
    #[serde(serialize_with = "crate::types::serde_util::empty_vec_as_null")]
    pub not_parsable: Vec<BlobId>,
    /// Blobs that don't exist.
    #[serde(serialize_with = "crate::types::serde_util::empty_vec_as_null")]
    pub not_found: Vec<BlobId>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// RFC 9007 §2.1.2 (the request).
    #[test]
    fn rfc_send_example() {
        let args = MdnSendArguments::parse(
            map(json!({
                "accountId": "ue150411c",
                "identityId": "I64588216",
                "send": {
                    "k1546": {
                        "forEmailId": "Md45b47b4877521042cec0938",
                        "subject": "Read receipt for: World domination",
                        "textBody": "This receipt shows that the email has been\r\ndisplayed on your recipient's computer. ...",
                        "reportingUA": "joes-pc.cs.example.com; Foomail 97.1",
                        "disposition": {
                            "actionMode": "manual-action",
                            "sendingMode": "mdn-sent-manually",
                            "type": "displayed"
                        },
                        "extensionFields": {"X-EXTENSION-EXAMPLE": "example.com"}
                    }
                },
                "onSuccessUpdateEmail": {"#k1546": {"keywords/$mdnsent": true}}
            })),
            10,
        )
        .unwrap();
        assert_eq!(args.on_success_update_email.len(), 1);
        let (_, object) = args.send.into_iter().next().unwrap();
        let mdn = MdnCreate::parse(object).unwrap();
        assert_eq!(mdn.disposition.kind, DispositionType::Displayed);
        assert_eq!(mdn.extension_fields.len(), 1);
        assert!(!mdn.include_original_message);
    }

    #[test]
    fn rejects_injection_and_server_set() {
        let base = || {
            json!({
                "forEmailId": "e1",
                "disposition": {"actionMode": "manual-action",
                                "sendingMode": "mdn-sent-manually", "type": "displayed"}
            })
        };
        let with = |key: &str, value: Value| {
            let mut object = base();
            object[key] = value;
            MdnCreate::parse(map(object))
        };
        assert!(MdnCreate::parse(map(base())).is_ok());
        for (key, value) in [
            ("subject", json!("hi\r\nBcc: evil@example.org")),
            ("reportingUA", json!("ua\n")),
            ("finalRecipient", json!("bob@example.org")),
            ("extensionFields", json!({"Disposition": "x"})),
            ("extensionFields", json!({"X-A": "a\r\nB: c"})),
            ("extensionFields", json!({"X A": "a"})),
            ("originalMessageId", json!("<a@b>")),
            ("mdnGateway", json!("x")),
        ] {
            let error = with(key, value).unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{key}");
        }
        assert!(with("unknown", json!(1)).is_err());
    }

    #[test]
    fn parse_arguments_and_shapes() {
        let args =
            MdnParseArguments::parse(map(json!({"accountId": "a", "blobIds": ["b1", "b2"]})), 10)
                .unwrap();
        assert_eq!(args.blob_ids.len(), 2);
        assert!(
            MdnParseArguments::parse(map(json!({"accountId": "a", "blobIds": ["b1", "b2"]})), 1)
                .is_err()
        );
        let response = MdnSendResponse {
            account_id: "a".parse().unwrap(),
            sent: BTreeMap::new(),
            not_sent: BTreeMap::new(),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"accountId": "a", "sent": null, "notSent": null})
        );
        let sent = MdnSent {
            final_recipient: Some("rfc822;alice@example.com".to_owned()),
            ..MdnSent::default()
        };
        assert_eq!(
            serde_json::to_value(&sent).unwrap(),
            json!({"finalRecipient": "rfc822;alice@example.com", "originalRecipient": null,
                   "originalMessageId": null, "mdnGateway": null})
        );
    }
}
