// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/parse` (RFC 8621 §4.9): a blob read as a message
//! (`message/rfc822`, RFC 5322 with RFC 6532 headers), shown as an Email
//! without storing it.
//!
//! - The blob may be an upload, a stored message (`bm…`) or one of its
//!   parts (`bp…`, `bs…`: an attached message), or a part of a message
//!   parsed this way (`bn…`).
//! - The list properties come from the same parsing the sync uses
//!   (`nebula_mail::parse_headers`); the body ones from `Email/get`'s.
//! - The parts' blob ids are `bn{part}_{blob}` (part `part` of the message
//!   in `blob`); for a stored message they are its own `bp…` ids.
//! - `id`, `mailboxIds`, `keywords` and `receivedAt` are `null` (§4.9),
//!   and so is `threadId` ("MAY be present if the server can calculate"
//!   it: the bridge doesn't).
//! - `hasAttachment` is whether `attachments` has a part, as for the
//!   message lists of `mail-parser` (RFC 8621 §4.1.4 leaves it to the
//!   server).
//! - A blob `mail-parser` can't read, or with no header field at all (a
//!   picture is not a message), is `notParsable`.

use std::collections::BTreeMap;
use std::sync::Arc;

use nebula_jmap_proto::object::Email;
use nebula_jmap_proto::object::email::{
    DEFAULT_PARSE_PROPERTIES, EmailBodyPartProperty, EmailField, EmailParseArguments,
    EmailParseResponse, EmailProperty,
};
use nebula_jmap_proto::{BlobId, MethodError, MethodErrorType, MethodResponse, UnsignedInt};
use nebula_mail::ParsedMessage;
use serde_json::{Map, Value};

use super::body::{self, PartOwner};
use super::get::{self, Request};
use crate::blob::BlobRef;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// Properties that are always `null` here (RFC 8621 §4.9).
const NULL_FIELDS: &[(EmailField, &str)] = &[
    (EmailField::Id, "id"),
    (EmailField::MailboxIds, "mailboxIds"),
    (EmailField::Keywords, "keywords"),
    (EmailField::ReceivedAt, "receivedAt"),
    (EmailField::ThreadId, "threadId"),
];

fn request(args: &EmailParseArguments) -> Request {
    let properties = args.properties.clone().unwrap_or_else(|| {
        DEFAULT_PARSE_PROPERTIES
            .iter()
            .map(|f| EmailProperty::Field(*f))
            .collect()
    });
    Request {
        properties,
        body_properties: args.body_properties.clone().unwrap_or_else(|| {
            EmailBodyPartProperty::DEFAULT
                .iter()
                .map(|f| EmailBodyPartProperty::Field(*f))
                .collect()
        }),
        fetch_text: args.fetch_body_values.text,
        fetch_html: args.fetch_body_values.html,
        fetch_all: args.fetch_body_values.all,
        max_body_bytes: args
            .max_body_value_bytes
            .map_or(0, |n| usize::try_from(n.get()).unwrap_or(usize::MAX)),
    }
}

