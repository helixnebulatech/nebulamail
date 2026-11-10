// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `EmailSubmission` (RFC 8621 §7): sending an Email.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::macros::{serde_via_str, string_enum};
use crate::method::parse::{PropertyErrors, take_argument};
use crate::method::patch::{Patch, Target};
use crate::method::{
    FilterCondition, PatchObject, QueryArguments, QueryChangesArguments, SetArguments, SortProperty,
};
use crate::references::IdOrRef;
use crate::types::{BlobId, EmailId, EmailSubmissionId, IdentityId, ThreadId, UtcDate};

/// Why an envelope address or parameter is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EnvelopeError {
    /// The address has spaces, `<>`, control characters, no `@`, or is
    /// longer than 254 bytes.
    #[error("invalid SMTP address")]
    Address,
    /// The parameter name is not an `esmtp-keyword`.
    #[error("invalid SMTP parameter name")]
    ParameterName,
    /// The parameter value is not an `esmtp-value`.
    #[error("invalid SMTP parameter value")]
    ParameterValue,
}

/// An SMTP envelope address (`MAIL FROM`/`RCPT TO` path, RFC 5321).
///
/// Checked so it can be written into an SMTP command safely: no spaces,
/// angle brackets or control characters (no CRLF command injection), at
/// most 254 bytes, and an `@` unless empty. Empty is only meaningful as
/// the null reverse-path of `mailFrom`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvelopeAddress(String);

impl EnvelopeAddress {
    /// Longest address accepted (RFC 5321 §4.5.3.1.3 path limit minus `<>`).
    pub const MAX_LEN: usize = 254;

    /// The address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for EnvelopeAddress {
    type Err = EnvelopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad_char = |c: char| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>');
        let valid = s.is_empty()
            || (s.len() <= Self::MAX_LEN && s.contains('@') && !s.chars().any(bad_char));
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(EnvelopeError::Address)
        }
    }
}

impl fmt::Display for EnvelopeAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for EnvelopeAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EnvelopeAddress").field(&self.0).finish()
    }
}

serde_via_str!(EnvelopeAddress);

/// An ESMTP parameter name (`esmtp-keyword`, RFC 5321 §4.1.2), uppercased:
/// `HOLDFOR`, `NOTIFY`, `SIZE`…
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SmtpParameterName(String);

impl SmtpParameterName {
    /// The name, uppercase.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SmtpParameterName {
    type Err = EnvelopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = s.bytes();
        let valid = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && s.len() <= 64;
        if valid {
            Ok(Self(s.to_ascii_uppercase()))
        } else {
            Err(EnvelopeError::ParameterName)
        }
    }
}

impl fmt::Display for SmtpParameterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SmtpParameterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SmtpParameterName").field(&self.0).finish()
    }
}

serde_via_str!(SmtpParameterName);

/// An ESMTP parameter value (`esmtp-value`): no spaces, `=` or control
/// characters (UTF-8 allowed, RFC 6531).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SmtpParameterValue(String);

impl SmtpParameterValue {
    /// The value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SmtpParameterValue {
    type Err = EnvelopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad_char = |c: char| c.is_control() || c.is_whitespace() || c == '=';
        if s.is_empty() || s.len() > 500 || s.chars().any(bad_char) {
            return Err(EnvelopeError::ParameterValue);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for SmtpParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SmtpParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SmtpParameterValue").field(&self.0).finish()
    }
}

serde_via_str!(SmtpParameterValue);

/// ESMTP parameters of an address: name → value (`None` for parameters
/// without a value, e.g. `SMTPUTF8`).
pub type SmtpParameters = BTreeMap<SmtpParameterName, Option<SmtpParameterValue>>;

/// An envelope address with its parameters (RFC 8621 §7 `Address`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionAddress {
    /// The address.
    pub email: EnvelopeAddress,
    /// Parameters, or `None` (`null`).
    #[serde(default)]
    pub parameters: Option<SmtpParameters>,
}

