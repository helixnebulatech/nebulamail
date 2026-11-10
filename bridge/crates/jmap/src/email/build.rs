// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Building the MIME message of an `Email/set` create (RFC 8621 §4.6) with
//! `mail-builder`: drafts and the messages the web app sends.
//!
//! - With `bodyStructure`, the tree is built as given.
//! - With `textBody`/`htmlBody`/`attachments`, the server makes the tree:
//!   `multipart/alternative` for text + HTML, `multipart/related` around it
//!   for inline parts with a `cid`, and `multipart/mixed` for the rest.
//! - Blobs come from uploads or from parts of existing emails.
//! - Every header value was checked by `jmap-proto` (no CR/LF injection);
//!   `mail-builder` encodes and folds them.
//! - The `Message-ID` is generated here when the client gives none, with
//!   the domain of the From address: never the bridge's host name.
//! - A forwarded `message/rfc822` goes as it is (RFC 2046 §5.2.1): `7bit`
//!   if ASCII, `8bit` if not (line ends made CRLF); `message/global` in
//!   `8bit` with 8-bit header fields, and in base64 with the original bytes
//!   when it has a NUL or a line over 998 octets. `message/partial` and
//!   `message/external-body` only in 7bit. No charset on any of them.
//! - The boundaries are ours (128 random bits, see [`encapsulate`]), chosen
//!   so that no part written as it is (messages, text) has a `--` line or
//!   declares a boundary that could be taken for one of them; a part that
//!   clashes with every try goes in base64.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use mail_builder::MessageBuilder;
use mail_builder::headers::HeaderType;
use mail_builder::headers::address::Address;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::raw::Raw;
use mail_builder::headers::text::Text;
use mail_builder::headers::url::URL;
use mail_builder::mime::{BodyPart, MimePart};
use nebula_jmap_proto::object::email::{
    BodyPartContent, EmailAddress, EmailBodyPartCreate, EmailCreate, EmailCreateBody,
    HeaderFormValue, HeaderProperties, HeaderValue, PartId,
};
use nebula_jmap_proto::{BlobId, SetError, SetErrorType};

use nebula_mail::{Copies, Lease, MailError};

use super::body;
use super::encapsulate::{self, Delimiters, Encapsulation};
use crate::blob::{BlobRef, ContentKey};
use crate::core::context::Ctx;

/// The bytes of the blobs a body uses. Shared: the same blob used twice,
/// or two ids of the same part (`bp…`/`bs…`), are one copy.
pub(crate) type Blobs = HashMap<BlobId, Arc<[u8]>>;

fn visit_blobs<'a>(part: &'a EmailBodyPartCreate, out: &mut Vec<&'a BlobId>) {
    match &part.content {
        BodyPartContent::Blob(id) => out.push(id),
        BodyPartContent::Multipart(children) => {
            for child in children {
                visit_blobs(child, out);
            }
        }
        BodyPartContent::Value(_) | _ => {}
    }
}

/// Every blob id the body refers to, once each and in order of first
/// use, with the number of parts that use it.
pub(crate) fn blob_uses(create: &EmailCreate) -> Vec<(BlobId, u64)> {
    let mut out = Vec::new();
    match &create.body {
        EmailCreateBody::Structure(part) => visit_blobs(part, &mut out),
        EmailCreateBody::Parts {
            text,
            html,
            attachments,
        } => {
            for part in text
                .as_deref()
                .into_iter()
                .chain(html.as_deref())
                .chain(attachments)
            {
                visit_blobs(part, &mut out);
            }
        }
        _ => {}
    }
    let mut index: HashMap<&BlobId, usize> = HashMap::with_capacity(out.len());
    let mut unique: Vec<(BlobId, u64)> = Vec::with_capacity(out.len());
    for id in out {
        if let Some((_, uses)) = index.get(id).and_then(|at| unique.get_mut(*at)) {
            *uses = uses.saturating_add(1);
        } else {
            index.insert(id, unique.len());
            unique.push((id.clone(), 1));
        }
    }
    unique
}

