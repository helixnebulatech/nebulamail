// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `MDN/send` and `MDN/parse` (RFC 9007, reports from RFC 8098).
//!
//! - **send**: the email must ask for one (`Disposition-Notification-To`)
//!   and not have `$mdnsent`. The report (`multipart/report`) goes only to
//!   the addresses in that header, from the identity's address, through the
//!   same SMTP path as `EmailSubmission/set` (one connection, `AUTH`).
//!   `Final-Recipient` is the identity unless the client gives one;
//!   `Original-Message-ID` and `Original-Recipient` come from the email.
//!   `onSuccessUpdateEmail` (the client sets `$mdnsent` with it) runs as an
//!   implicit `Email/set`. The report is not saved in Sent.
//! - **parse**: reads `message/disposition-notification` parts of blobs;
//!   `forEmailId` is the email with that `Original-Message-ID`, if any.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as BuilderAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::text::Text;
use mail_builder::mime::{BodyPart, MimePart};
use mail_parser::{HeaderForm, PartKind};
use nebula_jmap_proto::method::{PatchObject, SetResponse};
use nebula_jmap_proto::object::{
    ActionMode, Disposition, DispositionType, Mdn, MdnCreate, MdnParseArguments, MdnParseResponse,
    MdnSendArguments, MdnSendResponse, MdnSent, SendingMode,
};
use nebula_jmap_proto::{
    BlobId, CreationId, EmailId as JmapEmailId, IdOrRef, MethodResponse, SetError, SetErrorType,
};
use nebula_mail::ParsedMessage;
use nebula_mail::{Copies, MailAccount};
use nebula_smtp::{Envelope, MailFrom, OutgoingMessage, RcptTo};
use nebula_store::{DataType, Identity};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::email::build::blob_bytes;
use crate::email::encapsulate::{self, Encapsulation};
use crate::email::set::update_and_destroy;
use crate::error::{Bug, CallError, mail_set_error, store_set_error};
use crate::identity::identities;
use crate::submission::send::{self, set_error};

/// Most addresses an MDN goes to (a `Disposition-Notification-To` with more
/// is suspicious: it would turn the user into a mailing list).
const MAX_MDN_RECIPIENTS: usize = 5;

fn invalid(property: &str, why: &str) -> SetError {
    SetError::invalid_properties([property]).with_description(why.to_owned())
}

/// What the email says about the MDN it wants.
struct Request {
    /// `Disposition-Notification-To` addresses.
    to: Vec<String>,
    /// `Message-ID` with angle brackets.
    message_id: Option<String>,
    /// `Original-Recipient` header (RFC 8098 §2.3), if the MTA added one.
    original_recipient: Option<String>,
}