/// The SMTP envelope (RFC 8621 §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope {
    /// `MAIL FROM`.
    pub mail_from: SubmissionAddress,
    /// `RCPT TO` (one per recipient).
    pub rcpt_to: Vec<SubmissionAddress>,
}

string_enum! {
    /// Whether the submission can still be canceled.
    pub enum UndoStatus ("undo status") {
        /// Not sent yet; can be canceled.
        Pending = "pending",
        /// Sent (or being sent); can't be canceled.
        Final = "final",
        /// Canceled.
        Canceled = "canceled",
    }
}

string_enum! {
    /// `delivered` of a `DeliveryStatus`.
    pub enum Delivered ("delivery state") {
        /// Still in the queue.
        Queued = "queued",
        /// Delivered.
        Yes = "yes",
        /// Delivery failed.
        No = "no",
        /// Unknown (e.g. relayed to a server without DSN).
        Unknown = "unknown",
    }
}

string_enum! {
    /// `displayed` of a `DeliveryStatus` (from MDNs).
    pub enum Displayed ("display state") {
        /// Not known.
        Unknown = "unknown",
        /// The recipient displayed it.
        Yes = "yes",
    }
}

/// Delivery status for one recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStatus {
    /// The SMTP reply (e.g. `250 2.0.0 OK`).
    pub smtp_reply: String,
    /// Delivered?
    pub delivered: Delivered,
    /// Displayed?
    pub displayed: Displayed,
}

string_enum! {
    /// `EmailSubmission` properties.
    pub enum EmailSubmissionProperty ("EmailSubmission property") {
        /// `id`.
        Id = "id",
        /// `identityId`.
        IdentityId = "identityId",
        /// `emailId`.
        EmailId = "emailId",
        /// `threadId`.
        ThreadId = "threadId",
        /// `envelope`.
        Envelope = "envelope",
        /// `sendAt`.
        SendAt = "sendAt",
        /// `undoStatus`.
        UndoStatus = "undoStatus",
        /// `deliveryStatus`.
        DeliveryStatus = "deliveryStatus",
        /// `dsnBlobIds`.
        DsnBlobIds = "dsnBlobIds",
        /// `mdnBlobIds`.
        MdnBlobIds = "mdnBlobIds",
        /// [`SEND_WHEN_SIGNED_OUT_PROPERTY`] (the bridge's vendor property).
        SendWhenSignedOut = "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut",
    }
}

/// Vendor property of the bridge (capability
/// `https://helixnebula.tech/jmap/bridge`, only when it announces
/// `sendWhenSignedOut`): a held submission goes out even if the user signs
/// out before it is due (the bridge keeps the password, sealed, until
/// then). A boolean, `false` by default and on submissions no longer
/// pending. Set on create, or by an update while `undoStatus` is
/// `pending` (as a patch key, the slashes are `~1`).
pub const SEND_WHEN_SIGNED_OUT_PROPERTY: &str =
    "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut";

/// An `EmailSubmission` as returned by `EmailSubmission/get` (and in
/// `created`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmission {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<EmailSubmissionId>,
    /// `identityId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<IdentityId>,
    /// `emailId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_id: Option<EmailId>,
    /// `threadId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// `envelope` (`null`: taken from the headers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Option<Envelope>>,
    /// `sendAt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_at: Option<UtcDate>,
    /// `undoStatus`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<UndoStatus>,
    /// `deliveryStatus`: recipient → status (`null` if unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<Option<BTreeMap<String, DeliveryStatus>>>,
    /// `dsnBlobIds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsn_blob_ids: Option<Vec<BlobId>>,
    /// `mdnBlobIds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mdn_blob_ids: Option<Vec<BlobId>>,
    /// [`SEND_WHEN_SIGNED_OUT_PROPERTY`] (vendor).
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut",
        skip_serializing_if = "Option::is_none"
    )]
    pub send_when_signed_out: Option<bool>,
}