fn len_of(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

fn too_large() -> SetError {
    SetError::new(SetErrorType::TooLarge)
        .with_description("the message would be larger than maxSizeAttachmentsPerEmail")
}

/// Takes `size` bytes used `uses` times from what is `left`, or
/// `tooLarge` (and `left` stays as it was).
fn charge(left: &mut u64, size: u64, uses: u64) -> Result<(), SetError> {
    *left = left
        .checked_sub(size.saturating_mul(uses))
        .ok_or_else(too_large)?;
    Ok(())
}

/// A lease for loading blobs, `Forbidden` if the account isn't connected.
pub(crate) fn lease(ctx: &Ctx<'_>) -> Result<Lease, SetError> {
    Ok(ctx.mail().map_err(unavailable)?.lease())
}

/// The content of one blob of the account, `None` if it doesn't exist,
/// counted in `lease` (see [`ref_bytes`]).
pub(crate) async fn blob_bytes(
    ctx: &Ctx<'_>,
    id: &BlobId,
    lease: &mut Lease,
) -> Result<Option<Arc<[u8]>>, SetError> {
    match BlobRef::parse(id) {
        None => Ok(None),
        Some(blob) => ref_bytes(ctx, &blob, lease).await,
    }
}

fn unavailable<E>(_: E) -> SetError {
    SetError::new(SetErrorType::Forbidden)
        .with_description("the mail server is unavailable; try again later")
}

fn reserve_error(error: &MailError) -> SetError {
    crate::error::mail_set_error(error)
}

/// The content of the blob `blob` names, `None` if it doesn't exist. Its
/// memory is added to `lease` (an upload before it is read, a message as
/// the mail layer loads it, a part once cut); the account's turn, if a
/// big message needed it, is given back before returning: the blob is
/// loaded, what comes next (building, `APPEND`) is not the server's copy.
async fn ref_bytes(
    ctx: &Ctx<'_>,
    blob: &BlobRef,
    lease: &mut Lease,
) -> Result<Option<Arc<[u8]>>, SetError> {
    let found = load_ref(ctx, blob, lease).await;
    lease.drop_turn();
    found
}

async fn load_ref(
    ctx: &Ctx<'_>,
    blob: &BlobRef,
    lease: &mut Lease,
) -> Result<Option<Arc<[u8]>>, SetError> {
    match blob {
        BlobRef::Upload(upload) => {
            let now = Ctx::now().map_err(|_| SetError::new(SetErrorType::Forbidden))?;
            let store_error = |e: nebula_store::StoreError| crate::error::store_set_error(&e);
            let Some(info) = ctx
                .store()
                .upload_info(ctx.id(), *upload, now)
                .await
                .map_err(store_error)?
            else {
                return Ok(None);
            };
            lease
                .reserve(info.size)
                .await
                .map_err(|e| reserve_error(&e))?;
            let found = ctx
                .store()
                .upload(ctx.id(), *upload, now)
                .await
                .map_err(store_error)?;
            let Some(found) = found else {
                lease.release(info.size);
                return Ok(None);
            };
            // The stored size is the real one; if not, count the real one.
            let real = len_of(&found.data);
            if real > info.size {
                lease
                    .reserve(real - info.size)
                    .await
                    .map_err(|e| reserve_error(&e))?;
            } else {
                lease.release(info.size - real);
            }
            Ok(Some(Arc::from(found.data)))
        }
        BlobRef::Message(email) => {
            let mail = ctx.mail().map_err(unavailable)?;
            match mail.load_message(*email, lease, Copies::One).await {
                Ok(raw) => Ok(Some(raw)),
                Err(MailError::NotFound(_)) => Ok(None),
                Err(error) => Err(crate::error::mail_set_error(&error)),
            }
        }
        BlobRef::Sieve(id, hash) => {
            let found = crate::sieve::blob_content(ctx.inner, ctx.id(), *id, hash)
                .await
                .map(|found| found.map(Arc::<[u8]>::from))
                .map_err(|error| {
                    let error = error.into_method_error();
                    SetError::new(SetErrorType::Forbidden).with_description(
                        error
                            .description()
                            .unwrap_or("the Sieve server is unavailable")
                            .to_owned(),
                    )
                })?;
            // Scripts are small: counted once read.
            if let Some(bytes) = &found {
                lease
                    .reserve(len_of(bytes))
                    .await
                    .map_err(|e| reserve_error(&e))?;
            }
            Ok(found)
        }
        BlobRef::Part(email, part) | BlobRef::Section(email, part, _) => {
            let mail = ctx.mail().map_err(unavailable)?;
            let before = lease.bytes();
            let raw = match mail.load_message(*email, lease, Copies::Two).await {
                Ok(raw) => raw,
                Err(MailError::NotFound(_)) => return Ok(None),
                Err(error) => return Err(crate::error::mail_set_error(&error)),
            };
            let added = lease.bytes().saturating_sub(before);
            // Parsing a big message is CPU work: off the async workers.
            let part = *part;
            let cut = tokio::task::spawn_blocking(move || {
                body::part_content(&raw, part).map(|(bytes, _)| Arc::<[u8]>::from(bytes))
            })
            .await
            .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"));
            // The message is gone (or the cache's): only the part stays.
            let kept = match &cut {
                Ok(Some(bytes)) => len_of(bytes),
                Ok(None) | Err(_) => 0,
            };
            lease.release(added.saturating_sub(kept));
            cut
        }
        BlobRef::Nested(outer, part) => nested_bytes(ctx, outer, *part, lease).await,
    }
}

/// `bn…`: part `part` of the message in blob `outer`, counted in `lease`
/// (the outer blob while it is cut, then only the part).
async fn nested_bytes(
    ctx: &Ctx<'_>,
    outer: &BlobRef,
    part: u32,
    lease: &mut Lease,
) -> Result<Option<Arc<[u8]>>, SetError> {
    let before = lease.bytes();
    // The message the part is in (boxed: the outer blob may be nested
    // too, up to `MAX_NESTING` levels).
    let Some(raw) = Box::pin(load_ref(ctx, outer, lease)).await? else {
        return Ok(None);
    };
    // Room for the part cut from it, at most as big.
    lease
        .reserve(len_of(&raw))
        .await
        .map_err(|e| reserve_error(&e))?;
    let added = lease.bytes().saturating_sub(before);
    let cut = tokio::task::spawn_blocking(move || {
        body::part_content(&raw, part).map(|(bytes, _)| Arc::<[u8]>::from(bytes))
    })
    .await
    .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"));
    let kept = match &cut {
        Ok(Some(bytes)) => len_of(bytes),
        Ok(None) | Err(_) => 0,
    };
    lease.release(added.saturating_sub(kept));
    cut
}

/// The size of a blob, if it can be known without loading it (uploads
/// and whole messages).
async fn known_size(ctx: &Ctx<'_>, blob: &BlobRef) -> Result<Option<u64>, SetError> {
    match blob {
        BlobRef::Upload(upload) => {
            let now = Ctx::now().map_err(|_| SetError::new(SetErrorType::Forbidden))?;
            let info = ctx
                .store()
                .upload_info(ctx.id(), *upload, now)
                .await
                .map_err(|e| crate::error::store_set_error(&e))?;
            Ok(info.map(|info| info.size))
        }
        BlobRef::Message(email) => {
            let found = ctx
                .store()
                .emails(ctx.id(), vec![*email])
                .await
                .map_err(|e| crate::error::store_set_error(&e))?;
            Ok(found.first().map(|email| email.meta.size))
        }
        BlobRef::Part(..) | BlobRef::Section(..) | BlobRef::Sieve(..) | BlobRef::Nested(..) => {
            Ok(None)
        }
    }
}

/// Loads every blob the body uses, or `blobNotFound` with the missing
/// ones. The body values and each blob, times the parts that use it,
/// must fit in `maxSizeAttachmentsPerEmail`: that is checked before
/// loading the next blob (with the stored size when there is one, and
/// again with the real one), so a create that is too big stops with
/// `tooLarge` without loading the rest.
///
/// Their memory is counted in `lease`: a create that doesn't get it in
/// time fails with "try again later" (not `tooLarge`).
pub(crate) async fn load_blobs(
    ctx: &Ctx<'_>,
    create: &EmailCreate,
    lease: &mut Lease,
) -> Result<Blobs, SetError> {
    let mut left = ctx.config().max_size_attachments;
    for value in create.body_values.values() {
        charge(&mut left, len_of(value.as_bytes()), 1)?;
    }
    let mut blobs = Blobs::new();
    let mut by_content: HashMap<ContentKey, Arc<[u8]>> = HashMap::new();
    let mut missing = Vec::new();
    for (id, uses) in blob_uses(create) {
        let Some(blob) = BlobRef::parse(&id) else {
            missing.push(id);
            continue;
        };
        let key = blob.content_key();
        if let Some(bytes) = by_content.get(&key) {
            charge(&mut left, len_of(bytes), uses)?;
            blobs.insert(id, Arc::clone(bytes));
            continue;
        }
        if let Some(size) = known_size(ctx, &blob).await? {
            // Only a check: the real size is charged once loaded.
            if size.saturating_mul(uses) > left {
                return Err(too_large());
            }
        }
        match ref_bytes(ctx, &blob, lease).await? {
            Some(bytes) => {
                charge(&mut left, len_of(&bytes), uses)?;
                by_content.insert(key, Arc::clone(&bytes));
                blobs.insert(id, bytes);
            }
            None => missing.push(id),
        }
    }
    if missing.is_empty() {
        Ok(blobs)
    } else {
        Err(SetError::blob_not_found(missing))
    }
}

/// Bytes of `len` in base64 with a CRLF every 76 columns.
fn base64_len(len: u64) -> u64 {
    let encoded = len.div_ceil(3).saturating_mul(4);
    encoded.saturating_add(encoded.div_ceil(76).saturating_mul(2))
}

/// Room for the headers and the MIME structure around the parts.
const STRUCTURE_MARGIN: u64 = 64 * 1024;

/// An upper bound for the size of the message [`build`] writes: every
/// use of a blob and every body value in base64 (mail-builder never picks
/// anything bigger), plus a margin for the rest.
pub(crate) fn estimated_size(create: &EmailCreate, blobs: &Blobs) -> u64 {
    let mut total = STRUCTURE_MARGIN;
    for (id, uses) in blob_uses(create) {
        if let Some(bytes) = blobs.get(&id) {
            total = total.saturating_add(base64_len(len_of(bytes)).saturating_mul(uses));
        }
    }
    for value in create.body_values.values() {
        total = total.saturating_add(base64_len(len_of(value.as_bytes())));
    }
    total
}

fn address_list(list: &[EmailAddress]) -> Address<'static> {
    Address::new_list(
        list.iter()
            .map(|a| Address::new_address(a.name.clone(), a.email.clone()))
            .collect(),
    )
}

/// Message ids without the angle brackets (mail-builder adds them).
fn message_ids(list: &[String]) -> MessageId<'static> {
    MessageId::new_list(list.iter().map(|id| {
        id.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_owned()
    }))
}

fn header_value(value: &HeaderFormValue) -> Option<HeaderType<'static>> {
    Some(match value {
        HeaderFormValue::Raw(raw) => Raw::new(raw.trim_start().to_owned()).into(),
        HeaderFormValue::Text(text) => Text::new(text.clone()).into(),
        HeaderFormValue::Addresses(list) => address_list(list).into(),
        HeaderFormValue::GroupedAddresses(groups) => Address::new_list(
            groups
                .iter()
                .map(|g| match &g.name {
                    Some(name) => {
                        Address::new_group(Some(name.clone()), vec![address_list(&g.addresses)])
                    }
                    None => address_list(&g.addresses),
                })
                .collect(),
        )
        .into(),
        HeaderFormValue::MessageIds(Some(ids)) => message_ids(ids).into(),
        HeaderFormValue::Urls(Some(urls)) => URL::new_list(urls.iter().cloned()).into(),
        HeaderFormValue::Date(Some(date)) => Date::new(date.to_utc().unix_timestamp()).into(),
        HeaderFormValue::MessageIds(None)
        | HeaderFormValue::Urls(None)
        | HeaderFormValue::Date(None)
        | _ => {
            return None;
        }
    })
}

