// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/get` (RFC 8621 §4.2).
//!
//! The list properties come from the store. Bodies, `headers` and header
//! fields that the sync doesn't keep need the raw message: it is fetched
//! (`BODY.PEEK[]`, cached in memory) only when one of those is asked for.
//! A preview the server didn't give (no `PREVIEW` extension) is computed
//! from the first bytes of the text part only, for all the emails of the
//! call at once.
//!
//! Parsing the message (its MIME tree, the parts it decodes, the body
//! values) is CPU work, tens of milliseconds to seconds for one near the
//! 50 MB limit: it runs on a blocking thread with the message, the request
//! and its memory lease owned, never on the async workers
//! ([`parse_off_runtime`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mail_parser::{Headers, MessageParser};
use nebula_jmap_proto::method::GetResponse;
use nebula_jmap_proto::object::Email;
use nebula_jmap_proto::object::email::{
    BodyPartField, EmailBodyPart, EmailBodyPartProperty, EmailField, EmailGetArguments,
    EmailProperty, HeaderProperties, HeaderProperty,
};
use nebula_jmap_proto::types::TrueSet;
use nebula_jmap_proto::{
    Date, EmailId as JmapEmailId, MethodError, MethodResponse, UnsignedInt, UtcDate,
};
use nebula_mail::{
    Copies, LIST_FIELDS, Lease, MailError, MessageStructure, ParsedMessage, PartSection,
    header_block_is_complete,
};
use nebula_store::{DataType, Email as StoredEmail, EmailId, EmailQuery, QueryStart, SentAt};
use time::{OffsetDateTime, UtcOffset};

use super::body::{self, PartBuilder, PartOwner};
use super::headers;
use crate::blob::BlobRef;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

/// The properties that need the raw message.
const BODY_FIELDS: &[EmailField] = &[
    EmailField::BodyStructure,
    EmailField::BodyValues,
    EmailField::TextBody,
    EmailField::HtmlBody,
    EmailField::Attachments,
    EmailField::Headers,
];

pub(crate) fn utc_date(secs: i64) -> Result<UtcDate, Bug> {
    UtcDate::from_unix_timestamp(secs).map_err(|_| Bug("date"))
}

pub(crate) fn sent_date(sent: SentAt) -> Option<Date> {
    let offset =
        UtcOffset::from_whole_seconds(i32::from(sent.offset_minutes).saturating_mul(60)).ok()?;
    let time = OffsetDateTime::from_unix_timestamp(sent.utc.secs()).ok()?;
    Date::new(time.checked_to_offset(offset)?).ok()
}

/// Whether the stored header block can answer this header property: the
/// sync keeps only [`LIST_FIELDS`], and drops fields from very big blocks
/// (a block without its final blank line).
fn stored_block_has(email: &StoredEmail, property: &HeaderProperty) -> bool {
    block_has(&email.meta.headers, property)
}

fn block_has(block: &[u8], property: &HeaderProperty) -> bool {
    header_block_is_complete(block) && LIST_FIELDS.iter().any(|field| property.matches(field))
}

/// What one `Email/get` (or `Email/parse`) asks for.
pub(crate) struct Request {
    pub(crate) properties: Vec<EmailProperty>,
    pub(crate) body_properties: Vec<EmailBodyPartProperty>,
    pub(crate) fetch_text: bool,
    pub(crate) fetch_html: bool,
    pub(crate) fetch_all: bool,
    pub(crate) max_body_bytes: usize,
}

impl Request {
    pub(crate) fn wants(&self, field: EmailField) -> bool {
        self.properties.contains(&EmailProperty::Field(field))
    }

    fn headers(&self) -> impl Iterator<Item = &HeaderProperty> {
        self.properties.iter().filter_map(|p| match p {
            EmailProperty::Header(header) => Some(header),
            _ => None,
        })
    }

    fn needs_raw(&self, email: &StoredEmail) -> bool {
        BODY_FIELDS.iter().any(|f| self.wants(*f))
            || self.headers().any(|h| !stored_block_has(email, h))
    }
}