fn one_line(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= 998 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

fn read_request(raw: &[u8]) -> Option<Request> {
    let parsed = ParsedMessage::parse(raw)?;
    let message = parsed.message();
    // mail-parser keeps unknown fields raw: read it as an address list.
    let mut to = Vec::new();
    if let Some(header) = message.headers().get("Disposition-Notification-To") {
        let value = header.parse_as(HeaderForm::Addresses);
        if let Some(list) = value.value().as_address() {
            for mailbox in list.mailboxes() {
                if let Some(email) = mailbox.address() {
                    to.push(email.trim().to_owned());
                }
            }
        }
    }
    let message_id = message
        .message_id()
        .and_then(one_line)
        .map(|id| format!("<{id}>"));
    let original_recipient = message
        .headers()
        .get("Original-Recipient")
        .map(|header| String::from_utf8_lossy(header.raw_value()).into_owned())
        .as_deref()
        .and_then(one_line)
        .filter(|value| value.contains(';'));
    Some(Request {
        to,
        message_id,
        original_recipient,
    })
}

fn disposition_field(disposition: Disposition) -> String {
    format!(
        "{}/{}; {}",
        disposition.action_mode, disposition.sending_mode, disposition.kind
    )
}

/// The `message/disposition-notification` fields (RFC 8098 §3.1).
fn report_fields(mdn: &MdnCreate, final_recipient: &str, request: &Request) -> String {
    let mut fields = String::new();
    if let Some(ua) = &mdn.reporting_ua {
        let _ = write!(fields, "Reporting-UA: {ua}\r\n");
    }
    if let Some(original) = &request.original_recipient {
        let _ = write!(fields, "Original-Recipient: {original}\r\n");
    }
    let _ = write!(fields, "Final-Recipient: {final_recipient}\r\n");
    if let Some(id) = &request.message_id {
        let _ = write!(fields, "Original-Message-ID: {id}\r\n");
    }
    let _ = write!(
        fields,
        "Disposition: {}\r\n",
        disposition_field(mdn.disposition)
    );
    for (name, value) in &mdn.extension_fields {
        let _ = write!(fields, "{name}: {value}\r\n");
    }
    fields
}

fn default_text(mdn: &MdnCreate, subject: &str, final_recipient: &str) -> String {
    let what = match mdn.disposition.kind {
        DispositionType::Deleted => "deleted without being displayed",
        DispositionType::Dispatched => "dispatched",
        DispositionType::Processed => "processed",
        _ => "displayed",
    };
    let address = final_recipient
        .split_once(';')
        .map_or(final_recipient, |(_, a)| a);
    format!(
        "The message sent to {address} with subject \"{subject}\" was {what}.\r\n\
         This is no guarantee that the message has been read or understood.\r\n"
    )
}

/// A random `Message-ID` at the identity's domain (no host name).
fn new_message_id(identity: &Identity) -> Result<String, SetError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let mut local = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(local, "{byte:02x}");
    }
    let domain = identity
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.trim())
        .filter(|d| {
            !d.is_empty()
                && d.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
        .unwrap_or("nebula.invalid");
    Ok(format!("{local}.mdn@{domain}"))
}

/// The third part of the report with `includeOriginalMessage` (RFC 9007
/// §2.1: "the content of the original message"; RFC 8098 §3): the whole
/// message as `message/rfc822`, untouched, when it can go as it is in
/// 7 or 8 bits (RFC 2046 §5.2.1 forbids base64 or QP there); with UTF-8
/// header fields as `message/global` (RFC 6533 §6); anything else (long
/// lines, NUL, bare CR/LF) as `message/global` in base64 (RFC 6532 §3.5),
/// the same bytes inside.
fn original_part(original: &[u8]) -> MimePart<'_> {
    let canonical = encapsulate::canonical(original);
    let (media, body, transfer) = match encapsulate::classify(&canonical) {
        Encapsulation::SevenBit => ("message/rfc822", BodyPart::Binary(canonical), Some("7bit")),
        Encapsulation::EightBit => ("message/rfc822", BodyPart::Binary(canonical), Some("8bit")),
        Encapsulation::GlobalEightBit => {
            ("message/global", BodyPart::Binary(canonical), Some("8bit"))
        }
        // mail-builder writes a non-text part in base64 by itself.
        Encapsulation::Global => ("message/global", BodyPart::Binary(original.into()), None),
    };
    let part = MimePart::new(ContentType::new(media), body);
    match transfer {
        Some(cte) => part.transfer_encoding(cte),
        None => part,
    }
}

/// The report message (RFC 8098 §3).
fn build_report(
    mdn: &MdnCreate,
    identity: &Identity,
    request: &Request,
    original: &[u8],
    original_subject: &str,
    final_recipient: &str,
    now: i64,
) -> Result<Vec<u8>, SetError> {
    let subject = mdn.subject.clone().unwrap_or_else(|| {
        let prefix = match mdn.disposition.kind {
            DispositionType::Displayed => "Read",
            _ => "Disposition notification",
        };
        format!("{prefix}: {original_subject}")
    });
    let text = mdn
        .text_body
        .clone()
        .unwrap_or_else(|| default_text(mdn, original_subject, final_recipient));
    let fields = report_fields(mdn, final_recipient, request);
    // Non-ASCII report fields need the RFC 6533 type.
    let fields_type = if fields.is_ascii() {
        "message/disposition-notification"
    } else {
        "message/global-disposition-notification"
    };
    let mut parts = vec![
        MimePart::new(
            ContentType::new("text/plain").attribute("charset", "utf-8"),
            BodyPart::Text(text.into()),
        ),
        MimePart::new(ContentType::new(fields_type), BodyPart::Text(fields.into())),
    ];
    if mdn.include_original_message {
        parts.push(original_part(original));
    }
    let report = MimePart::new(
        ContentType::new("multipart/report").attribute("report-type", "disposition-notification"),
        BodyPart::Multipart(parts),
    );
    let from = if identity.name.trim().is_empty() {
        BuilderAddress::new_address(None::<String>, identity.email.clone())
    } else {
        BuilderAddress::new_address(Some(identity.name.clone()), identity.email.clone())
    };
    let to: Vec<BuilderAddress<'_>> = request
        .to
        .iter()
        .map(|a| BuilderAddress::new_address(None::<String>, a.clone()))
        .collect();
    let mut builder = MessageBuilder::new()
        .from(from)
        .to(BuilderAddress::new_list(to))
        .subject(Text::new(subject))
        .date(Date::new(now))
        .message_id(MessageId::new(new_message_id(identity)?))
        .body(report);
    if let Some(id) = &request.message_id {
        let bare = id.trim_start_matches('<').trim_end_matches('>').to_owned();
        builder = builder.references(MessageId::new(bare));
    }
    builder
        .write_to_vec()
        .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"))
}