fn values_of(value: &HeaderValue) -> Vec<&HeaderFormValue> {
    match value {
        HeaderValue::Last(value) => vec![value],
        HeaderValue::All(list) => list.iter().collect(),
        HeaderValue::Missing | _ => Vec::new(),
    }
}

/// The name a field is written with. `mail-builder` adds its own `Date`
/// and `Message-ID` unless it finds these exact names (it compares case
/// and all), so the client's spelling is not kept for them; nor for
/// `Content-Disposition`, which it looks at to pick an encoding.
fn field_name(name: &str) -> String {
    if name.eq_ignore_ascii_case("date") {
        "Date".to_owned()
    } else if name.eq_ignore_ascii_case("message-id") {
        "Message-ID".to_owned()
    } else if name.eq_ignore_ascii_case("content-disposition") {
        "Content-Disposition".to_owned()
    } else {
        name.to_owned()
    }
}

/// `(name, value)` pairs of the `header:` properties (a `null` value
/// writes nothing).
fn extra_headers(properties: &HeaderProperties) -> Vec<(String, HeaderType<'static>)> {
    let mut out = Vec::new();
    for (property, value) in properties.iter() {
        for value in values_of(value) {
            if let Some(header) = header_value(value) {
                out.push((field_name(property.name()), header));
            }
        }
    }
    out
}

/// Whether a `header:` property writes field `name` (`null` doesn't).
fn writes_field(properties: &HeaderProperties, name: &str) -> bool {
    properties.iter().any(|(property, value)| {
        property.name().eq_ignore_ascii_case(name)
            && values_of(value)
                .into_iter()
                .any(|value| header_value(value).is_some())
    })
}

/// A part that mail-builder could write as it is (not in base64): its
/// bytes can hold `--` lines, so the boundaries are chosen around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LeafKind {
    /// `message/rfc822`: as it is, or `message/global` (see
    /// [`encapsulate`]).
    Message,
    /// `message/partial` and `message/external-body`: only 7bit (RFC 2046
    /// §5.2.2–§5.2.3).
    SevenBitOnly,
    /// Text (`text/*` blobs and every body value): mail-builder may pick
    /// 7bit or quoted-printable, which keep lines as they are.
    Text,
}

fn leaf_kind(part: &EmailBodyPartCreate) -> Option<LeafKind> {
    let media = part.media_type.to_ascii_lowercase();
    match media.as_str() {
        "message/rfc822" => Some(LeafKind::Message),
        "message/partial" | "message/external-body" => Some(LeafKind::SevenBitOnly),
        _ if matches!(part.content, BodyPartContent::Value(_)) => Some(LeafKind::Text),
        _ if media.starts_with("text/") => Some(LeafKind::Text),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LeafKey {
    Blob(BlobId),
    Value(PartId),
}

/// How one such part is written.
struct Leaf<'a> {
    /// What goes out: CRLF line ends for messages, as given for text.
    bytes: Cow<'a, [u8]>,
    /// The bytes as given (for `message/global` in base64).
    original: &'a [u8],
    encapsulation: Encapsulation,
    /// A text part whose lines clash with every boundary tried: in base64.
    encoded: bool,
}

impl<'a> Leaf<'a> {
    /// The media type, the body and the transfer encoding of an
    /// encapsulated message.
    fn encapsulated(&self, media: &str) -> (String, BodyPart<'a>, Option<&'static str>) {
        let raw = || BodyPart::Binary(self.bytes.clone());
        match self.encapsulation {
            Encapsulation::SevenBit => (media.to_owned(), raw(), Some("7bit")),
            Encapsulation::EightBit => (media.to_owned(), raw(), Some("8bit")),
            Encapsulation::GlobalEightBit => ("message/global".to_owned(), raw(), Some("8bit")),
            // mail-builder writes a non-text part in base64 by itself.
            Encapsulation::Global => (
                "message/global".to_owned(),
                BodyPart::Binary(Cow::Borrowed(self.original)),
                None,
            ),
        }
    }
}

fn collect_leaves<'p>(
    part: &'p EmailBodyPartCreate,
    leaves: &mut Vec<&'p EmailBodyPartCreate>,
    multiparts: &mut usize,
) {
    if let BodyPartContent::Multipart(children) = &part.content {
        *multiparts = multiparts.saturating_add(1);
        for child in children {
            collect_leaves(child, leaves, multiparts);
        }
    } else {
        leaves.push(part);
    }
}

fn unbuildable() -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description("message could not be built")
}

/// How a leaf part is written.
struct LeafContent<'a> {
    media: String,
    charset: Option<String>,
    transfer: Option<&'static str>,
    body: BodyPart<'a>,
}

/// The body values and blobs, borrowed by the MIME parts (no copies), how
/// the parts written as they are go, and the boundaries.
struct Parts<'a> {
    values: &'a std::collections::BTreeMap<PartId, String>,
    blobs: &'a Blobs,
    leaves: HashMap<(LeafKey, LeafKind), Leaf<'a>>,
    boundaries: RefCell<Vec<String>>,
}

impl<'a> Parts<'a> {
    /// Looks at every part that would be written as it is and chooses the
    /// boundaries around them (see [`encapsulate`]).
    fn new(create: &'a EmailCreate, blobs: &'a Blobs) -> Result<Self, SetError> {
        let mut found = Vec::new();
        let mut multiparts = 0_usize;
        match &create.body {
            EmailCreateBody::Structure(part) => collect_leaves(part, &mut found, &mut multiparts),
            EmailCreateBody::Parts {
                text,
                html,
                attachments,
            } => {
                // alternative, related and mixed at most.
                multiparts = 3;
                for part in text
                    .as_deref()
                    .into_iter()
                    .chain(html.as_deref())
                    .chain(attachments)
                {
                    collect_leaves(part, &mut found, &mut multiparts);
                }
            }
            _ => {}
        }
        let mut leaves: HashMap<(LeafKey, LeafKind), Leaf<'a>> = HashMap::new();
        let mut raw: Vec<((LeafKey, LeafKind), Delimiters)> = Vec::new();
        for part in found {
            let Some(kind) = leaf_kind(part) else {
                continue;
            };
            // A missing value or blob fails later, where it is used.
            let (key, original): (LeafKey, &'a [u8]) = match &part.content {
                BodyPartContent::Value(id) => match create.body_values.get(id) {
                    Some(text) => (LeafKey::Value(id.clone()), text.as_bytes()),
                    None => continue,
                },
                BodyPartContent::Blob(id) => match blobs.get(id) {
                    Some(bytes) => (LeafKey::Blob(id.clone()), &bytes[..]),
                    None => continue,
                },
                _ => continue,
            };
            let key = (key, kind);
            if leaves.contains_key(&key) {
                continue;
            }
            let (bytes, encapsulation) = match kind {
                LeafKind::Message | LeafKind::SevenBitOnly => {
                    let bytes = encapsulate::canonical(original);
                    let class = encapsulate::classify(&bytes);
                    if kind == LeafKind::SevenBitOnly && class != Encapsulation::SevenBit {
                        return Err(SetError::invalid_properties(["bodyStructure"])
                            .with_description(
                                "message/partial and message/external-body must be 7-bit",
                            ));
                    }
                    (bytes, class)
                }
                LeafKind::Text => (Cow::Borrowed(original), Encapsulation::SevenBit),
            };
            if encapsulation.is_raw() {
                raw.push((key.clone(), Delimiters::of(&bytes)));
            }
            leaves.insert(
                key,
                Leaf {
                    bytes,
                    original,
                    encapsulation,
                    encoded: false,
                },
            );
        }
        let delimiters: Vec<Delimiters> = raw.iter().map(|(_, found)| found.clone()).collect();
        let plan = encapsulate::plan_boundaries(multiparts, &delimiters, encapsulate::boundary)?;
        for ((key, _), encoded) in raw.into_iter().zip(plan.encoded) {
            if !encoded {
                continue;
            }
            match key.1 {
                LeafKind::SevenBitOnly => return Err(unbuildable()),
                LeafKind::Message => {
                    if let Some(leaf) = leaves.get_mut(&key) {
                        leaf.encapsulation = Encapsulation::Global;
                    }
                }
                LeafKind::Text => {
                    if let Some(leaf) = leaves.get_mut(&key) {
                        leaf.encoded = true;
                    }
                }
            }
        }
        let mut boundaries = plan.boundaries;
        boundaries.reverse();
        Ok(Self {
            values: &create.body_values,
            blobs,
            leaves,
            boundaries: RefCell::new(boundaries),
        })
    }