/// The properties from the store.
fn metadata(email: &StoredEmail, request: &Request, out: &mut Email) -> Result<(), Bug> {
    let meta = &email.meta;
    let wants = |f| request.wants(f);
    let list = |v: &Vec<String>| (!v.is_empty()).then(|| v.clone());
    if wants(EmailField::BlobId) {
        out.blob_id = Some(BlobRef::Message(email.id).to_blob_id()?);
    }
    if wants(EmailField::ThreadId) {
        out.thread_id = Some(ids::thread_id(email.thread)?);
    }
    if wants(EmailField::MailboxIds) {
        out.mailbox_ids = Some(
            email
                .mailboxes
                .iter()
                .map(|m| ids::mailbox_id(*m))
                .collect::<Result<TrueSet<_>, _>>()?,
        );
    }
    if wants(EmailField::Keywords) {
        out.keywords = Some(email.keywords.iter().cloned().collect());
    }
    if wants(EmailField::Size) {
        out.size = Some(UnsignedInt::saturating(meta.size));
    }
    if wants(EmailField::ReceivedAt) {
        out.received_at = Some(utc_date(meta.received_at.secs())?);
    }
    if wants(EmailField::MessageId) {
        out.message_id = Some(meta.message_id.clone().map(|id| vec![id]));
    }
    if wants(EmailField::InReplyTo) {
        out.in_reply_to = Some(list(&meta.in_reply_to));
    }
    if wants(EmailField::References) {
        out.references = Some(list(&meta.references));
    }
    if wants(EmailField::Sender) {
        out.sender = Some(meta.sender.clone());
    }
    if wants(EmailField::From) {
        out.from = Some(meta.from.clone());
    }
    if wants(EmailField::To) {
        out.to = Some(meta.to.clone());
    }
    if wants(EmailField::Cc) {
        out.cc = Some(meta.cc.clone());
    }
    if wants(EmailField::Bcc) {
        out.bcc = Some(meta.bcc.clone());
    }
    if wants(EmailField::ReplyTo) {
        out.reply_to = Some(meta.reply_to.clone());
    }
    if wants(EmailField::Subject) {
        out.subject = Some(meta.subject.clone());
    }
    if wants(EmailField::SentAt) {
        out.sent_at = Some(meta.sent_at.and_then(sent_date));
    }
    if wants(EmailField::HasAttachment) {
        out.has_attachment = Some(meta.has_attachment);
    }
    if wants(EmailField::Preview) {
        out.preview = Some(meta.preview.clone().unwrap_or_default());
    }
    Ok(())
}

/// These store properties of one email, as `Email/get` gives them (for
/// Web Push's `EmailPush`; no body, nothing fetched).
pub(crate) fn listed(email: &StoredEmail, fields: &[EmailField]) -> Result<Email, Bug> {
    let request = Request {
        properties: fields.iter().map(|f| EmailProperty::Field(*f)).collect(),
        body_properties: Vec::new(),
        fetch_text: false,
        fetch_html: false,
        fetch_all: false,
        max_body_bytes: 0,
    };
    let mut out = Email {
        id: request
            .wants(EmailField::Id)
            .then(|| ids::email_id(email.id))
            .transpose()?,
        ..Email::default()
    };
    metadata(email, &request, &mut out)?;
    Ok(out)
}