/// Reads what the email asks for, and keeps the message itself only if it
/// goes inside the report (its memory counted in the returned lease).
async fn load_request(
    mail: &MailAccount,
    email: nebula_store::EmailId,
    keep: bool,
) -> Result<(Request, Option<Arc<[u8]>>, nebula_mail::Lease), SetError> {
    let mut lease = mail.lease();
    let raw = mail
        .load_message(email, &mut lease, Copies::One)
        .await
        .map_err(|e| mail_set_error(&e))?;
    lease.drop_turn();
    // Parsing a big message is CPU work: off the async workers.
    let (request, raw) = tokio::task::spawn_blocking(move || read_request(&raw).map(|r| (r, raw)))
        .await
        .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"))?
        .ok_or_else(|| invalid("forEmailId", "unreadable email"))?;
    if keep {
        return Ok((request, Some(raw), lease));
    }
    drop(raw);
    lease.clear();
    Ok((request, None, lease))
}

async fn send_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    identity: &Identity,
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<(JmapEmailId, MdnSent), SetError> {
    let mdn = MdnCreate::parse(object)?;
    let email_jmap = ctx
        .resolve(&mdn.for_email_id)
        .ok_or_else(|| invalid("forEmailId", "unknown email"))?;
    let email = match ids::store_email(&email_jmap) {
        Some(row) => ctx
            .store()
            .emails(ctx.id(), vec![row])
            .await
            .map_err(|e| store_set_error(&e))?
            .pop(),
        None => None,
    }
    .ok_or_else(|| invalid("forEmailId", "unknown email"))?;
    if email.keywords.iter().any(|k| k.as_str() == "$mdnsent") {
        return Err(SetError::new(SetErrorType::MdnAlreadySent));
    }
    let (request, original, _lease) =
        load_request(mail, email.id, mdn.include_original_message).await?;
    if request.to.is_empty() {
        return Err(invalid(
            "forEmailId",
            "the email asks for no MDN (no Disposition-Notification-To)",
        ));
    }
    if request.to.len() > MAX_MDN_RECIPIENTS {
        return Err(SetError::too_many_recipients(
            nebula_jmap_proto::UnsignedInt::try_from(MAX_MDN_RECIPIENTS)
                .unwrap_or(nebula_jmap_proto::UnsignedInt::MAX),
        ));
    }
    let mut rcpt = Vec::new();
    for address in &request.to {
        let parsed: RcptTo = address
            .parse()
            .map_err(|_| SetError::invalid_recipients(vec![address.clone()]))?;
        rcpt.push(parsed);
    }
    let from: MailFrom = identity
        .email
        .trim()
        .parse()
        .map_err(|_| SetError::new(SetErrorType::ForbiddenMailFrom))?;
    let envelope =
        Envelope::new(from, rcpt).map_err(|_| SetError::new(SetErrorType::NoRecipients))?;
    let server_set_final = mdn.final_recipient.is_none();
    let final_recipient = mdn
        .final_recipient
        .clone()
        .unwrap_or_else(|| format!("rfc822;{}", identity.email.trim()));
    let now = Ctx::now().map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let subject = email.meta.subject.clone().unwrap_or_default();
    let report = build_report(
        &mdn,
        identity,
        &request,
        original.as_deref().unwrap_or_default(),
        &subject,
        &final_recipient,
        now.secs(),
    )?;
    let message =
        OutgoingMessage::new(&report).map_err(|_| SetError::new(SetErrorType::InvalidEmail))?;
    let login = ctx.inner.smtp_login(ctx.id()).ok_or_else(|| {
        SetError::new(SetErrorType::ForbiddenToSend)
            .with_description("temporary failure, try again later: not connected")
    })?;
    // No ENVID of our own: read receipts are matched by Message-ID (an MDN
    // carries no ENVID), and nothing matches reports about an MDN.
    let mut lease = mail.lease();
    send::send(
        ctx.inner,
        &login,
        &envelope,
        Arc::new(message),
        &mut lease,
        None,
    )
    .await
    .map_err(|e| set_error(&e))?;
    drop(login);
    tracing::info!(account = ctx.id().get(), "MDN sent");
    Ok((
        email_jmap,
        MdnSent {
            final_recipient: server_set_final.then_some(final_recipient),
            original_recipient: request.original_recipient,
            original_message_id: request.message_id,
            mdn_gateway: None,
            reporting_ua: None,
        },
    ))
}