/// The list properties, from the message's own header fields.
fn summary(raw: &[u8], message: &ParsedMessage<'_>, request: &Request, out: &mut Email) {
    let wants = |f| request.wants(f);
    let headers = nebula_mail::parse_headers(raw);
    let list = |v: &Vec<String>| (!v.is_empty()).then(|| v.clone());
    if wants(EmailField::MessageId) {
        out.message_id = Some(headers.message_id.clone().map(|id| vec![id]));
    }
    if wants(EmailField::InReplyTo) {
        out.in_reply_to = Some(list(&headers.in_reply_to));
    }
    if wants(EmailField::References) {
        out.references = Some(list(&headers.references));
    }
    if wants(EmailField::Sender) {
        out.sender = Some(headers.sender.clone());
    }
    if wants(EmailField::From) {
        out.from = Some(headers.from.clone());
    }
    if wants(EmailField::To) {
        out.to = Some(headers.to.clone());
    }
    if wants(EmailField::Cc) {
        out.cc = Some(headers.cc.clone());
    }
    if wants(EmailField::Bcc) {
        out.bcc = Some(headers.bcc.clone());
    }
    if wants(EmailField::ReplyTo) {
        out.reply_to = Some(headers.reply_to.clone());
    }
    if wants(EmailField::Subject) {
        out.subject = Some(headers.subject.clone());
    }
    if wants(EmailField::SentAt) {
        out.sent_at = Some(headers.sent_at.and_then(get::sent_date));
    }
    if wants(EmailField::HasAttachment) {
        out.has_attachment = Some(!message.attachments().is_empty());
    }
    if wants(EmailField::Preview) {
        out.preview = Some(nebula_mail::message_preview(raw).unwrap_or_default());
    }
    if wants(EmailField::Size) {
        out.size = Some(UnsignedInt::try_from(raw.len()).unwrap_or(UnsignedInt::MAX));
    }
}

/// The Email in `raw` (blob `blob`), as JSON; `None` if it isn't a
/// message. CPU work: run off the async workers.
fn parse_blob(
    blob: &BlobRef,
    raw: &[u8],
    request: &Request,
) -> Result<Option<Map<String, Value>>, Bug> {
    let Some(message) = body::parse(raw) else {
        return Ok(None);
    };
    if message.message().headers().iter().next().is_none() {
        return Ok(None);
    }
    // A stored message's parts keep their own ids (`bp…`).
    let owner = match blob {
        BlobRef::Message(email) => PartOwner::Email(*email),
        other => PartOwner::Blob(other),
    };
    let mut out = Email::default();
    if request.wants(EmailField::BlobId) {
        out.blob_id = Some(blob.to_blob_id()?);
    }
    summary(raw, &message, request, &mut out);
    get::parsed_bodies(owner, &message, request, &mut out)?;
    let Ok(Value::Object(mut object)) = serde_json::to_value(&out) else {
        return Err(Bug("parsed email"));
    };
    for (field, name) in NULL_FIELDS {
        if request.wants(*field) {
            object.insert((*name).to_owned(), Value::Null);
        }
    }
    Ok(Some(object))
}

pub(crate) async fn parse(
    ctx: &Ctx<'_>,
    args: EmailParseArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let request = Arc::new(request(&args));
    let mut parsed: BTreeMap<BlobId, Map<String, Value>> = BTreeMap::new();
    let mut not_parsable = Vec::new();
    let mut not_found = Vec::new();
    for blob_id in args.blob_ids {
        if parsed.contains_key(&blob_id)
            || not_parsable.contains(&blob_id)
            || not_found.contains(&blob_id)
        {
            continue;
        }
        let Some(blob) = BlobRef::parse(&blob_id) else {
            not_found.push(blob_id);
            continue;
        };
        // Held while this blob is: its memory is counted.
        let mut lease = ctx.mail()?.lease();
        let bytes = match super::build::blob_bytes(ctx, &blob_id, &mut lease).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                not_found.push(blob_id);
                continue;
            }
            Err(error) => {
                return Err(MethodError::new(MethodErrorType::ServerUnavailable)
                    .with_description(
                        error
                            .description()
                            .unwrap_or("the blob could not be read; try again later")
                            .to_owned(),
                    )
                    .into());
            }
        };
        let request = Arc::clone(&request);
        let found = tokio::task::spawn_blocking(move || parse_blob(&blob, &bytes, &request))
            .await
            .map_err(|_| Bug("Email/parse"))??;
        drop(lease);
        match found {
            Some(email) => {
                parsed.insert(blob_id, email);
            }
            None => not_parsable.push(blob_id),
        }
    }
    Ok(MethodResponse::EmailParse(EmailParseResponse {
        account_id: ctx.jmap_id(),
        parsed,
        not_parsable,
        not_found,
    }))
}

#[cfg(test)]
mod tests {
    use nebula_store::UploadId;
    use serde_json::json;