/// An `EmailSubmission` to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSubmissionCreate {
    /// The identity to send as.
    pub identity_id: IdOrRef<IdentityId>,
    /// The Email to send (often `#creationId` of a draft in the same
    /// request).
    pub email_id: IdOrRef<EmailId>,
    /// The envelope; `None`: built from the Email's headers.
    pub envelope: Option<Envelope>,
    /// [`SEND_WHEN_SIGNED_OUT_PROPERTY`]; `false` if absent.
    pub send_when_signed_out: bool,
}

const SERVER_SET: &[&str] = &[
    "id",
    "threadId",
    "sendAt",
    "undoStatus",
    "deliveryStatus",
    "dsnBlobIds",
    "mdnBlobIds",
];

impl EmailSubmissionCreate {
    /// Parses one object of `EmailSubmission/set` `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every problem.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut identity_id = None;
        let mut email_id = None;
        let mut envelope = None;
        let mut send_when_signed_out = None;
        for (key, value) in object {
            match key.as_str() {
                "identityId" => identity_id = errors.parse(&key, value),
                SEND_WHEN_SIGNED_OUT_PROPERTY => send_when_signed_out = errors.parse(&key, value),
                "emailId" => email_id = errors.parse(&key, value),
                "envelope" => envelope = errors.parse::<Option<Envelope>>(&key, value),
                server_set if SERVER_SET.contains(&server_set) => {
                    errors.invalid_because(server_set, "set by the server");
                }
                _ => errors.invalid(key),
            }
        }
        let identity_id = errors.required("identityId", identity_id);
        let email_id = errors.required("emailId", email_id);
        errors.finish()?;
        match (identity_id, email_id) {
            (Some(identity_id), Some(email_id)) => Ok(Self {
                identity_id,
                email_id,
                envelope: envelope.flatten(),
                send_when_signed_out: send_when_signed_out.unwrap_or(false),
            }),
            // `required` recorded the error, so `finish` returned above.
            _ => Err(SetError::invalid_properties(["identityId", "emailId"])),
        }
    }
}

/// An `EmailSubmission/set` update: the only changes allowed are
/// canceling (`{"undoStatus": "canceled"}`) and
/// [`SEND_WHEN_SIGNED_OUT_PROPERTY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailSubmissionPatch {
    /// Cancel a pending submission (whatever else the patch sets).
    Cancel,
    /// Set [`SEND_WHEN_SIGNED_OUT_PROPERTY`] of a pending submission.
    SendWhenSignedOut(bool),
    /// An empty patch: nothing to do.
    Nothing,
}

impl EmailSubmissionPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidProperties` for anything other than `undoStatus:
    /// "canceled"` and a boolean [`SEND_WHEN_SIGNED_OUT_PROPERTY`].
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut cancel = false;
        let mut flag = None;
        for Patch { key, path, value } in patch.patches()? {
            match (Target::of(&path), &value) {
                (Target::Property("undoStatus"), Value::String(status)) if status == "canceled" => {
                    cancel = true;
                }
                (Target::Property(SEND_WHEN_SIGNED_OUT_PROPERTY), Value::Bool(on)) => {
                    flag = Some(*on);
                }
                _ => {
                    return Err(SetError::invalid_properties([key]).with_description(
                        "only undoStatus: \"canceled\" and sendWhenSignedOut can be set",
                    ));
                }
            }
        }
        Ok(match (cancel, flag) {
            (true, _) => Self::Cancel,
            (false, Some(on)) => Self::SendWhenSignedOut(on),
            (false, None) => Self::Nothing,
        })
    }
}

/// `EmailSubmission/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSubmissionSetArguments {
    /// The standard arguments.
    pub set: SetArguments<EmailSubmissionId>,
    /// After success: submission id (or `#creationId`) → patch for its
    /// Email (parse with `EmailPatch::parse`).
    pub on_success_update_email: BTreeMap<IdOrRef<EmailSubmissionId>, PatchObject>,
    /// After success: submissions whose Email to destroy.
    pub on_success_destroy_email: Vec<IdOrRef<EmailSubmissionId>>,
}

