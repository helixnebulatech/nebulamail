// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! FETCH data: owned and validated.

use std::borrow::Cow;

use imap_proto::{AttributeValue, MessageSection, SectionPath};

use super::Invalid;
use super::body::{BodyStructure, convert_body_structure};
use crate::ValueError;
use crate::types::{Flag, InternalDate, ModSeq, SeqNum, Uid};

/// An object id of RFC 8474 (`EMAILID`, `THREADID`, `MAILBOXID`):
/// 1 to 255 letters, digits, `_` or `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(Box<str>);

impl ObjectId {
    /// Validates an object id.
    ///
    /// # Errors
    ///
    /// Empty, longer than 255, or with other characters.
    pub fn new(text: &str) -> Result<Self, ValueError> {
        if text.is_empty() {
            return Err(ValueError::Empty("object id"));
        }
        if text.len() > 255 {
            return Err(ValueError::OutOfRange("object id"));
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ValueError::Invalid("object id"));
        }
        Ok(Self(text.into()))
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `THREADID` value: an id, or `NIL` when the message has no thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ThreadId {
    /// The thread id.
    Id(ObjectId),
    /// `NIL`
    None,
}

/// `PREVIEW` value (RFC 8970): text, or `NIL` if the server has none yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Preview {
    /// The preview text (at most 256 characters per the RFC).
    Text(String),
    /// `NIL`
    None,
}

/// Which section a `BODY[...]` response holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SectionKind {
    /// `BODY[]`
    Full,
    /// `BODY[HEADER]` or `BODY[HEADER.FIELDS (...)]` (`imap-proto` does not
    /// tell them apart; the client knows what it asked for).
    Header,
    /// `BODY[TEXT]`
    Text,
    /// `BODY[1.2]`
    Part(Vec<u32>),
    /// `BODY[1.2.HEADER]`
    PartHeader(Vec<u32>),
    /// `BODY[1.2.TEXT]`
    PartText(Vec<u32>),
    /// `BODY[1.2.MIME]`
    PartMime(Vec<u32>),
}

/// The data of a `BODY[section]<origin>` response item.
#[derive(Clone, PartialEq, Eq)]
pub struct BodySection {
    /// Which section.
    pub section: SectionKind,
    /// `<origin>` for partial fetches.
    pub origin: Option<u32>,
    /// The bytes (`None` for `NIL`). Message content: never log it.
    pub data: Option<Vec<u8>>,
}

impl std::fmt::Debug for BodySection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodySection")
            .field("section", &self.section)
            .field("origin", &self.origin)
            .field("len", &self.data.as_ref().map(Vec::len))
            .finish()
    }
}

/// The data of a `BINARY[part]<origin>` response item (RFC 3516): the
/// part with its transfer encoding removed.
#[derive(Clone, PartialEq, Eq)]
pub struct BinarySection {
    /// The part numbers (empty: the whole message).
    pub part: Vec<u32>,
    /// `<origin>` for partial fetches.
    pub origin: Option<u32>,
    /// The decoded bytes (`None` for `NIL`). Message content: never log it.
    pub data: Option<Vec<u8>>,
}

impl std::fmt::Debug for BinarySection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinarySection")
            .field("part", &self.part)
            .field("origin", &self.origin)
            .field("len", &self.data.as_ref().map(Vec::len))
            .finish()
    }
}

/// One `* n FETCH (...)` response. Fields the server did not send are `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FetchData {
    /// Sequence number.
    pub seq: SeqNum,
    /// `UID`
    pub uid: Option<Uid>,
    /// `FLAGS`
    pub flags: Option<Vec<Flag>>,
    /// `INTERNALDATE`
    pub internal_date: Option<InternalDate>,
    /// `RFC822.SIZE`
    pub size: Option<u32>,
    /// `BODYSTRUCTURE`
    pub body_structure: Option<BodyStructure>,
    /// `MODSEQ`
    pub mod_seq: Option<ModSeq>,
    /// `EMAILID`
    pub email_id: Option<ObjectId>,
    /// `THREADID`
    pub thread_id: Option<ThreadId>,
    /// `X-GM-MSGID`
    pub gmail_msg_id: Option<u64>,
    /// `X-GM-THRID`
    pub gmail_thread_id: Option<u64>,
    /// `X-GM-LABELS`, as sent (mailbox names in wire form or `\Inbox`…).
    pub gmail_labels: Option<Vec<String>>,
    /// `PREVIEW`
    pub preview: Option<Preview>,
    /// `BODY[...]` items.
    pub sections: Vec<BodySection>,
    /// `BINARY[...]` items.
    pub binary: Vec<BinarySection>,
    /// `BINARY.SIZE[...]` items: part numbers (empty: whole message) and
    /// decoded size.
    pub binary_sizes: Vec<(Vec<u32>, u32)>,
}

impl FetchData {
    pub(crate) fn empty(seq: SeqNum) -> Self {
        Self {
            seq,
            uid: None,
            flags: None,
            internal_date: None,
            size: None,
            body_structure: None,
            mod_seq: None,
            email_id: None,
            thread_id: None,
            gmail_msg_id: None,
            gmail_thread_id: None,
            gmail_labels: None,
            preview: None,
            sections: Vec::new(),
            binary: Vec::new(),
            binary_sizes: Vec::new(),
        }
    }
}