/// Body properties from the parsed message.
fn bodies(
    owner: PartOwner<'_>,
    message: &ParsedMessage<'_>,
    sections: &BTreeMap<u32, PartSection>,
    request: &Request,
    out: &mut Email,
) -> Result<(), Bug> {
    let builder = PartBuilder {
        message,
        owner,
        properties: &request.body_properties,
        sections,
    };
    if request.wants(EmailField::BodyStructure) {
        // `bodyStructure` always shows the tree, whatever bodyProperties say.
        let mut with_children = request.body_properties.clone();
        let sub_parts = EmailBodyPartProperty::Field(BodyPartField::SubParts);
        if !with_children.contains(&sub_parts) {
            with_children.push(sub_parts);
        }
        let tree = PartBuilder {
            properties: &with_children,
            ..builder
        };
        out.body_structure = tree.part(0, 0)?;
    }
    if request.wants(EmailField::TextBody) {
        out.text_body = Some(builder.list(&message.text_body())?);
    }
    if request.wants(EmailField::HtmlBody) {
        out.html_body = Some(builder.list(&message.html_body())?);
    }
    if request.wants(EmailField::Attachments) {
        out.attachments = Some(builder.list(&message.attachments())?);
    }
    if request.wants(EmailField::BodyValues) {
        let mut parts = BTreeSet::new();
        if request.fetch_all {
            parts.extend(body::text_parts(message));
        }
        if request.fetch_text {
            parts.extend(message.text_body());
        }
        if request.fetch_html {
            parts.extend(message.html_body());
        }
        out.body_values = Some(body::values(message, parts, request.max_body_bytes)?);
    }
    if request.wants(EmailField::Headers) {
        out.headers = Some(headers::all_raw(message.message().headers()));
    }
    Ok(())
}

fn header_properties(block: Headers<'_>, request: &Request) -> HeaderProperties {
    let mut out = HeaderProperties::new();
    for property in request.headers() {
        out.insert(property.clone(), headers::property(block, property));
    }
    out
}

/// The server's structure of a big message, for the parts that can be
/// streamed from their IMAP section (their blob ids say so). Small
/// messages are downloaded whole and cached, so they keep the plain part
/// ids: `None`. Without the server's structure (an error, or too deeply
/// nested) every part keeps its plain id too: downloads then cut it from
/// the whole message, as before.
async fn structure(
    ctx: &Ctx<'_>,
    email: EmailId,
    len: usize,
    request: &Request,
) -> Option<MessageStructure> {
    let wants_parts = [
        EmailField::BodyStructure,
        EmailField::TextBody,
        EmailField::HtmlBody,
        EmailField::Attachments,
    ]
    .into_iter()
    .any(|field| request.wants(field));
    let blob_ids = EmailBodyPartProperty::Field(BodyPartField::BlobId);
    if !wants_parts || !request.body_properties.contains(&blob_ids) {
        return None;
    }
    let mail = ctx.mail().ok()?;
    if u64::try_from(len).is_ok_and(|len| len <= mail.stream_threshold()) {
        return None;
    }
    match mail.body_structure(email).await {
        Ok(structure) => structure,
        Err(error) => {
            tracing::debug!(error = %error, "no BODYSTRUCTURE; parts keep their plain ids");
            None
        }
    }
}

/// The body and `header:` properties of `raw` into `out`; `false` if it
/// can't be parsed (then `out` is as it was). CPU work: for a 50 MB
/// message, parsing decodes every part (tens to hundreds of ms), so it runs
/// in [`parse_off_runtime`], never on the async workers.
fn parse_into(
    email: EmailId,
    raw: &[u8],
    structure: Option<&MessageStructure>,
    request: &Request,
    out: &mut Email,
) -> Result<bool, Bug> {
    let Some(message) = body::parse(raw) else {
        return Ok(false);
    };
    let sections = structure.map_or_else(BTreeMap::new, |structure| {
        nebula_mail::streamable_parts(&message, structure)
    });
    bodies(PartOwner::Email(email), &message, &sections, request, out)?;
    out.header_properties = header_properties(message.message().headers(), request);
    Ok(true)
}

/// The body and `header:` properties of a parsed message that is not a
/// stored email (`Email/parse`): its parts' blob ids say whose parts they
/// are (`owner`).
pub(crate) fn parsed_bodies(
    owner: PartOwner<'_>,
    message: &ParsedMessage<'_>,
    request: &Request,
    out: &mut Email,
) -> Result<(), Bug> {
    bodies(owner, message, &BTreeMap::new(), request, out)?;
    out.header_properties = header_properties(message.message().headers(), request);
    Ok(())
}