impl EmailSubmissionSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let on_success_update_email =
            take_argument(&mut arguments, "onSuccessUpdateEmail")?.unwrap_or_default();
        let on_success_destroy_email =
            take_argument(&mut arguments, "onSuccessDestroyEmail")?.unwrap_or_default();
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            on_success_update_email,
            on_success_destroy_email,
        })
    }
}

/// `EmailSubmission` `FilterCondition` (RFC 8621 §7.3). Every member is
/// optional; all given ones must match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailSubmissionFilterCondition {
    /// `identityId` is one of these.
    #[serde(default)]
    pub identity_ids: Option<Vec<IdentityId>>,
    /// `emailId` is one of these.
    #[serde(default)]
    pub email_ids: Option<Vec<EmailId>>,
    /// `threadId` is one of these.
    #[serde(default)]
    pub thread_ids: Option<Vec<ThreadId>>,
    /// `undoStatus` is this.
    #[serde(default)]
    pub undo_status: Option<UndoStatus>,
    /// `sendAt` before this.
    #[serde(default)]
    pub before: Option<UtcDate>,
    /// `sendAt` at or after this.
    #[serde(default)]
    pub after: Option<UtcDate>,
}

impl FilterCondition for EmailSubmissionFilterCondition {
    const PROPERTIES: &'static [&'static str] = &[
        "identityIds",
        "emailIds",
        "threadIds",
        "undoStatus",
        "before",
        "after",
    ];
}

/// `EmailSubmission` sort properties (RFC 8621 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailSubmissionSort {
    /// `emailId`.
    EmailId,
    /// `threadId`.
    ThreadId,
    /// `sentAt` (the `sendAt` property).
    SentAt,
}

impl SortProperty for EmailSubmissionSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        match property {
            "emailId" => Ok(Self::EmailId),
            "threadId" => Ok(Self::ThreadId),
            "sentAt" => Ok(Self::SentAt),
            other => Err(MethodError::unsupported_sort(format!(
                "cannot sort submissions by {other:?}"
            ))),
        }
    }
}

/// `EmailSubmission/query` arguments: only the standard ones.
pub type EmailSubmissionQueryArguments =
    QueryArguments<EmailSubmissionFilterCondition, EmailSubmissionSort, EmailSubmissionId>;

