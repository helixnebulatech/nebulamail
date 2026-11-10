// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `UID FETCH` with typed attributes and CONDSTORE/QRESYNC modifiers.

use std::fmt;
use std::num::NonZeroU32;

use super::{Writer, command};
use crate::types::{ModSeq, UidSet};
use crate::{EncodeError, ValueError};

/// A header field name (RFC 5322: printable ASCII except `:`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeaderName(Box<str>);

impl HeaderName {
    /// Validates a field name such as `Message-ID`.
    ///
    /// # Errors
    ///
    /// Empty names or names with characters outside 33..=126 or `:`.
    pub fn new(name: &str) -> Result<Self, ValueError> {
        if name.is_empty() {
            return Err(ValueError::Empty("header name"));
        }
        if !name.bytes().all(|b| (33..=126).contains(&b) && b != b':') {
            return Err(ValueError::Invalid("header name"));
        }
        Ok(Self(name.into()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A MIME part number such as `1.2.3`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartPath(Vec<NonZeroU32>);

impl PartPath {
    /// Builds a path from part numbers (at least one).
    ///
    /// # Errors
    ///
    /// An empty path.
    pub fn new(parts: Vec<NonZeroU32>) -> Result<Self, ValueError> {
        if parts.is_empty() {
            return Err(ValueError::Empty("part path"));
        }
        Ok(Self(parts))
    }

    /// The part numbers.
    #[must_use]
    pub fn parts(&self) -> &[NonZeroU32] {
        &self.0
    }
}

impl fmt::Display for PartPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, part) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

/// The section of `BODY.PEEK[section]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Section {
    /// `[]`: the whole message.
    Full,
    /// `[HEADER]`
    Header,
    /// `[HEADER.FIELDS (...)]`: only these header fields.
    HeaderFields(Vec<HeaderName>),
    /// `[HEADER.FIELDS.NOT (...)]`: all header fields but these.
    HeaderFieldsNot(Vec<HeaderName>),
    /// `[TEXT]`: the body without the header.
    Text,
    /// `[1.2]`: the content of a MIME part.
    Part(PartPath),
    /// `[1.2.MIME]`: the MIME header of a part.
    PartMime(PartPath),
}

/// `<offset.length>`: only part of a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Partial {
    /// First byte.
    pub offset: u32,
    /// Number of bytes.
    pub length: NonZeroU32,
}

/// A FETCH data item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FetchAttribute {
    /// `UID` (always returned by UID FETCH anyway).
    Uid,
    /// `FLAGS`
    Flags,
    /// `INTERNALDATE`
    InternalDate,
    /// `RFC822.SIZE`
    Rfc822Size,
    /// `BODYSTRUCTURE`
    BodyStructure,
    /// `BODY.PEEK[section]<partial>`: does not set `\Seen`.
    BodyPeek {
        /// Which section.
        section: Section,
        /// Optional byte range.
        partial: Option<Partial>,
    },
    /// `BINARY.PEEK[part]<partial>` (RFC 3516): the part with its
    /// Content-Transfer-Encoding already decoded by the server; the whole
    /// message without a part. Does not set `\Seen`. Ask for it in a FETCH
    /// of its own (with UID, FLAGS or MODSEQ at most): the answer is read by
    /// our own parser, which only knows those items.
    BinaryPeek {
        /// The part (`None`: the whole message).
        part: Option<PartPath>,
        /// Optional byte range of the decoded data.
        partial: Option<Partial>,
    },
    /// `BINARY.SIZE[part]` (RFC 3516): the decoded size. Same rule as
    /// [`FetchAttribute::BinaryPeek`].
    BinarySize {
        /// The part (`None`: the whole message).
        part: Option<PartPath>,
    },
    /// `MODSEQ` (CONDSTORE).
    ModSeq,
    /// `EMAILID` (OBJECTID).
    EmailId,
    /// `THREADID` (OBJECTID).
    ThreadId,
    /// `OBJECTID` (`OBJECTID+` draft): email and thread ids in one item.
    ObjectIds,
    /// `X-GM-MSGID` (Gmail).
    GmailMsgId,
    /// `X-GM-THRID` (Gmail).
    GmailThreadId,
    /// `X-GM-LABELS` (Gmail).
    GmailLabels,
    /// `PREVIEW` (RFC 8970).
    Preview,
}