    /// The next planned boundary.
    fn boundary(&self) -> Result<String, SetError> {
        self.boundaries.borrow_mut().pop().ok_or_else(unbuildable)
    }

    fn leaf(&self, part: &EmailBodyPartCreate, key: LeafKey) -> Option<(LeafKind, &Leaf<'a>)> {
        let kind = leaf_kind(part)?;
        self.leaves.get(&(key, kind)).map(|leaf| (kind, leaf))
    }

    /// The content of a leaf part, how it is written.
    fn leaf_content(&self, part: &EmailBodyPartCreate) -> Result<LeafContent<'a>, SetError> {
        let mut media = part.media_type.clone();
        let mut charset: Option<String> = None;
        let mut transfer: Option<&'static str> = None;
        let body = match &part.content {
            BodyPartContent::Value(id) => {
                let text = self.values.get(id).ok_or_else(|| {
                    SetError::invalid_properties(["bodyStructure"])
                        .with_description("missing body value")
                })?;
                match self.leaf(part, LeafKey::Value(id.clone())) {
                    // An encapsulated message has no charset.
                    Some((LeafKind::Message | LeafKind::SevenBitOnly, leaf)) => {
                        let (as_media, body, cte) = leaf.encapsulated(&media);
                        media = as_media;
                        transfer = cte;
                        body
                    }
                    // Body values are always UTF-8 (and the client can't give a
                    // charset with partId). mail-builder adds it only when the
                    // type has no other parameter (`method`, `name`).
                    Some((_, leaf)) if leaf.encoded => {
                        charset = Some("utf-8".to_owned());
                        transfer = Some("base64");
                        BodyPart::Binary(Cow::Owned(encapsulate::base64_lines(text.as_bytes())))
                    }
                    _ => {
                        charset = Some("utf-8".to_owned());
                        BodyPart::Text(Cow::Borrowed(text.as_str()))
                    }
                }
            }
            BodyPartContent::Blob(id) => {
                let bytes = self
                    .blobs
                    .get(id)
                    .ok_or_else(|| SetError::blob_not_found(vec![id.clone()]))?;
                match self.leaf(part, LeafKey::Blob(id.clone())) {
                    Some((LeafKind::Message | LeafKind::SevenBitOnly, leaf)) => {
                        let (as_media, body, cte) = leaf.encapsulated(&media);
                        media = as_media;
                        transfer = cte;
                        body
                    }
                    Some((_, leaf)) if leaf.encoded => {
                        charset.clone_from(&part.charset);
                        transfer = Some("base64");
                        BodyPart::Binary(Cow::Owned(encapsulate::base64_lines(bytes)))
                    }
                    _ => {
                        charset.clone_from(&part.charset);
                        BodyPart::Binary(Cow::Borrowed(&bytes[..]))
                    }
                }
            }
            _ => return Err(unbuildable()),
        };
        Ok(LeafContent {
            media,
            charset,
            transfer,
            body,
        })
    }

    fn mime(
        &self,
        part: &EmailBodyPartCreate,
        default_disposition: Option<&str>,
    ) -> Result<MimePart<'a>, SetError> {
        let invalid = |why: &str| {
            SetError::invalid_properties(["bodyStructure"]).with_description(why.to_owned())
        };
        let mut boundary: Option<String> = None;
        let (contents, media, charset, transfer) = match &part.content {
            BodyPartContent::Value(_) | BodyPartContent::Blob(_) => {
                let leaf = self.leaf_content(part)?;
                (leaf.body, leaf.media, leaf.charset, leaf.transfer)
            }
            BodyPartContent::Multipart(children) => {
                boundary = Some(self.boundary()?);
                let children = children
                    .iter()
                    .map(|child| self.mime(child, None))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    BodyPart::Multipart(children),
                    part.media_type.clone(),
                    None,
                    None,
                )
            }
            _ => return Err(invalid("unknown body part")),
        };
        let mut content_type = ContentType::new(media);
        for (name, value) in &part.media_type_params {
            content_type = content_type.attribute(name.clone(), value.clone());
        }
        if let Some(charset) = charset {
            content_type = content_type.attribute("charset", charset);
        }
        if let Some(boundary) = boundary {
            content_type = content_type.attribute("boundary", boundary);
        }
        if let Some(name) = &part.name {
            content_type = content_type.attribute("name", name.clone());
        }
        let mut mime = MimePart::new(content_type, contents);
        if let Some(cte) = transfer {
            mime = mime.transfer_encoding(cte);
        }
        // The default disposition only if the client didn't write its own.
        let default_disposition = default_disposition
            .filter(|_| !writes_field(&part.header_properties, "content-disposition"));
        let disposition = part.disposition.as_deref().or(default_disposition);
        if let Some(disposition) = disposition {
            let mut value = ContentType::new(disposition.to_ascii_lowercase());
            if let Some(name) = &part.name {
                value = value.attribute("filename", name.clone());
            }
            mime = mime.header("Content-Disposition", value);
        }
        if let Some(cid) = &part.cid {
            mime = mime.cid(cid.trim_start_matches('<').trim_end_matches('>').to_owned());
        }
        if let Some(language) = &part.language {
            mime = mime.language(language.join(", "));
        }
        if let Some(location) = &part.location {
            mime = mime.location(location.clone());
        }
        for (name, value) in extra_headers(&part.header_properties) {
            mime = mime.header(name, value);
        }
        Ok(mime)
    }

    /// The tree the server makes from `textBody`, `htmlBody` and
    /// `attachments`.
    fn tree_of_lists(
        &self,
        text: Option<&EmailBodyPartCreate>,
        html: Option<&EmailBodyPartCreate>,
        attachments: &[EmailBodyPartCreate],
    ) -> Result<MimePart<'a>, SetError> {
        let text = text.map(|p| self.mime(p, None)).transpose()?;
        let html = html.map(|p| self.mime(p, None)).transpose()?;
        let mut main = match (text, html) {
            (Some(text), Some(html)) => MimePart::new(
                ContentType::new("multipart/alternative").attribute("boundary", self.boundary()?),
                BodyPart::Multipart(vec![text, html]),
            ),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => MimePart::new("text/plain", BodyPart::Text(Cow::Borrowed(""))),
        };
        let (inline, files): (Vec<&EmailBodyPartCreate>, Vec<&EmailBodyPartCreate>) =
            attachments.iter().partition(|p| {
                p.cid.is_some()
                    && p.disposition
                        .as_deref()
                        .is_none_or(|d| d.eq_ignore_ascii_case("inline"))
            });
        if !inline.is_empty() {
            let mut parts = vec![main];
            for part in inline {
                parts.push(self.mime(part, Some("inline"))?);
            }
            main = MimePart::new(
                ContentType::new("multipart/related").attribute("boundary", self.boundary()?),
                BodyPart::Multipart(parts),
            );
        }
        if files.is_empty() {
            return Ok(main);
        }
        let mut parts = vec![main];
        for part in files {
            parts.push(self.mime(part, Some("attachment"))?);
        }
        Ok(MimePart::new(
            ContentType::new("multipart/mixed").attribute("boundary", self.boundary()?),
            BodyPart::Multipart(parts),
        ))
    }
}