pub(crate) async fn send(
    ctx: &mut Ctx<'_>,
    args: MdnSendArguments,
) -> Result<Vec<MethodResponse>, CallError> {
    ctx.check_account(&args.account_id)?;
    let mail = ctx.mail()?;
    let all = identities(ctx).await?;
    let identity = ctx
        .resolve(&args.identity_id)
        .as_ref()
        .and_then(ids::store_identity)
        .and_then(|row| all.iter().find(|i| i.id == row))
        .cloned();
    let mut sent = BTreeMap::new();
    let mut not_sent = BTreeMap::new();
    let mut succeeded: BTreeMap<CreationId, JmapEmailId> = BTreeMap::new();
    for (creation_id, object) in args.send {
        let result = match &identity {
            None => Err(invalid("identityId", "unknown identity")),
            Some(identity) => send_one(ctx, &mail, identity, object).await,
        };
        match result {
            Ok((email, object)) => {
                succeeded.insert(creation_id.clone(), email);
                sent.insert(creation_id, object);
            }
            Err(error) => {
                not_sent.insert(creation_id, error);
            }
        }
    }
    let mut responses = vec![MethodResponse::MdnSend(MdnSendResponse {
        account_id: ctx.jmap_id(),
        sent,
        not_sent,
    })];
    // `#creationId` → the email of that MDN.
    let updates: BTreeMap<IdOrRef<JmapEmailId>, PatchObject> = args
        .on_success_update_email
        .into_iter()
        .filter_map(|(key, patch)| match key {
            IdOrRef::Ref(creation) => succeeded
                .get(&creation)
                .map(|email| (IdOrRef::Id(email.clone()), patch)),
            _ => None,
        })
        .collect();
    if !updates.is_empty() {
        let old_state = ctx.state(DataType::Email).await?;
        let changed = update_and_destroy(ctx, &mail, &updates, &[]).await;
        let new_state = ctx.state(DataType::Email).await?;
        let mut implicit = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
        implicit.updated = changed.updated;
        implicit.not_updated = changed.not_updated;
        responses.push(MethodResponse::EmailSet(implicit));
    }
    Ok(responses)
}