/// CONDSTORE/QRESYNC modifiers of FETCH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FetchModifiers {
    /// No modifiers.
    None,
    /// `(CHANGEDSINCE n)`: only messages changed after `n`.
    ChangedSince(ModSeq),
    /// `(CHANGEDSINCE n VANISHED)`: also report expunged UIDs (QRESYNC).
    ChangedSinceWithVanished(ModSeq),
}

/// `UID FETCH set (attributes) [modifiers]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidFetch<'a> {
    /// Which messages.
    pub uids: &'a UidSet,
    /// At least one attribute.
    pub attributes: &'a [FetchAttribute],
    /// CONDSTORE/QRESYNC modifiers.
    pub modifiers: FetchModifiers,
}

command!(UidFetch<'_>, "UID FETCH", |fetch, writer| {
    if fetch.attributes.is_empty() {
        return Err(EncodeError::Empty("FETCH attributes"));
    }
    writer.raw(b"UID FETCH ");
    writer.display(fetch.uids);
    writer.raw(b" (");
    for (index, attribute) in fetch.attributes.iter().enumerate() {
        if index > 0 {
            writer.space();
        }
        write_attribute(writer, attribute)?;
    }
    writer.raw(b")");
    match fetch.modifiers {
        FetchModifiers::None => {}
        FetchModifiers::ChangedSince(mod_seq) => {
            writer.raw(b" (CHANGEDSINCE ");
            writer.number(mod_seq.get());
            writer.raw(b")");
        }
        FetchModifiers::ChangedSinceWithVanished(mod_seq) => {
            writer.raw(b" (CHANGEDSINCE ");
            writer.number(mod_seq.get());
            writer.raw(b" VANISHED)");
        }
    }
    Ok(())
});

fn write_attribute(writer: &mut Writer, attribute: &FetchAttribute) -> Result<(), EncodeError> {
    let name: &[u8] = match attribute {
        FetchAttribute::BodyPeek { section, partial } => {
            writer.raw(b"BODY.PEEK[");
            write_section(writer, section)?;
            writer.raw(b"]");
            write_partial(writer, partial.as_ref());
            return Ok(());
        }
        FetchAttribute::BinaryPeek { part, partial } => {
            writer.raw(b"BINARY.PEEK[");
            if let Some(part) = part {
                writer.display(part);
            }
            writer.raw(b"]");
            write_partial(writer, partial.as_ref());
            return Ok(());
        }
        FetchAttribute::BinarySize { part } => {
            writer.raw(b"BINARY.SIZE[");
            if let Some(part) = part {
                writer.display(part);
            }
            writer.raw(b"]");
            return Ok(());
        }
        FetchAttribute::Uid => b"UID",
        FetchAttribute::Flags => b"FLAGS",
        FetchAttribute::InternalDate => b"INTERNALDATE",
        FetchAttribute::Rfc822Size => b"RFC822.SIZE",
        FetchAttribute::BodyStructure => b"BODYSTRUCTURE",
        FetchAttribute::ModSeq => b"MODSEQ",
        FetchAttribute::EmailId => b"EMAILID",
        FetchAttribute::ThreadId => b"THREADID",
        FetchAttribute::ObjectIds => b"OBJECTID",
        FetchAttribute::GmailMsgId => b"X-GM-MSGID",
        FetchAttribute::GmailThreadId => b"X-GM-THRID",
        FetchAttribute::GmailLabels => b"X-GM-LABELS",
        FetchAttribute::Preview => b"PREVIEW",
    };
    writer.raw(name);
    Ok(())
}

fn write_partial(writer: &mut Writer, partial: Option<&Partial>) {
    if let Some(partial) = partial {
        writer.raw(b"<");
        writer.number(partial.offset);
        writer.raw(b".");
        writer.number(partial.length.get());
        writer.raw(b">");
    }
}