/// A random `Message-ID` at the From address's domain.
pub(crate) fn new_message_id(create: &EmailCreate) -> Result<String, SetError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let mut local = String::with_capacity(32);
    for byte in bytes {
        // Writing to a String can't fail.
        let _ = write!(local, "{byte:02x}");
    }
    let domain = create
        .from
        .as_ref()
        .and_then(|list| list.first())
        .and_then(|a| a.email.rsplit_once('@'))
        .map(|(_, domain)| domain.trim())
        .filter(|d| {
            !d.is_empty()
                && d.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
        .unwrap_or("nebula.invalid");
    Ok(format!("{local}@{domain}"))
}

/// The raw RFC 5322 message.
pub(crate) fn build(create: &EmailCreate, blobs: &Blobs, now: i64) -> Result<Vec<u8>, SetError> {
    let mut builder = MessageBuilder::new();
    if let Some(list) = &create.from {
        builder = builder.from(address_list(list));
    }
    if let Some(list) = &create.sender {
        builder = builder.sender(address_list(list));
    }
    if let Some(list) = &create.to {
        builder = builder.to(address_list(list));
    }
    if let Some(list) = &create.cc {
        builder = builder.cc(address_list(list));
    }
    if let Some(list) = &create.bcc {
        builder = builder.bcc(address_list(list));
    }
    if let Some(list) = &create.reply_to {
        builder = builder.reply_to(address_list(list));
    }
    if let Some(subject) = &create.subject {
        builder = builder.subject(Text::new(subject.clone()));
    }
    // The server writes Date and Message-ID only if the client doesn't,
    // with a property or with a `header:` one (RFC 8621 §4.6).
    if !writes_field(&create.header_properties, "message-id") {
        let message_id = match create.message_id.as_ref().and_then(|ids| ids.first()) {
            Some(id) => id.clone(),
            None => new_message_id(create)?,
        };
        builder = builder.message_id(message_ids(&[message_id]));
    }
    if let Some(ids) = &create.in_reply_to {
        builder = builder.in_reply_to(message_ids(ids));
    }
    if let Some(ids) = &create.references {
        builder = builder.references(message_ids(ids));
    }
    if !writes_field(&create.header_properties, "date") {
        // RFC 5322 §3.3: the zone is the offset of the local time used, so
        // a `sentAt` keeps its own (RFC 8621 §4.1.2.6), not UTC.
        let with_offset = create.sent_at.and_then(|date| {
            date.to_offset_date_time()
                .format(&time::format_description::well_known::Rfc2822)
                .ok()
        });
        builder = match with_offset {
            Some(date) => builder.header("Date", Raw::new(date)),
            None => builder.date(Date::new(
                create
                    .sent_at
                    .map_or(now, |date| date.to_utc().unix_timestamp()),
            )),
        };
    }
    for (name, value) in extra_headers(&create.header_properties) {
        builder = builder.header(name, value);
    }
    let parts = Parts::new(create, blobs)?;
    let body = match &create.body {
        EmailCreateBody::Structure(part) => parts.mime(part, None)?,
        EmailCreateBody::Parts {
            text,
            html,
            attachments,
        } => parts.tree_of_lists(text.as_deref(), html.as_deref(), attachments)?,
        _ => return Err(SetError::invalid_properties(["bodyStructure"])),
    };
    builder.body(body).write_to_vec().map_err(|_| {
        SetError::new(SetErrorType::Forbidden).with_description("message could not be built")
    })
}

#[cfg(test)]
mod tests {
    use mail_parser::MessageParser;
    use serde_json::json;

    use super::*;

    fn create(value: &serde_json::Value) -> EmailCreate {
        EmailCreate::parse(value.as_object().unwrap().clone()).unwrap()
    }

    /// What the web app's composer sends (apps/web/src/mail/compose.ts).
    #[test]
    fn web_draft() {
        let draft = create(&json!({
            "mailboxIds": {"m2": true},
            "keywords": {"$draft": true, "$seen": true},
            "from": [{"email": "alice@nebulamail.test", "name": "Alice"}],
            "to": [{"email": "bob@nebulamail.test", "name": null}],
            "bcc": [{"email": "carol@nebulamail.test"}],
            "subject": "Informe ñ",
            "inReplyTo": ["abc@x.test"],
            "textBody": [{"partId": "text", "type": "text/plain"}],
            "htmlBody": [{"partId": "html", "type": "text/html"}],
            "attachments": [{"blobId": "bu1", "type": "application/pdf", "name": "a.pdf"}],
            "bodyValues": {"text": {"value": "Hola"}, "html": {"value": "<p>Hola</p>"}}
        }));
        let blobs = Blobs::from([("bu1".parse().unwrap(), Arc::from(b"%PDF".as_slice()))]);
        let raw = build(&draft, &blobs, 1_792_836_000).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(message.subject(), Some("Informe ñ"));
        assert_eq!(
            message.bcc().unwrap().first().unwrap().address(),
            Some("carol@nebulamail.test")
        );
        assert!(message.message_id().unwrap().ends_with("@nebulamail.test"));
        assert_eq!(
            message.in_reply_to().and_then(|ids| ids.first()),
            Some("abc@x.test")
        );
        assert_eq!(message.text_body().len(), 1);
        assert_eq!(message.body_text(0).as_deref(), Some("Hola"));
        assert_eq!(message.body_html(0).as_deref(), Some("<p>Hola</p>"));
        let attachment = message.attachments().next().unwrap();
        assert_eq!(attachment.decoded().as_ref(), b"%PDF");
        assert_eq!(attachment.attachment_name(), Some("a.pdf"));
        assert_eq!(
            message.root_part().content_type().unwrap().subtype(),
            Some("mixed")
        );
    }

    #[test]
    fn structure_with_inline_image() {
        let draft = create(&json!({
            "mailboxIds": {"m2": true},
            "from": [{"email": "alice@nebulamail.test"}],
            "bodyStructure": {"type": "multipart/related", "subParts": [
                {"type": "multipart/alternative", "subParts": [
                    {"partId": "text", "type": "text/plain"},
                    {"partId": "html", "type": "text/html"}
                ]},
                {"blobId": "bu2", "type": "image/png", "cid": "img1@x", "disposition": "inline", "name": "i.png"}
            ]},
            "bodyValues": {"text": {"value": "T"}, "html": {"value": "<img src=\"cid:img1@x\">"}}
        }));
        let blobs = Blobs::from([(
            "bu2".parse().unwrap(),
            Arc::from([0x89, b'P', b'N', b'G'].as_slice()),
        )]);
        let raw = build(&draft, &blobs, 0).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        let image = message.parts().find(|p| p.content_id().is_some()).unwrap();
        assert_eq!(image.content_id(), Some("img1@x"));
        assert_eq!(image.decoded().as_ref(), &[0x89, b'P', b'N', b'G']);
        assert_eq!(
            blob_uses(&draft),
            vec![("bu2".parse::<BlobId>().unwrap(), 1)]
        );
    }

    /// The web's iMIP reply (calendar/itip.ts): the method goes in the
    /// part's Content-Type (RFC 6047 §2.4).
    #[test]
    fn calendar_reply_with_method() {
        let reply = create(&json!({
            "mailboxIds": {"m8": true},
            "from": [{"email": "bob@nebulamail.test", "name": null}],
            "to": [{"email": "alice@nebulamail.test", "name": null}],
            "subject": "Accepted: Planning",
            "bodyStructure": {"type": "multipart/alternative", "subParts": [
                {"partId": "text", "type": "text/plain"},
                {"partId": "ics", "type": "text/calendar; method=REPLY"}
            ]},
            "bodyValues": {
                "text": {"value": "Bob accepted."},
                "ics": {"value": "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n"}
            }
        }));
        let raw = build(&reply, &Blobs::new(), 0).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        let calendar = message
            .parts()
            .find(|part| part.is_content_type("text", "calendar"))
            .unwrap();
        assert_eq!(
            calendar.content_type().unwrap().attribute("method"),
            Some("REPLY")
        );
        assert!(calendar.text().unwrap().contains("METHOD:REPLY"));
    }

    #[test]
    fn missing_blobs() {
        let draft = create(&json!({
            "mailboxIds": {"m2": true},
            "attachments": [{"blobId": "bu9", "type": "image/png"}],
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "x"}}
        }));
        let error = build(&draft, &Blobs::new(), 0).unwrap_err();
        assert_eq!(error.kind(), SetErrorType::BlobNotFound);
    }

    fn count(raw: &[u8], field: &str) -> usize {
        let text = String::from_utf8_lossy(raw);
        let head = text.split("\r\n\r\n").next().unwrap_or_default().to_owned();
        head.split("\r\n")
            .filter(|line| {
                line.split_once(':')
                    .is_some_and(|(name, _)| name.eq_ignore_ascii_case(field))
            })
            .count()
    }

    fn minimal(extra: &serde_json::Value) -> EmailCreate {
        let mut object = json!({
            "mailboxIds": {"m2": true},
            "from": [{"email": "alice@nebulamail.test"}],
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "x"}}
        });
        object
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        create(&object)
    }

    #[test]
    fn date_and_message_id_once() {
        let raw = build(&minimal(&json!({})), &Blobs::new(), 0).unwrap();
        assert_eq!(count(&raw, "Date"), 1);
        assert_eq!(count(&raw, "Message-ID"), 1);
        assert_eq!(count(&raw, "MIME-Version"), 1);
        // The client's own, in any case: only that one.
        let raw = build(
            &minimal(&json!({"header:message-id:asMessageIds": ["own@nebulamail.test"]})),
            &Blobs::new(),
            0,
        )
        .unwrap();
        assert_eq!(count(&raw, "Message-ID"), 1);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("<own@nebulamail.test>"), "{text}");
        assert!(!text.contains("localhost"), "{text}");
        let raw = build(
            &minimal(&json!({"header:date:asDate": "2026-10-24T10:00:00+02:00"})),
            &Blobs::new(),
            0,
        )
        .unwrap();
        assert_eq!(count(&raw, "Date"), 1);
        let message = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            message.date().map(|d| d.to_timestamp()),
            Some(1_792_828_800)
        );
        // null writes nothing, so the server's own goes in.
        let raw = build(
            &minimal(&json!({
                "header:Date:asDate": null,
                "header:Message-ID:asMessageIds": null
            })),
            &Blobs::new(),
            0,
        )
        .unwrap();
        assert_eq!(count(&raw, "Date"), 1);
        assert_eq!(count(&raw, "Message-ID"), 1);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("@nebulamail.test>"), "{text}");
        assert!(!text.contains("localhost"), "{text}");
    }

    #[test]
    fn one_content_disposition() {
        let draft = minimal(&json!({
            "attachments": [{
                "blobId": "bu1", "type": "application/pdf",
                "header:Content-Disposition": " attachment; filename=\"x.pdf\""
            }]
        }));
        let blobs = Blobs::from([("bu1".parse().unwrap(), Arc::from(b"%PDF".as_slice()))]);
        let raw = build(&draft, &blobs, 0).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert_eq!(text.matches("Content-Disposition").count(), 1, "{text}");
        assert_eq!(
            text.to_ascii_lowercase()
                .matches("content-disposition")
                .count(),
            1
        );
    }

    #[test]
    fn text_values_say_utf8() {
        let draft = create(&json!({
            "mailboxIds": {"m8": true},
            "from": [{"email": "alice@nebulamail.test"}],
            "bodyStructure": {"type": "multipart/mixed", "subParts": [
                {"partId": "ics", "type": "text/calendar; method=REPLY"},
                {"partId": "t", "name": "nota.txt"}
            ]},
            "bodyValues": {
                "ics": {"value": "BEGIN:VCALENDAR\r\nSUMMARY:Reunión\r\nEND:VCALENDAR\r\n"},
                "t": {"value": "Canción"}
            }
        }));
        let raw = build(&draft, &Blobs::new(), 0).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        let calendar = message
            .parts()
            .find(|part| part.is_content_type("text", "calendar"))
            .unwrap();
        let content_type = calendar.content_type().unwrap();
        assert_eq!(content_type.attribute("method"), Some("REPLY"));
        assert_eq!(content_type.attribute("charset"), Some("utf-8"));
        assert!(calendar.text().unwrap().contains("Reunión"));
        let note = message
            .parts()
            .find(|part| part.attachment_name() == Some("nota.txt"))
            .unwrap();
        assert_eq!(
            note.content_type().unwrap().attribute("charset"),
            Some("utf-8")
        );
        assert_eq!(note.text().as_deref(), Some("Canción"));
        let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        assert_eq!(text.matches("charset").count(), 2, "{text}");
    }

    #[test]
    fn blobs_are_counted_by_use() {
        let draft = create(&json!({
            "mailboxIds": {"m2": true},
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "x"}},
            "attachments": [
                {"blobId": "bu1", "type": "image/png"},
                {"blobId": "bu2", "type": "image/png"},
                {"blobId": "bu1", "type": "image/png"},
                {"blobId": "bu1", "type": "image/png"}
            ]
        }));
        let id = |text: &str| text.parse::<BlobId>().unwrap();
        assert_eq!(blob_uses(&draft), vec![(id("bu1"), 3), (id("bu2"), 1)]);
        // 3 uses of 10 bytes need 30.
        let mut left = 29;
        assert_eq!(
            charge(&mut left, 10, 3).unwrap_err().kind(),
            SetErrorType::TooLarge
        );
        assert_eq!(left, 29);
        let mut left = 30;
        charge(&mut left, 10, 3).unwrap();
        assert_eq!(left, 0);
        let mut left = u64::MAX - 1;
        assert!(charge(&mut left, u64::MAX / 2 + 1, 2).is_err());
        // A shared blob is one copy in the message built.
        let bytes: Arc<[u8]> = Arc::from(b"PNG".as_slice());
        let blobs = Blobs::from([(id("bu1"), Arc::clone(&bytes)), (id("bu2"), bytes)]);
        let raw = build(&draft, &blobs, 0).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(message.attachments().count(), 4);
    }

    #[test]
    fn the_estimate_covers_the_message() {
        let draft = create(&json!({
            "mailboxIds": {"m2": true},
            "textBody": [{"partId": "t", "type": "text/plain"}],
            "bodyValues": {"t": {"value": "x".repeat(10_000)}},
            "attachments": [
                {"blobId": "bu1", "type": "application/octet-stream"},
                {"blobId": "bu1", "type": "application/octet-stream"}
            ]
        }));
        let blobs = Blobs::from([("bu1".parse().unwrap(), Arc::from(vec![0xff_u8; 300_000]))]);
        let raw = build(&draft, &blobs, 0).unwrap();
        let estimate = estimated_size(&draft, &blobs);
        assert!(len_of(&raw) <= estimate, "{} > {estimate}", raw.len());
        assert!(estimate < len_of(&raw) * 2);
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(57), 78);
    }

    /// A create whose message doesn't fit the free memory waits, then
    /// fails with "try again later": not `tooLarge`, it may work later.
    #[tokio::test(start_paused = true)]
    async fn a_full_budget_is_try_again_later() {
        use std::time::Duration;

        let budget = nebula_mail::MemoryBudget::new(
            1024 * 1024,
            Duration::from_secs(30),
            Duration::from_secs(30),
        );
        let draft = minimal(&json!({
            "attachments": [{"blobId": "bu1", "type": "application/pdf"}]
        }));
        let blobs = Blobs::from([(
            "bu1".parse().unwrap(),
            Arc::from(vec![1_u8; 2 * 1024 * 1024]),
        )]);
        // Another big message holds all of it.
        let mut other = budget.lease();
        other.reserve(1024 * 1024).await.unwrap();
        let mut lease = budget.lease();
        let started = tokio::time::Instant::now();
        let error = lease
            .reserve(estimated_size(&draft, &blobs))
            .await
            .unwrap_err();
        assert!(started.elapsed() >= Duration::from_secs(30));
        let error = crate::error::mail_set_error(&error);
        assert_eq!(error.kind(), SetErrorType::Forbidden);
        assert!(error.description().unwrap().contains("try again later"));
        assert_eq!(lease.bytes(), 0);
    }

    /// The encapsulated message of a built forward: its media type, its
    /// transfer encoding and its content (decoded).
    fn forwarded(raw: &[u8]) -> (String, Option<String>, Vec<u8>) {
        let parsed = nebula_mail::ParsedMessage::parse(raw).unwrap();
        let (index, part) = parsed
            .parts()
            .find(|(_, part)| nebula_mail::media_type(part).starts_with("message/"))
            .unwrap();
        let cte = part
            .content_transfer_encoding()
            .map(|value| value.trim().to_ascii_lowercase());
        let media = nebula_mail::media_type(&part);
        let (content, _) = body::part_content(raw, index).unwrap();
        (media, cte, content)
    }

    fn forward(inner: &[u8]) -> Vec<u8> {
        let draft = minimal(&json!({
            "attachments": [{"blobId": "bu1", "type": "message/rfc822", "name": "fwd.eml"}]
        }));
        let blobs = Blobs::from([("bu1".parse().unwrap(), Arc::from(inner))]);
        build(&draft, &blobs, 0).unwrap()
    }

    /// The boundaries a message declares.
    fn boundaries_of(raw: &[u8]) -> Vec<String> {
        let parsed = nebula_mail::ParsedMessage::parse(raw).unwrap();
        parsed
            .parts()
            .filter_map(|(_, part)| part.boundary().map(str::to_owned))
            .collect()
    }

    #[test]
    fn forwarded_ascii_message_is_7bit() {
        let inner = b"Subject: Hola\r\n\r\nHola\r\n";
        let raw = forward(inner);
        let (media, cte, content) = forwarded(&raw);
        assert_eq!(media, "message/rfc822");
        assert_eq!(cte.as_deref(), Some("7bit"));
        assert_eq!(content, inner);
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.to_ascii_lowercase().contains("base64"), "{text}");
        assert!(text.contains("Subject: Hola\r\n\r\nHola\r\n"), "{text}");
        let message = MessageParser::default().parse(&raw).unwrap();
        let nested = message
            .attachments()
            .find_map(|part| part.nested())
            .unwrap();
        assert_eq!(nested.subject(), Some("Hola"));
    }

    #[test]
    fn forwarded_8bit_message_is_8bit() {
        let inner = "Subject: Hola\r\n\r\nÑandú\r\n".as_bytes();
        let raw = forward(inner);
        let (media, cte, content) = forwarded(&raw);
        assert_eq!(media, "message/rfc822");
        assert_eq!(cte.as_deref(), Some("8bit"));
        assert_eq!(content, inner);
        assert!(raw.windows(inner.len()).any(|window| window == inner));
    }

    #[test]
    fn forwarded_message_gets_crlf() {
        let raw = forward(b"Subject: Hola\n\nuno\ndos\n");
        assert!(
            raw.windows(2)
                .enumerate()
                .all(|(at, w)| w[1] != b'\n' || w[0] == b'\r' || at == usize::MAX)
        );
        assert_eq!(forwarded(&raw).2, b"Subject: Hola\r\n\r\nuno\r\ndos\r\n");
    }

    #[test]
    fn unencodable_message_goes_as_global_base64() {
        let with_line = |n: usize| {
            let mut inner = b"Subject: a\r\n\r\n".to_vec();
            inner.extend(std::iter::repeat_n(b'x', n));
            inner.extend_from_slice(b"\r\n");
            inner
        };
        for inner in [with_line(999), b"Subject: a\r\n\r\nx\0y\n".to_vec()] {
            let raw = forward(&inner);
            let (media, cte, content) = forwarded(&raw);
            assert_eq!(media, "message/global");
            assert_eq!(cte.as_deref(), Some("base64"));
            // The original bytes, not even the line ends changed.
            assert_eq!(content, inner);
            assert!(!raw.contains(&0));
        }
        let (media, cte, _) = forwarded(&forward(&with_line(998)));
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/rfc822", Some("7bit"))
        );
    }

    /// RFC 6532 §3.7: a message with 8-bit header fields is a
    /// `message/global`; it still goes as it is.
    #[test]
    fn utf8_headers_make_a_global_message() {
        let inner = "Subject: Ñandú\r\nFrom: José <josé@example.com>\r\n\r\nHola\r\n".as_bytes();
        let (media, cte, content) = forwarded(&forward(inner));
        assert_eq!(media, "message/global");
        assert_eq!(cte.as_deref(), Some("8bit"));
        assert_eq!(content, inner);
    }

    #[test]
    fn rfc822_body_value_has_no_charset() {
        let draft = minimal(&json!({
            "attachments": [{"partId": "m", "type": "message/rfc822"}],
            "bodyValues": {"t": {"value": "x"}, "m": {"value": "Subject: a\r\n\r\nb\r\n"}}
        }));
        let raw = build(&draft, &Blobs::new(), 0).unwrap();
        let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        // Only the text part has a charset.
        assert_eq!(text.matches("charset").count(), 1, "{text}");
        let (media, cte, content) = forwarded(&raw);
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/rfc822", Some("7bit"))
        );
        assert_eq!(content, b"Subject: a\r\n\r\nb\r\n");
    }

    #[test]
    fn message_partial_must_be_7bit() {
        let draft = minimal(&json!({
            "attachments": [{"blobId": "bu1", "type": "message/partial"}]
        }));
        let blobs = Blobs::from([(
            "bu1".parse().unwrap(),
            Arc::from("Subject: a\r\n\r\nñ\r\n".as_bytes()),
        )]);
        let error = build(&draft, &blobs, 0).unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        let ascii = Blobs::from([(
            "bu1".parse().unwrap(),
            Arc::from(&b"Subject: a\r\n\r\nb\r\n"[..]),
        )]);
        let raw = build(&draft, &ascii, 0).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("Content-Transfer-Encoding: 7bit"));
    }

    #[test]
    fn boundaries_are_random() {
        let one = boundaries_of(
            &build(
                &minimal(&json!({
                    "htmlBody": [{"partId": "h", "type": "text/html"}],
                    "bodyValues": {"t": {"value": "x"}, "h": {"value": "<p>x</p>"}},
                    "attachments": [{"blobId": "bu1", "type": "application/pdf"}]
                })),
                &Blobs::from([("bu1".parse().unwrap(), Arc::from(&b"%PDF"[..]))]),
                0,
            )
            .unwrap(),
        );
        let two = boundaries_of(&forward(b"Subject: a\r\n\r\nb\r\n"));
        assert_eq!(one.len(), 2);
        for boundary in one.iter().chain(&two) {
            assert_eq!(boundary.len(), 32, "{boundary}");
            assert!(
                boundary.bytes().all(|b| b.is_ascii_hexdigit()),
                "{boundary}"
            );
        }
        assert_ne!(one[0], one[1]);
        assert!(!one.contains(&two[0]));
    }

    /// The parts of a built forward (text + the message), and the message
    /// as the receiver gets it.
    fn check_forward(inner: &[u8]) -> (String, Option<String>) {
        let raw = forward(inner);
        let parsed = nebula_mail::ParsedMessage::parse(&raw).unwrap();
        // Root (mixed), text and the message: nothing more, nothing less.
        assert_eq!(parsed.len(), 3, "{}", String::from_utf8_lossy(&raw));
        let message = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(message.body_text(0).as_deref(), Some("x"));
        assert_eq!(message.attachments().count(), 1);
        // No line of the message is taken for one of our delimiters.
        let ours = boundaries_of(&raw);
        let (media, cte, content) = forwarded(&raw);
        if media == "message/rfc822" {
            assert_eq!(content, encapsulate::canonical(inner).as_ref());
            let found = Delimiters::of(&content);
            assert!(ours.iter().all(|boundary| !found.clash(boundary)));
        } else {
            assert_eq!(content, inner);
        }
        (media, cte)
    }

    #[test]
    fn a_forwarded_message_cannot_break_out() {
        let inner = b"Subject: digest\r\n\
            Content-Type: multipart/mixed; boundary=\"inner\"\r\n\r\n\
            --inner\r\n\r\none\r\n--inner\r\n\r\ntwo\r\n--inner--\r\n\
            --whatever--\r\n--\r\n-- \r\n";
        let (media, cte) = check_forward(inner);
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/rfc822", Some("7bit"))
        );
    }

    /// R1: an inner boundary that is a prefix of ours would end the inner
    /// multipart at our delimiter for a parser that matches by prefix.
    /// Every hex digit taken: the message goes encoded.
    #[test]
    fn short_inner_boundaries_never_prefix_ours() {
        for digit in "0123456789abcdef".chars() {
            let inner = format!(
                "Subject: x\r\nContent-Type: multipart/mixed; boundary=\"{digit}\"\r\n\r\n\
                 --{digit}\r\n\r\nuno\r\n--{digit}--\r\n"
            );
            let (media, _) = check_forward(inner.as_bytes());
            assert_eq!(media, "message/rfc822");
        }
        // R1 bypass: a boundary in a form the scan can't read (RFC 2231,
        // blanks around `=`, a comment) goes encoded.
        for declared in [
            "boundary = \"0\"",
            "boundary*=''0",
            "boundary*=utf-8''%30",
            "boundary*0=\"0\"",
            "boundary=(c)0",
        ] {
            let inner = format!(
                "Subject: x\r\nContent-Type: multipart/mixed; {declared}\r\n\r\nno parts\r\n"
            );
            let (media, cte) = check_forward(inner.as_bytes());
            assert_eq!(
                (media.as_str(), cte.as_deref()),
                ("message/global", Some("base64")),
                "{declared}"
            );
        }
        // Declared, never closed (nor opened).
        let (media, _) = check_forward(
            b"Subject: x\r\nContent-Type: multipart/mixed; boundary=7\r\n\r\nno parts\r\n",
        );
        assert_eq!(media, "message/rfc822");
        let all: String = "0123456789abcdef".chars().fold(
            String::from("Subject: x\r\n\r\n"),
            |mut out, digit| {
                let _ = write!(out, "--{digit}\r\n");
                out
            },
        );
        let (media, cte) = check_forward(all.as_bytes());
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/global", Some("base64"))
        );
    }

    /// The same for the text parts, which mail-builder may write as they
    /// are: they go in base64 when every boundary clashes.
    #[test]
    fn text_with_every_delimiter_goes_in_base64() {
        let all: String = "0123456789abcdef"
            .chars()
            .fold(String::new(), |mut out, digit| {
                let _ = write!(out, "--{digit}\r\n");
                out
            });
        let draft = minimal(&json!({
            "bodyValues": {"t": {"value": all}},
            "attachments": [{"blobId": "bu1", "type": "application/pdf"}]
        }));
        let blobs = Blobs::from([("bu1".parse().unwrap(), Arc::from(&b"%PDF"[..]))]);
        let raw = build(&draft, &blobs, 0).unwrap();
        let message = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(message.body_text(0).as_deref(), Some(all.as_str()));
        assert_eq!(message.attachments().count(), 1);
        let text = message
            .parts()
            .find(|part| part.is_content_type("text", "plain"))
            .unwrap();
        assert_eq!(
            text.content_transfer_encoding()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("base64")
        );
        assert_eq!(
            text.content_type().unwrap().attribute("charset"),
            Some("utf-8")
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        #[test]
        fn rfc822_roundtrip(
            body in proptest::collection::vec(
                proptest::prop_oneof![
                    proptest::prelude::Just(b'\r'),
                    proptest::prelude::Just(b'\n'),
                    proptest::prelude::Just(0_u8),
                    proptest::prelude::Just(b'-'),
                    proptest::prelude::Just(0xc3_u8),
                    proptest::prelude::Just(0xb1_u8),
                    proptest::prelude::any::<u8>(),
                    b'a'..=b'f',
                ],
                0..3000,
            )
        ) {
            let mut inner = b"Subject: x\r\n\r\n".to_vec();
            inner.extend(&body);
            let raw = forward(&inner);
            let canon = encapsulate::canonical(&inner).into_owned();
            let (media, cte, content) = forwarded(&raw);
            match encapsulate::classify(&canon) {
                Encapsulation::Global => {
                    proptest::prop_assert_eq!(media.as_str(), "message/global");
                    proptest::prop_assert_eq!(&content, &inner);
                }
                _ if cte.as_deref() == Some("base64") => {
                    // Clashed with every boundary tried: encoded.
                    proptest::prop_assert_eq!(&content, &inner);
                }
                _ => proptest::prop_assert_eq!(&content, &canon),
            }
            // As sent: no NUL, no line over 998 octets.
            proptest::prop_assert!(!raw.contains(&0));
            proptest::prop_assert!(
                raw.split(|b| *b == b'\n').all(|line| line.len() <= encapsulate::MAX_LINE + 1)
            );
        }
    }

    #[test]
    fn downgrade_turns_8bit_rfc822_into_global() {
        let inner = "Subject: Hola\r\n\r\nÑandú\r\n".as_bytes();
        let raw = forward(inner);
        let seven = encapsulate::downgrade_7bit(&raw).unwrap();
        assert!(seven.is_ascii());
        let (media, cte, content) = forwarded(&seven);
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/global", Some("base64"))
        );
        assert_eq!(content, inner);
        // Everything around the part is byte for byte the same.
        let parsed = nebula_mail::ParsedMessage::parse(&raw).unwrap();
        let (_, part) = parsed
            .parts()
            .find(|(_, part)| nebula_mail::media_type(part) == "message/rfc822")
            .unwrap();
        let start = usize::try_from(part.offset_header()).unwrap();
        let end = usize::try_from(part.offset_end()).unwrap();
        assert!(seven.starts_with(&raw[..start]));
        assert!(seven.ends_with(&raw[end..]));
        // The name stays.
        let text = String::from_utf8_lossy(&seven);
        assert!(
            text.contains("Content-Type: message/global; name=\"fwd.eml\""),
            "{text}"
        );
        let message = MessageParser::default().parse(&seven).unwrap();
        assert_eq!(message.body_text(0).as_deref(), Some("x"));
        assert_eq!(message.attachments().count(), 1);
    }

    #[test]
    fn downgrade_takes_global_and_binary_too() {
        // message/global in 8bit, as built for UTF-8 header fields.
        let inner = "Subject: Ñandú\r\n\r\nHola\r\n".as_bytes();
        let seven = encapsulate::downgrade_7bit(&forward(inner)).unwrap();
        let (media, cte, content) = forwarded(&seven);
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/global", Some("base64"))
        );
        assert_eq!(content, inner);
        // message/rfc822 in binary, from another client's draft.
        let draft = "From: a@x\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
            --b\r\nContent-Type: text/plain\r\n\r\nx\r\n\
            --b\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: binary\r\n\r\n\
            Subject: y\r\n\r\nñ\r\n--b--\r\n";
        let seven = encapsulate::downgrade_7bit(draft.as_bytes()).unwrap();
        let (media, cte, content) = forwarded(&seven);
        assert_eq!(
            (media.as_str(), cte.as_deref()),
            ("message/global", Some("base64"))
        );
        assert_eq!(content, "Subject: y\r\n\r\nñ".as_bytes());
        assert!(!String::from_utf8_lossy(&seven).contains("binary"));
    }

    #[test]
    fn sent_at_keeps_its_offset_in_the_date_field() {
        let draft = minimal(&json!({"sentAt": "2026-11-06T14:03:00+01:00"}));
        let raw = build(&draft, &Blobs::new(), 0).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("Date: Fri, 06 Nov 2026 14:03:00 +0100\r\n"),
            "{text}"
        );
    }

    #[test]
    fn downgrade_refuses_other_8bit() {
        let draft = minimal(&json!({
            "header:Subject": " Ñandú",
            "attachments": [{"blobId": "bu1", "type": "message/rfc822"}]
        }));
        let blobs = Blobs::from([(
            "bu1".parse().unwrap(),
            Arc::from("Subject: a\r\n\r\nñ\r\n".as_bytes()),
        )]);
        let raw = build(&draft, &blobs, 0).unwrap();
        assert!(!raw.is_ascii());
        assert!(encapsulate::downgrade_7bit(&raw).is_none());
        // 8 bits in a plain body: now re-encoded in base64 (RFC 6152 §3
        // allows converting instead of refusing).
        assert!(
            encapsulate::downgrade_7bit("Subject: a\r\n\r\nñ\r\n".as_bytes())
                .unwrap()
                .is_ascii()
        );
        assert!(encapsulate::downgrade_7bit(b"").is_none());
    }
}