/// [`parse_into`] on a blocking thread, with everything it needs owned.
/// `lease` (the message and what parsing it makes) goes with it and is
/// given back only once the parse is over: if this future is dropped (the
/// request was cancelled), the thread still runs to the end and the
/// memory stays counted until then, never less.
async fn parse_off_runtime(
    email: EmailId,
    raw: Arc<[u8]>,
    structure: Option<MessageStructure>,
    request: Arc<Request>,
    mut out: Email,
    lease: Lease,
) -> Result<(Email, bool), Bug> {
    tokio::task::spawn_blocking(move || {
        let parsed = parse_into(email, &raw, structure.as_ref(), &request, &mut out);
        // The message first, then its share of the budget.
        drop(raw);
        drop(lease);
        parsed.map(|parsed| (out, parsed))
    })
    .await
    .map_err(|_| Bug("Email/get parse task"))?
}

/// One Email, or `None` if its message is gone from the server.
async fn build(
    ctx: &Ctx<'_>,
    mut email: StoredEmail,
    request: &Arc<Request>,
) -> Result<Option<Email>, CallError> {
    let mut out = Email {
        id: Some(ids::email_id(email.id)?),
        ..Email::default()
    };
    if request.needs_raw(&email) {
        // The message and what parsing it makes, until this email is done.
        let mail = ctx.mail()?;
        let mut lease = mail.lease();
        let raw = match mail.load_message(email.id, &mut lease, Copies::Two).await {
            Ok(raw) => raw,
            Err(MailError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // Loaded: the other big loads of the account may go on.
        lease.drop_turn();
        if email.meta.preview.is_none()
            && let Some(fresh) = ctx.store().emails(ctx.id(), vec![email.id]).await?.pop()
        {
            // `load_message` filled in the preview.
            email.meta.preview = fresh.meta.preview;
        }
        let structure = structure(ctx, email.id, raw.len(), request).await;
        let len = raw.len();
        let (parsed_out, parsed) =
            parse_off_runtime(email.id, raw, structure, Arc::clone(request), out, lease).await?;
        out = parsed_out;
        if !parsed {
            // Not a message mail-parser can read (or it panicked): the rest
            // of the batch must not fail because of it.
            tracing::warn!(
                email = email.id.get(),
                "message can't be parsed; shown as one attachment"
            );
            unparsable(email.id, len, request, &mut out)?;
            stored_header_properties(&email, request, &mut out);
        }
    } else {
        stored_header_properties(&email, request, &mut out);
    }
    metadata(&email, request, &mut out)?;
    Ok(Some(out))
}

/// The `header:` properties from the header block the sync stored.
fn stored_header_properties(email: &StoredEmail, request: &Request, out: &mut Email) {
    if request.headers().next().is_none() {
        return;
    }
    let block = MessageParser::default().parse_headers(email.meta.headers.as_slice());
    if let Some(block) = block {
        out.header_properties = header_properties(block.headers(), request);
    }
}

/// The body properties of a message that can't be parsed: one part, the
/// whole message as an attachment (`application/octet-stream`, its
/// `bm…` blob), so the client can still download it. RFC 8621 has no
/// `null` `bodyStructure`.
fn unparsable(email: EmailId, size: usize, request: &Request, out: &mut Email) -> Result<(), Bug> {
    let wants = |field| {
        request
            .body_properties
            .contains(&EmailBodyPartProperty::Field(field))
    };
    let mut part = EmailBodyPart::default();
    if wants(BodyPartField::PartId) {
        part.part_id = Some(None);
    }
    if wants(BodyPartField::BlobId) {
        part.blob_id = Some(Some(BlobRef::Message(email).to_blob_id()?));
    }
    if wants(BodyPartField::Size) {
        part.size = Some(UnsignedInt::try_from(size).unwrap_or(UnsignedInt::MAX));
    }
    if wants(BodyPartField::Headers) {
        part.headers = Some(Vec::new());
    }
    if wants(BodyPartField::Name) {
        part.name = Some(None);
    }
    if wants(BodyPartField::Type) {
        part.media_type = Some("application/octet-stream".to_owned());
    }
    if wants(BodyPartField::Charset) {
        part.charset = Some(None);
    }
    if wants(BodyPartField::Disposition) {
        part.disposition = Some(Some("attachment".to_owned()));
    }
    if wants(BodyPartField::Cid) {
        part.cid = Some(None);
    }
    if wants(BodyPartField::Language) {
        part.language = Some(None);
    }
    if wants(BodyPartField::Location) {
        part.location = Some(None);
    }
    if request.wants(EmailField::BodyStructure) {
        // `bodyStructure` always says whether a part has children.
        out.body_structure = Some(EmailBodyPart {
            sub_parts: Some(None),
            ..part.clone()
        });
    }
    if request.wants(EmailField::TextBody) {
        out.text_body = Some(Vec::new());
    }
    if request.wants(EmailField::HtmlBody) {
        out.html_body = Some(Vec::new());
    }
    if request.wants(EmailField::Attachments) {
        out.attachments = Some(vec![part]);
    }
    if request.wants(EmailField::BodyValues) {
        out.body_values = Some(BTreeMap::new());
    }
    if request.wants(EmailField::Headers) {
        out.headers = Some(Vec::new());
    }
    Ok(())
}

/// Missing previews, unless the raw message is fetched anyway (then
/// `load_message` fills them in). A failure only leaves them empty.
async fn fill_previews(ctx: &Ctx<'_>, request: &Request, stored: &mut [StoredEmail]) {
    if !request.wants(EmailField::Preview) {
        return;
    }
    let missing: Vec<EmailId> = stored
        .iter()
        .filter(|e| e.meta.preview.is_none() && !request.needs_raw(e))
        .map(|e| e.id)
        .collect();
    if missing.is_empty() {
        return;
    }
    let found = match ctx.mail() {
        Ok(mail) => mail.fill_previews(&missing).await,
        Err(_) => return,
    };
    match found {
        Ok(mut found) => {
            for email in stored.iter_mut() {
                if let Some(preview) = found.remove(&email.id) {
                    email.meta.preview = Some(preview);
                }
            }
        }
        Err(error) => tracing::warn!(error = %error, "previews could not be computed"),
    }
}

/// The ids to get: the ones asked for, or every email (if not too many).
async fn wanted_ids(
    ctx: &Ctx<'_>,
    asked: Option<&Vec<nebula_jmap_proto::IdOrRef<JmapEmailId>>>,
) -> Result<Vec<JmapEmailId>, CallError> {
    if let Some(asked) = asked {
        return Ok(asked.iter().filter_map(|id| ctx.resolve(id)).collect());
    }
    let max = ctx.limits.max_objects_in_get;
    let all = ctx
        .store()
        .query_emails(
            ctx.id(),
            EmailQuery {
                filter: None,
                sort: Vec::new(),
                collapse_threads: false,
                start: QueryStart::Position(0),
                limit: Some(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1)),
            },
        )
        .await?;
    if all.ids.len() > max {
        return Err(MethodError::request_too_large("too many emails for ids: null").into());
    }
    all.ids
        .into_iter()
        .map(|id| ids::email_id(id).map_err(CallError::from))
        .collect()
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: EmailGetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.get.account_id)?;
    let state = ctx.state(DataType::Email).await?;
    let mut properties = args.get.properties.clone().unwrap_or_else(|| {
        EmailProperty::DEFAULT
            .iter()
            .map(|f| EmailProperty::Field(*f))
            .collect()
    });
    properties.retain(|p| *p != EmailProperty::Field(EmailField::Id));
    let request = Arc::new(Request {
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
            .map_or(0, UnsignedInt::to_usize_saturating),
    });
    let wanted = wanted_ids(ctx, args.get.ids.as_ref()).await?;
    let rows: Vec<EmailId> = wanted.iter().filter_map(ids::store_email).collect();
    let mut stored = ctx.store().emails(ctx.id(), rows).await?;
    fill_previews(ctx, &request, &mut stored).await;
    let mut list = Vec::with_capacity(stored.len());
    let mut found = BTreeSet::new();
    for email in stored {
        let id = email.id;
        if let Some(object) = build(ctx, email, &request).await? {
            found.insert(id);
            list.push(object);
        }
    }
    let not_found = wanted
        .into_iter()
        .filter(|id| ids::store_email(id).is_none_or(|row| !found.contains(&row)))
        .collect();
    Ok(MethodResponse::EmailGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}

#[cfg(test)]
mod tests {
    use nebula_store::UnixTime;

    use super::*;

    /// A message with two base64 attachments of `size` bytes and a text.
    fn big_message(size: usize) -> Vec<u8> {
        let line = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAx\r\n";
        // 57 bytes a line once decoded.
        let body = line.repeat(size / 57);
        format!(
            "From: Ana <ana@{domain}>\r\nSubject: grande\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
             --b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHola.\r\n\
             --b\r\nContent-Type: application/octet-stream\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{body}\
             --b\r\nContent-Type: application/octet-stream\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{body}--b--\r\n",
            domain = "example.com"
        )
        .into_bytes()
    }

    fn body_request() -> Arc<Request> {
        Arc::new(Request {
            properties: [
                EmailField::BodyStructure,
                EmailField::TextBody,
                EmailField::Attachments,
                EmailField::BodyValues,
            ]
            .into_iter()
            .map(EmailProperty::Field)
            .collect(),
            body_properties: EmailBodyPartProperty::DEFAULT
                .iter()
                .map(|f| EmailBodyPartProperty::Field(*f))
                .collect(),
            fetch_text: true,
            fetch_html: false,
            fetch_all: false,
            max_body_bytes: 0,
        })
    }

    const MIB: u64 = 1024 * 1024;

    fn len_of(raw: &[u8]) -> u64 {
        u64::try_from(raw.len()).unwrap()
    }

    /// The parse of a big message runs on a blocking thread: another task
    /// of a single-threaded runtime keeps running meanwhile, and the result
    /// is the same as parsing it right here.
    #[tokio::test(flavor = "current_thread")]
    async fn a_big_parse_does_not_block_the_runtime() {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};

        let raw: Arc<[u8]> = Arc::from(big_message(12_000_000));
        let request = body_request();
        let email = EmailId::from_raw(1).unwrap();
        // On this thread: how long it takes, and what it gives.
        let started = Instant::now();
        let mut expected = Email::default();
        assert!(parse_into(email, &raw, None, &request, &mut expected).unwrap());
        let alone = started.elapsed();
        assert!(
            alone >= Duration::from_millis(40),
            "{alone:?}: too quick to tell"
        );
        assert_eq!(expected.attachments.as_ref().map(Vec::len), Some(2));

        let worst = Arc::new(Mutex::new(Duration::ZERO));
        let ticker = tokio::spawn({
            let worst = Arc::clone(&worst);
            async move {
                let mut last = Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let now = Instant::now();
                    let mut worst = worst.lock().unwrap();
                    *worst = (*worst).max(now - last);
                    last = now;
                }
            }
        });
        // Let the ticker start.
        tokio::time::sleep(Duration::from_millis(10)).await;
        *worst.lock().unwrap() = Duration::ZERO;
        let budget = nebula_mail::MemoryBudget::new(
            256 * MIB,
            Duration::from_secs(30),
            Duration::from_secs(30),
        );
        let mut lease = budget.lease();
        lease.reserve(2 * len_of(&raw)).await.unwrap();
        let started = Instant::now();
        let (out, parsed) = parse_off_runtime(
            email,
            Arc::clone(&raw),
            None,
            Arc::clone(&request),
            Email::default(),
            lease,
        )
        .await
        .unwrap();
        let took = started.elapsed();
        // The ticker notes a stall when it runs again: let it.
        tokio::time::sleep(Duration::from_millis(20)).await;
        ticker.abort();
        let worst = *worst.lock().unwrap();
        assert!(parsed);
        assert_eq!(out, expected);
        assert_eq!(budget.available(), 256 * MIB, "the lease is given back");
        eprintln!("parse: {alone:?} alone, {took:?} off the runtime; longest tick gap {worst:?}");
        assert!(
            worst < alone / 2,
            "the runtime stalled {worst:?} during a parse of {alone:?}"
        );
    }

    /// A cancelled `Email/get` (its future dropped) can't stop the parse
    /// already running: its memory stays counted until the parse ends, and
    /// is all given back then.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancelled_parse_keeps_its_memory_until_it_ends() {
        use std::time::Duration;

        let raw: Arc<[u8]> = Arc::from(big_message(12_000_000));
        let budget = nebula_mail::MemoryBudget::new(
            256 * MIB,
            Duration::from_secs(30),
            Duration::from_secs(30),
        );
        let mut lease = budget.lease();
        lease.reserve(2 * len_of(&raw)).await.unwrap();
        let held = budget.available();
        assert!(held < 256 * MIB);
        let task = tokio::spawn(parse_off_runtime(
            EmailId::from_raw(1).unwrap(),
            raw,
            None,
            body_request(),
            Email::default(),
            lease,
        ));
        tokio::time::sleep(Duration::from_millis(5)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(budget.available(), held, "still counted while it parses");
        let mut freed = false;
        for _ in 0..600 {
            if budget.available() == 256 * MIB {
                freed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(freed, "given back when the parse ends");
    }

    #[test]
    fn sent_dates_keep_the_offset() {
        let date = sent_date(SentAt {
            utc: UnixTime::from_secs(1_792_836_000),
            offset_minutes: 120,
        })
        .unwrap();
        assert_eq!(date.to_string(), "2026-10-24T12:00:00+02:00");
    }

    #[test]
    fn an_unparsable_message_is_one_attachment() {
        assert!(body::parse(b"").is_none());
        let field = |f| EmailProperty::Field(f);
        let request = Request {
            properties: vec![
                field(EmailField::BodyStructure),
                field(EmailField::TextBody),
                field(EmailField::HtmlBody),
                field(EmailField::Attachments),
                field(EmailField::BodyValues),
                field(EmailField::Headers),
            ],
            body_properties: EmailBodyPartProperty::DEFAULT
                .iter()
                .map(|f| EmailBodyPartProperty::Field(*f))
                .collect(),
            fetch_text: true,
            fetch_html: false,
            fetch_all: false,
            max_body_bytes: 0,
        };
        let email = EmailId::from_raw(7).unwrap();
        let mut out = Email::default();
        unparsable(email, 1234, &request, &mut out).unwrap();
        let json = serde_json::to_value(&out).unwrap();
        let structure = &json["bodyStructure"];
        assert!(structure.is_object(), "{json}");
        assert_eq!(structure["type"], "application/octet-stream");
        assert_eq!(structure["size"], 1234);
        assert_eq!(structure["subParts"], serde_json::Value::Null);
        let blob = BlobRef::Message(email).to_blob_id().unwrap();
        assert_eq!(structure["blobId"], serde_json::json!(blob));
        assert_eq!(json["attachments"][0]["blobId"], serde_json::json!(blob));
        assert_eq!(json["attachments"][0]["disposition"], "attachment");
        assert_eq!(json["textBody"], serde_json::json!([]));
        assert_eq!(json["htmlBody"], serde_json::json!([]));
        assert_eq!(json["bodyValues"], serde_json::json!({}));
        assert_eq!(json["headers"], serde_json::json!([]));
    }

    #[test]
    fn sent_dates_out_of_range_give_none() {
        // 9999-12-31T23:59:59Z at +01:00 would be year 10000.
        let last = sent_date(SentAt {
            utc: UnixTime::from_secs(253_402_300_799),
            offset_minutes: 60,
        });
        assert_eq!(last, None);
        let late = sent_date(SentAt {
            utc: UnixTime::from_secs(i64::MAX),
            offset_minutes: 0,
        });
        assert_eq!(late, None);
    }

    #[test]
    fn cut_blocks_answer_no_header() {
        let from: HeaderProperty = "header:From:asAddresses".parse().unwrap();
        let other: HeaderProperty = "header:X-Spam".parse().unwrap();
        assert!(block_has(b"From: a@x\r\n\r\n", &from));
        assert!(!block_has(b"From: a@x\r\n\r\n", &other));
        // Fields were dropped (no blank line at the end): ask the server.
        assert!(!block_has(b"From: a@x\r\nSubject: s\r\n", &from));
    }
}