/// FETCH items that `imap-proto` skips, read by our own tokenizer:
/// `PREVIEW` and the `OBJECTID (...)` item of the `OBJECTID+` draft.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FetchExtras {
    pub(crate) preview: Option<Preview>,
    pub(crate) email_id: Option<ObjectId>,
    pub(crate) thread_id: Option<ThreadId>,
}

/// Converts the attributes of `imap-proto`, plus the extras.
pub(crate) fn convert_fetch(
    seq: u32,
    attributes: Vec<AttributeValue<'_>>,
    extras: FetchExtras,
) -> Result<FetchData, Invalid> {
    let seq = SeqNum::new(seq).ok_or(Invalid("sequence number"))?;
    let mut data = FetchData::empty(seq);
    data.preview = extras.preview;
    data.email_id = extras.email_id;
    data.thread_id = extras.thread_id;
    for attribute in attributes {
        add_attribute(&mut data, attribute)?;
    }
    Ok(data)
}

fn add_attribute(data: &mut FetchData, attribute: AttributeValue<'_>) -> Result<(), Invalid> {
    match attribute {
        AttributeValue::Uid(uid) => data.uid = Some(Uid::new(uid).ok_or(Invalid("UID"))?),
        AttributeValue::Flags(flags) => data.flags = Some(convert_flags(&flags)),
        // A date we cannot read (or outside the years 1 to 9999 in UTC) is
        // left out: callers fall back to the Date header, and the rest of
        // the FETCH (UID, flags…) is not lost.
        AttributeValue::InternalDate(text) => data.internal_date = text.parse().ok(),
        AttributeValue::Rfc822Size(size) => data.size = Some(size),
        AttributeValue::BodyStructure(body) => {
            data.body_structure = Some(convert_body_structure(body));
        }
        AttributeValue::ModSeq(mod_seq) => {
            data.mod_seq = Some(ModSeq::new(mod_seq).ok_or(Invalid("MODSEQ"))?);
        }
        AttributeValue::EmailId(id) => {
            data.email_id = Some(ObjectId::new(&id).map_err(|_| Invalid("EMAILID"))?);
        }
        AttributeValue::ThreadId(id) => {
            data.thread_id = Some(match id {
                Some(id) => ThreadId::Id(ObjectId::new(&id).map_err(|_| Invalid("THREADID"))?),
                None => ThreadId::None,
            });
        }
        AttributeValue::GmailMsgId(id) => data.gmail_msg_id = Some(id),
        AttributeValue::GmailThrId(id) => data.gmail_thread_id = Some(id),
        AttributeValue::GmailLabels(labels) => {
            data.gmail_labels = Some(labels.into_iter().map(Cow::into_owned).collect());
        }
        AttributeValue::BodySection {
            section,
            index,
            data: bytes,
        } => {
            data.sections.push(BodySection {
                section: convert_section(section),
                origin: index,
                data: bytes.map(Cow::into_owned),
            });
        }
        // We never ask for ENVELOPE or the RFC822 items, and unknown items
        // (such as SAVEDATE) are already skipped by `imap-proto`. The enum is
        // non-exhaustive, so new variants also land here.
        _ => {}
    }
    Ok(())
}

/// The flags we can represent. A received flag that breaks the keyword
/// rules (too long, odd characters) is dropped and the others are kept: a
/// single bad keyword must not hide the whole FETCH. Dropping is safe
/// because the client only ever adds or removes flags (`+FLAGS`/`-FLAGS`),
/// never replaces the list, so it cannot erase what it did not read.
/// Not logged: this crate has no logging dependency, and the flag text
/// could be private anyway.
pub(crate) fn convert_flags<T: AsRef<str>>(flags: &[T]) -> Vec<Flag> {
    flags
        .iter()
        .filter_map(|flag| Flag::parse(flag.as_ref()).ok())
        .collect()
}

fn convert_section(section: Option<SectionPath>) -> SectionKind {
    match section {
        None => SectionKind::Full,
        // `BODY[MIME]` is not valid without a part; treat it as the header.
        Some(SectionPath::Full(MessageSection::Header | MessageSection::Mime)) => {
            SectionKind::Header
        }
        Some(SectionPath::Full(MessageSection::Text)) => SectionKind::Text,
        Some(SectionPath::Part(parts, None)) => SectionKind::Part(parts),
        Some(SectionPath::Part(parts, Some(MessageSection::Header))) => {
            SectionKind::PartHeader(parts)
        }
        Some(SectionPath::Part(parts, Some(MessageSection::Text))) => SectionKind::PartText(parts),
        Some(SectionPath::Part(parts, Some(MessageSection::Mime))) => SectionKind::PartMime(parts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids() {
        assert!(ObjectId::new("M6d99ac3275bb4e").is_ok());
        assert!(ObjectId::new("a b").is_err());
        assert!(ObjectId::new("").is_err());
        assert!(ObjectId::new(&"a".repeat(256)).is_err());
    }
}