/// `EmailSubmission/queryChanges` arguments: only the standard ones.
pub type EmailSubmissionQueryChangesArguments =
    QueryChangesArguments<EmailSubmissionFilterCondition, EmailSubmissionSort, EmailSubmissionId>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;
    use crate::object::EmailPatch;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// RFC 8621 §7.5.1 request.
    #[test]
    fn rfc_set_example() {
        let args = EmailSubmissionSetArguments::parse(
            map(json!({
                "accountId": "ue411d190",
                "create": {
                    "k1490": {
                        "identityId": "I64588216",
                        "emailId": "M7f6ed5bcfd7e2604d1753f6c",
                        "envelope": {
                            "mailFrom": {"email": "john@example.com", "parameters": null},
                            "rcptTo": [{"email": "jane@example.com", "parameters": null}]
                        }
                    }
                },
                "onSuccessUpdateEmail": {
                    "#k1490": {
                        "mailboxIds/7cb4e8ee-df87-4757-b9c4-2ea1ca41b38e": null,
                        "mailboxIds/73dbcb4b-bffc-48bd-8c2a-a2e91ca672f6": true,
                        "keywords/$draft": null
                    }
                }
            })),
            10,
        );
        // `7cb4e8ee-…` style ids are valid JMAP ids.
        let args = args.unwrap();
        let (submission, patch) = args.on_success_update_email.into_iter().next().unwrap();
        assert_eq!(submission, IdOrRef::Ref("k1490".parse().unwrap()));
        assert!(EmailPatch::parse(&patch).unwrap().mailbox_ids.is_some());
        let create =
            EmailSubmissionCreate::parse(args.set.create.into_values().next().unwrap()).unwrap();
        let envelope = create.envelope.unwrap();
        assert_eq!(envelope.rcpt_to[0].email.as_str(), "jane@example.com");
    }

    /// What the web client sends (compose.ts `sendEmail`).
    #[test]
    fn web_client_submission() {
        let create = EmailSubmissionCreate::parse(map(json!({
            "emailId": "#draft",
            "envelope": {
                "mailFrom": {"email": "alice@localhost", "parameters": {"HOLDFOR": "30"}},
                "rcptTo": [{"email": "bob@localhost"}]
            },
            "identityId": "i1"
        })))
        .unwrap();
        assert_eq!(create.email_id, IdOrRef::Ref("draft".parse().unwrap()));
        assert!(!create.send_when_signed_out);
        let parameters = create.envelope.unwrap().mail_from.parameters.unwrap();
        assert_eq!(
            parameters
                .get(&"holdfor".parse().unwrap())
                .unwrap()
                .as_ref()
                .unwrap()
                .as_str(),
            "30"
        );
    }

    #[test]
    fn send_when_signed_out_on_create() {
        let create = EmailSubmissionCreate::parse(map(json!({
            "emailId": "e", "identityId": "i", SEND_WHEN_SIGNED_OUT_PROPERTY: true
        })))
        .unwrap();
        assert!(create.send_when_signed_out);
        let error = EmailSubmissionCreate::parse(map(json!({
            "emailId": "e", "identityId": "i", SEND_WHEN_SIGNED_OUT_PROPERTY: "yes"
        })))
        .unwrap_err();
        assert_eq!(error.properties(), [SEND_WHEN_SIGNED_OUT_PROPERTY]);
        assert_eq!(
            SEND_WHEN_SIGNED_OUT_PROPERTY
                .parse::<EmailSubmissionProperty>()
                .ok(),
            Some(EmailSubmissionProperty::SendWhenSignedOut)
        );
        let shown = EmailSubmission {
            send_when_signed_out: Some(true),
            ..EmailSubmission::default()
        };
        assert_eq!(
            serde_json::to_value(&shown).unwrap(),
            json!({SEND_WHEN_SIGNED_OUT_PROPERTY: true})
        );
    }

    #[test]
    fn rejects_smtp_injection() {
        for envelope in [
            json!({"mailFrom": {"email": "a@b\r\nRCPT TO:<x@y>"}, "rcptTo": []}),
            json!({"mailFrom": {"email": "a@b"}, "rcptTo": [{"email": "<x@y>"}]}),
            json!({"mailFrom": {"email": "a@b"}, "rcptTo": [{"email": "x y@z"}]}),
            json!({"mailFrom": {"email": "a@b"}, "rcptTo": [{"email": "nobody"}]}),
            json!({"mailFrom": {"email": "a@b", "parameters": {"A B": null}}, "rcptTo": []}),
            json!({"mailFrom": {"email": "a@b", "parameters": {"X": "a b"}}, "rcptTo": []}),
            json!({"mailFrom": {"email": "a@b", "parameters": {"X": "a\r\n"}}, "rcptTo": []}),
        ] {
            let error = EmailSubmissionCreate::parse(map(json!({
                "emailId": "e", "identityId": "i", "envelope": envelope
            })))
            .unwrap_err();
            assert_eq!(error.properties(), ["envelope"]);
        }
        // Null reverse-path.
        assert!(
            EmailSubmissionCreate::parse(map(json!({
                "emailId": "e", "identityId": "i",
                "envelope": {"mailFrom": {"email": ""}, "rcptTo": [{"email": "a@b"}]}
            })))
            .is_ok()
        );
    }

    #[test]
    fn create_errors_and_patch() {
        let error =
            EmailSubmissionCreate::parse(map(json!({"undoStatus": "pending"}))).unwrap_err();
        assert_eq!(error.properties(), ["undoStatus", "identityId", "emailId"]);
        let cancel: PatchObject =
            serde_json::from_value(json!({"undoStatus": "canceled"})).unwrap();
        assert_eq!(
            EmailSubmissionPatch::parse(&cancel).unwrap(),
            EmailSubmissionPatch::Cancel
        );
        // The vendor property, as a JSON pointer (`~1` for `/`).
        let key = SEND_WHEN_SIGNED_OUT_PROPERTY.replace('/', "~1");
        let flag: PatchObject = serde_json::from_value(json!({key.clone(): true})).unwrap();
        assert_eq!(
            EmailSubmissionPatch::parse(&flag).unwrap(),
            EmailSubmissionPatch::SendWhenSignedOut(true)
        );
        let both: PatchObject =
            serde_json::from_value(json!({key.clone(): false, "undoStatus": "canceled"})).unwrap();
        assert_eq!(
            EmailSubmissionPatch::parse(&both).unwrap(),
            EmailSubmissionPatch::Cancel
        );
        for bad in [
            json!({key.clone(): "yes"}),
            json!({key: null}),
            json!({SEND_WHEN_SIGNED_OUT_PROPERTY: true}),
        ] {
            let patch: PatchObject = serde_json::from_value(bad.clone()).unwrap();
            assert!(EmailSubmissionPatch::parse(&patch).is_err(), "{bad}");
        }
        let other: PatchObject = serde_json::from_value(json!({"undoStatus": "final"})).unwrap();
        assert_eq!(
            EmailSubmissionPatch::parse(&other).unwrap_err().kind(),
            SetErrorType::InvalidProperties
        );
    }

    #[test]
    fn response_shape() {
        let submission = EmailSubmission {
            id: Some("s1".parse().unwrap()),
            undo_status: Some(UndoStatus::Final),
            send_at: Some("2026-10-24T10:00:00Z".parse().unwrap()),
            delivery_status: Some(Some(
                [(
                    "bob@localhost".to_owned(),
                    DeliveryStatus {
                        smtp_reply: "250 2.0.0 OK".to_owned(),
                        delivered: Delivered::Unknown,
                        displayed: Displayed::Unknown,
                    },
                )]
                .into(),
            )),
            envelope: Some(None),
            ..EmailSubmission::default()
        };
        assert_eq!(
            serde_json::to_value(&submission).unwrap(),
            json!({
                "id": "s1", "envelope": null, "sendAt": "2026-10-24T10:00:00Z",
                "undoStatus": "final",
                "deliveryStatus": {"bob@localhost": {
                    "smtpReply": "250 2.0.0 OK", "delivered": "unknown", "displayed": "unknown"
                }}
            })
        );
    }

    /// RFC 8621 §7.3 conditions and sorts, and what the web asks.
    #[test]
    fn query_arguments() {
        use crate::error::MethodErrorType;
        use crate::method::Filter;
        let args = EmailSubmissionQueryArguments::parse(map(json!({
            "accountId": "a",
            "filter": {"emailIds": ["e1", "e2"], "undoStatus": "final",
                       "after": "2026-10-24T10:00:00Z"},
            "sort": [{"property": "sentAt", "isAscending": false}, {"property": "emailId"}],
            "calculateTotal": true
        })))
        .unwrap();
        let Some(Filter::Condition(condition)) = args.filter else {
            panic!("expected a condition")
        };
        assert_eq!(condition.email_ids.unwrap().len(), 2);
        assert_eq!(condition.undo_status, Some(UndoStatus::Final));
        assert!(condition.after.is_some());
        assert_eq!(args.sort[0].property, EmailSubmissionSort::SentAt);
        assert!(!args.sort[0].is_ascending);
        let kind = |value: Value| {
            EmailSubmissionQueryArguments::parse(map(value))
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "a", "filter": {"envelope": {}}})),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(
            kind(json!({"accountId": "a", "sort": [{"property": "sendAt"}]})),
            MethodErrorType::UnsupportedSort
        );
        assert_eq!(
            kind(json!({"accountId": "a", "filter": {"undoStatus": "gone"}})),
            MethodErrorType::InvalidArguments
        );
        let changes = EmailSubmissionQueryChangesArguments::parse(map(json!({
            "accountId": "a", "sinceQueryState": "3", "filter": {"threadIds": ["t1"]}
        })))
        .unwrap();
        assert_eq!(changes.since_query_state.as_str(), "3");
    }
}