    use super::*;

    const MESSAGE: &[u8] = b"From: Ana <ana@e.c>\r\n\
To: bob@e.c\r\n\
Subject: =?utf-8?q?Hola?= \r\n\
Date: Thu, 24 Oct 2026 10:00:00 +0200\r\n\
Message-ID: <m1@e.c>\r\n\
Content-Type: multipart/mixed; boundary=b\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
Hola mundo\r\n\
--b\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=a.pdf\r\n\
\r\n\
PDF\r\n\
--b--\r\n";

    fn args(value: Value) -> EmailParseArguments {
        EmailParseArguments::parse(serde_json::from_value(value).unwrap(), 10).unwrap()
    }

    #[test]
    fn a_blob_as_an_email() {
        let upload = BlobRef::Upload(UploadId::from_raw(7).unwrap());
        let request = request(&args(json!({
            "accountId": "a", "blobIds": ["bu7"],
            "properties": ["id", "blobId", "threadId", "mailboxIds", "keywords", "receivedAt",
                "size", "subject", "from", "sentAt", "messageId", "hasAttachment", "preview",
                "textBody", "attachments", "bodyValues", "header:Subject:asText"],
            "fetchTextBodyValues": true
        })));
        let email = Value::Object(parse_blob(&upload, MESSAGE, &request).unwrap().unwrap());
        for null in ["id", "threadId", "mailboxIds", "keywords", "receivedAt"] {
            assert_eq!(email[null], Value::Null, "{null}");
        }
        assert_eq!(email["blobId"], "bu7");
        assert_eq!(email["size"], MESSAGE.len());
        assert_eq!(email["subject"], "Hola ");
        assert_eq!(email["header:Subject:asText"], "Hola ");
        assert_eq!(email["from"], json!([{"name": "Ana", "email": "ana@e.c"}]));
        assert_eq!(email["sentAt"], "2026-10-24T10:00:00+02:00");
        assert_eq!(email["messageId"], json!(["m1@e.c"]));
        assert_eq!(email["hasAttachment"], true);
        assert_eq!(email["preview"], "Hola mundo");
        // Parts are `bn{part}_{blob}`.
        assert_eq!(email["textBody"][0]["blobId"], "bn1_bu7");
        assert_eq!(email["attachments"][0]["blobId"], "bn2_bu7");
        assert_eq!(email["attachments"][0]["name"], "a.pdf");
        assert_eq!(email["bodyValues"]["1"]["value"], "Hola mundo");
        // Not asked for: not there.
        assert!(email.get("to").is_none());
    }

    #[test]
    fn defaults_and_stored_messages() {
        let request = request(&args(json!({"accountId": "a", "blobIds": ["bm3"]})));
        let message = BlobRef::Message(nebula_store::EmailId::from_raw(3).unwrap());
        let email = Value::Object(parse_blob(&message, MESSAGE, &request).unwrap().unwrap());
        // RFC 8621 §4.9 default properties.
        for name in [
            "messageId",
            "inReplyTo",
            "references",
            "sender",
            "from",
            "to",
            "cc",
            "bcc",
            "replyTo",
            "subject",
            "sentAt",
            "hasAttachment",
            "preview",
            "bodyValues",
            "textBody",
            "htmlBody",
            "attachments",
        ] {
            assert!(email.get(name).is_some(), "{name}");
        }
        assert!(email.get("id").is_none());
        // A stored message's parts keep their `bp…` ids.
        assert_eq!(email["textBody"][0]["blobId"], "bp3_1");
    }

    #[test]
    fn not_a_message() {
        let request = request(&args(json!({"accountId": "a", "blobIds": ["bu1"]})));
        let upload = BlobRef::Upload(UploadId::from_raw(1).unwrap());
        assert_eq!(
            parse_blob(&upload, b"\x89PNG\r\n\x1a\n\0\0", &request).unwrap(),
            None
        );
    }
}