/// The fields of a `message/disposition-notification` body.
fn parse_fields(body: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        if line.starts_with([' ', '\t']) {
            // Folded continuation.
            if let Some((_, value)) = fields.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            fields.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    fields
}

fn parse_disposition(value: &str) -> Option<Disposition> {
    let (modes, kind) = value.split_once(';')?;
    let (action, sending) = modes.trim().split_once('/')?;
    let kind = kind.trim().split(['/', ' ']).next()?.to_ascii_lowercase();
    Some(Disposition {
        action_mode: action
            .trim()
            .to_ascii_lowercase()
            .parse::<ActionMode>()
            .ok()?,
        sending_mode: sending
            .trim()
            .to_ascii_lowercase()
            .parse::<SendingMode>()
            .ok()?,
        kind: kind.parse().ok()?,
    })
}

/// Report fields with their own MDN property.
const KNOWN: [&str; 7] = [
    "Reporting-UA",
    "MDN-Gateway",
    "Original-Recipient",
    "Final-Recipient",
    "Original-Message-ID",
    "Disposition",
    "Error",
];

/// The MDN in a message, without `forEmailId`.
fn parse_mdn(raw: &[u8]) -> Option<Mdn> {
    let parsed = ParsedMessage::parse(raw)?;
    let message = parsed.message();
    let mut fields = None;
    let mut text_body = None;
    let mut include_original_message = false;
    for (_, part) in parsed.parts() {
        let Some(kind) = part.content_type() else {
            continue;
        };
        let subtype = kind.subtype().unwrap_or_default().to_ascii_lowercase();
        let full = format!("{}/{subtype}", kind.ctype().to_ascii_lowercase());
        let body = match part.kind() {
            PartKind::Text | PartKind::Html => part.text().map(std::borrow::Cow::into_owned),
            PartKind::Binary | PartKind::InlineBinary => {
                Some(String::from_utf8_lossy(&part.decoded()).into_owned())
            }
            PartKind::Multipart | PartKind::Message(_) => None,
        };
        match full.as_str() {
            "message/disposition-notification" | "message/global-disposition-notification" => {
                fields = body.map(|b| parse_fields(&b));
            }
            "text/plain" if text_body.is_none() => text_body = body,
            "text/rfc822-headers" | "message/rfc822" | "message/global" => {
                include_original_message = true;
            }
            _ => {}
        }
    }
    let fields = fields?;
    let get = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let disposition = parse_disposition(&get("Disposition")?)?;
    let errors: Vec<String> = fields
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("Error"))
        .map(|(_, v)| v.clone())
        .collect();
    let extension: BTreeMap<String, String> = fields
        .iter()
        .filter(|(n, _)| !KNOWN.iter().any(|k| k.eq_ignore_ascii_case(n)))
        .take(64)
        .cloned()
        .collect();
    Some(Mdn {
        for_email_id: None,
        subject: message.subject().map(str::to_owned),
        text_body,
        include_original_message,
        reporting_ua: get("Reporting-UA"),
        disposition,
        mdn_gateway: get("MDN-Gateway"),
        original_recipient: get("Original-Recipient"),
        final_recipient: get("Final-Recipient"),
        original_message_id: get("Original-Message-ID"),
        error: (!errors.is_empty()).then_some(errors),
        extension_fields: (!extension.is_empty()).then_some(extension),
    })
}

/// The email of the account with this `Message-ID` (any mailbox).
async fn email_with_message_id(
    ctx: &Ctx<'_>,
    message_id: &str,
) -> Result<Option<JmapEmailId>, CallError> {
    let bare = message_id
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned();
    if bare.is_empty() {
        return Ok(None);
    }
    for mailbox in ctx.store().mailboxes(ctx.id()).await? {
        let found = ctx
            .store()
            .emails_by_message_id(ctx.id(), mailbox.id, bare.clone())
            .await?;
        if let Some(email) = found.first() {
            return Ok(Some(ids::email_id(*email)?));
        }
    }
    Ok(None)
}

pub(crate) async fn parse(
    ctx: &Ctx<'_>,
    args: MdnParseArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let mut parsed: BTreeMap<BlobId, Mdn> = BTreeMap::new();
    let mut not_parsable = Vec::new();
    let mut not_found = Vec::new();
    for blob in args.blob_ids {
        // Held while this blob is: its memory is counted.
        let Ok(mut lease) = crate::email::build::lease(ctx) else {
            not_found.push(blob);
            continue;
        };
        let Ok(Some(bytes)) = blob_bytes(ctx, &blob, &mut lease).await else {
            not_found.push(blob);
            continue;
        };
        // Parsing a big message is CPU work: off the async workers.
        let report = tokio::task::spawn_blocking(move || parse_mdn(&bytes))
            .await
            .map_err(|_| Bug("MDN parse"))?;
        drop(lease);
        match report {
            Some(mut mdn) => {
                if let Some(id) = mdn.original_message_id.clone() {
                    mdn.for_email_id = email_with_message_id(ctx, &id).await?;
                }
                parsed.insert(blob, mdn);
            }
            None => not_parsable.push(blob),
        }
    }
    Ok(MethodResponse::MdnParse(MdnParseResponse {
        account_id: ctx.jmap_id(),
        parsed,
        not_parsable,
        not_found,
    }))
}

#[cfg(test)]
mod tests {
    use nebula_store::IdentityId;
    use serde_json::json;