fn write_section(writer: &mut Writer, section: &Section) -> Result<(), EncodeError> {
    match section {
        Section::Full => {}
        Section::Header => writer.raw(b"HEADER"),
        Section::HeaderFields(names) => {
            writer.raw(b"HEADER.FIELDS ");
            write_header_names(writer, names)?;
        }
        Section::HeaderFieldsNot(names) => {
            writer.raw(b"HEADER.FIELDS.NOT ");
            write_header_names(writer, names)?;
        }
        Section::Text => writer.raw(b"TEXT"),
        Section::Part(path) => writer.display(path),
        Section::PartMime(path) => {
            writer.display(path);
            writer.raw(b".MIME");
        }
    }
    Ok(())
}

fn write_header_names(writer: &mut Writer, names: &[HeaderName]) -> Result<(), EncodeError> {
    if names.is_empty() {
        return Err(EncodeError::Empty("header field list"));
    }
    writer.raw(b"(");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            writer.space();
        }
        // Field names are printable ASCII, so this is an atom or quoted.
        writer.astring(name.as_str().as_bytes(), "header name")?;
    }
    writer.raw(b")");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;
    use crate::types::Uid;

    #[test]
    fn encodes_attributes_and_modifiers() {
        let uids = UidSet::from_value(Uid::new(1).unwrap());
        let headers = vec![
            HeaderName::new("From").unwrap(),
            HeaderName::new("Message-ID").unwrap(),
        ];
        let attributes = [
            FetchAttribute::Uid,
            FetchAttribute::Flags,
            FetchAttribute::ModSeq,
            FetchAttribute::EmailId,
            FetchAttribute::BodyPeek {
                section: Section::HeaderFields(headers),
                partial: None,
            },
            FetchAttribute::BodyPeek {
                section: Section::PartMime(
                    PartPath::new(vec![
                        NonZeroU32::new(1).unwrap(),
                        NonZeroU32::new(2).unwrap(),
                    ])
                    .unwrap(),
                ),
                partial: Some(Partial {
                    offset: 0,
                    length: NonZeroU32::new(1024).unwrap(),
                }),
            },
        ];
        let fetch = UidFetch {
            uids: &uids,
            attributes: &attributes,
            modifiers: FetchModifiers::ChangedSinceWithVanished(ModSeq::new(12_345).unwrap()),
        };
        assert_eq!(
            basic(&fetch),
            "A1 UID FETCH 1:* (UID FLAGS MODSEQ EMAILID BODY.PEEK[HEADER.FIELDS (From Message-ID)] \
             BODY.PEEK[1.2.MIME]<0.1024>) (CHANGEDSINCE 12345 VANISHED)\r\n"
        );
    }

    #[test]
    fn encodes_binary() {
        let uids = UidSet::single(Uid::new(7).unwrap());
        let part = PartPath::new(vec![NonZeroU32::new(2).unwrap()]).unwrap();
        let attributes = [
            FetchAttribute::Uid,
            FetchAttribute::BinarySize {
                part: Some(part.clone()),
            },
            FetchAttribute::BinaryPeek {
                part: Some(part),
                partial: None,
            },
            FetchAttribute::BinaryPeek {
                part: None,
                partial: Some(Partial {
                    offset: 10,
                    length: NonZeroU32::new(5).unwrap(),
                }),
            },
        ];
        let fetch = UidFetch {
            uids: &uids,
            attributes: &attributes,
            modifiers: FetchModifiers::None,
        };
        assert_eq!(
            basic(&fetch),
            "A1 UID FETCH 7 (UID BINARY.SIZE[2] BINARY.PEEK[2] BINARY.PEEK[]<10.5>)\r\n"
        );
    }

    #[test]
    fn header_names_are_validated() {
        assert!(HeaderName::new("X-Foo").is_ok());
        assert!(HeaderName::new("Bad:Name").is_err());
        assert!(HeaderName::new("Bad Name").is_err());
        assert!(HeaderName::new("").is_err());
    }
}