    use super::*;

    const REPORT_REQUEST: &[u8] = b"From: Bob <bob@example.org>\r\n\
        To: alice@example.com\r\n\
        Subject: World domination\r\n\
        Message-ID: <199509192301.23456@example.org>\r\n\
        Disposition-Notification-To: Bob <bob@example.org>\r\n\
        Original-Recipient: rfc822;alice@example.com\r\n\r\nHello\r\n";

    fn mdn(extra: &serde_json::Value) -> MdnCreate {
        let mut object = json!({
            "forEmailId": "e1",
            "disposition": {"actionMode": "manual-action",
                            "sendingMode": "mdn-sent-manually", "type": "displayed"}
        });
        if let (Some(target), Some(extra)) = (object.as_object_mut(), extra.as_object()) {
            target.extend(extra.clone());
        }
        MdnCreate::parse(object.as_object().unwrap().clone()).unwrap()
    }

    fn identity() -> Identity {
        Identity {
            id: IdentityId::from_raw(1).unwrap(),
            name: "Alice".to_owned(),
            email: "alice@example.com".to_owned(),
            reply_to: None,
            bcc: None,
            text_signature: String::new(),
            html_signature: String::new(),
            may_delete: false,
        }
    }

    #[test]
    fn reads_the_request() {
        let request = read_request(REPORT_REQUEST).unwrap();
        assert_eq!(request.to, ["bob@example.org"]);
        assert_eq!(
            request.message_id.as_deref(),
            Some("<199509192301.23456@example.org>")
        );
        assert_eq!(
            request.original_recipient.as_deref(),
            Some("rfc822;alice@example.com")
        );
        let none = read_request(b"From: a@b\r\n\r\nx").unwrap();
        assert!(none.to.is_empty());
    }

    #[test]
    fn report_round_trip() {
        let request = read_request(REPORT_REQUEST).unwrap();
        let create = mdn(&json!({
            "reportingUA": "joes-pc.cs.example.com; Foomail 97.1",
            "includeOriginalMessage": true,
            "extensionFields": {"X-EXTENSION-EXAMPLE": "example.com"}
        }));
        let report = build_report(
            &create,
            &identity(),
            &request,
            REPORT_REQUEST,
            "World domination",
            "rfc822;alice@example.com",
            1_700_000_000,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&report);
        assert!(
            text.contains("report-type=\"disposition-notification\"")
                || text.contains("report-type=disposition-notification"),
            "{text}"
        );
        assert!(text.contains("Subject: Read: World domination"), "{text}");
        assert!(text.contains("To: <bob@example.org>"), "{text}");
        assert!(!text.contains("Bcc"));

        let parsed = parse_mdn(&report).unwrap();
        assert_eq!(parsed.disposition, create.disposition);
        assert_eq!(
            parsed.final_recipient.as_deref(),
            Some("rfc822;alice@example.com")
        );
        assert_eq!(
            parsed.original_message_id.as_deref(),
            Some("<199509192301.23456@example.org>")
        );
        assert_eq!(
            parsed.reporting_ua.as_deref(),
            Some("joes-pc.cs.example.com; Foomail 97.1")
        );
        assert!(parsed.include_original_message);
        assert_eq!(
            parsed
                .extension_fields
                .as_ref()
                .and_then(|f| f.get("X-EXTENSION-EXAMPLE"))
                .map(String::as_str),
            Some("example.com")
        );
        assert!(parsed.text_body.unwrap().contains("was displayed"));
        // RFC 9007 §2.1: the original message itself, not only its header.
        assert!(text.contains("Content-Type: message/rfc822"), "{text}");
        let body_start = REPORT_REQUEST
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body = String::from_utf8_lossy(&REPORT_REQUEST[body_start..]);
        assert!(text.contains(body.trim()), "{text}");
    }

    #[test]
    fn dispositions() {
        let parsed = parse_disposition("automatic-action/MDN-sent-automatically; deleted").unwrap();
        assert_eq!(parsed.action_mode, ActionMode::AutomaticAction);
        assert_eq!(parsed.kind, DispositionType::Deleted);
        assert!(parse_disposition("manual-action; displayed").is_none());
        assert!(parse_mdn(b"Subject: x\r\n\r\nnot a report").is_none());
    }
}
