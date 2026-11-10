// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Which `mail-parser` part is which IMAP section, checked.
//!
//! JMAP part ids are part numbers of the root message
//! ([`super::parsed`]); IMAP numbers parts by their place in the MIME tree
//! (`1.2`). Both number a well-formed message the same way (part numbers
//! are the tree in pre-order, a nested `message/rfc822` being one leaf),
//! but they can disagree on a broken one
//! (a missing boundary, a multipart without `boundary=`…), and a download
//! must never give another part. So a part may only be fetched by its IMAP
//! section if, with the whole message at hand, everything agrees:
//!
//! - the tree has the same shape (each multipart the same children, same
//!   number of parts in total) and every part the same media type;
//! - the leaf has the same transfer encoding and the server's size of its
//!   encoded body is the length of the bytes `mail-parser` read for it;
//! - decoding those bytes with our piece-by-piece decoder gives exactly
//!   `mail-parser`'s transfer-decoded body (so its `size` is the one shown
//!   to clients).
//!
//! Any disagreement in the shape and no part is streamed; in a leaf, only
//! that part isn't. Those parts are downloaded as before, from the whole
//! message.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use mail_parser::{Encoding as ParsedEncoding, MessagePart};
use nebula_imap::proto::command::PartPath;
use nebula_imap::proto::response::{BodyStructure, SinglePart, TransferEncoding};

use super::parsed::{ParsedMessage, media_type};
use super::transfer::{Decoder, Encoding};

/// The MIME structure the server gave (`BODYSTRUCTURE`) of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageStructure(pub(crate) BodyStructure);

/// Where a part of a message is on the IMAP server, and its size once
/// decoded: what a download needs to stream it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartSection {
    path: PartPath,
    size: u64,
}

impl PartSection {
    /// A section. `None` for an empty path or a size of 0 (nothing to
    /// stream).
    #[must_use]
    pub fn new(path: Vec<NonZeroU32>, size: u64) -> Option<Self> {
        if size == 0 {
            return None;
        }
        Some(Self {
            path: PartPath::new(path).ok()?,
            size,
        })
    }

    /// The IMAP part numbers (`[1, 2]` for `1.2`).
    #[must_use]
    pub fn path(&self) -> &[NonZeroU32] {
        self.path.parts()
    }

    /// The decoded size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn part_path(&self) -> &PartPath {
        &self.path
    }
}

/// The transfer encoding we can decode, if the server and `mail-parser`
/// agree on it.
pub(crate) fn encoding_of(encoding: &TransferEncoding) -> Option<Encoding> {
    match encoding {
        TransferEncoding::SevenBit | TransferEncoding::EightBit | TransferEncoding::Binary => {
            Some(Encoding::Identity)
        }
        TransferEncoding::Base64 => Some(Encoding::Base64),
        TransferEncoding::QuotedPrintable => Some(Encoding::QuotedPrintable),
        _ => None,
    }
}

fn parsed_encoding(part: &MessagePart<'_>) -> Encoding {
    match part.encoding() {
        ParsedEncoding::Base64 => Encoding::Base64,
        ParsedEncoding::QuotedPrintable => Encoding::QuotedPrintable,
        ParsedEncoding::None => Encoding::Identity,
    }
}

/// `type/subtype` as the server gives it.
pub(crate) fn server_type(media_type: &str, subtype: &str) -> String {
    format!("{media_type}/{subtype}")
}

/// Finds a part by its IMAP path; `[1]` of a message that is not
/// multipart is its body. `in_multipart` tells whether it is inside one.
pub(crate) fn find<'a>(
    structure: &'a BodyStructure,
    path: &[NonZeroU32],
) -> Option<(&'a SinglePart, bool)> {
    match structure {
        BodyStructure::Single(part) => (path.len() == 1
            && path.first().is_some_and(|n| n.get() == 1))
        .then_some((part.as_ref(), false)),
        BodyStructure::Multipart(_) => {
            let mut node = structure;
            for number in path {
                let BodyStructure::Multipart(multipart) = node else {
                    return None;
                };
                let index = usize::try_from(number.get().checked_sub(1)?).ok()?;
                node = multipart.parts.get(index)?;
            }
            match node {
                BodyStructure::Single(part) => Some((part.as_ref(), true)),
                _ => None,
            }
        }
        _ => None,
    }
}

struct Walk<'m, 'x> {
    message: &'m ParsedMessage<'x>,
    next: u32,
    agrees: bool,
    out: BTreeMap<u32, PartSection>,
}

impl Walk<'_, '_> {
    fn node(&mut self, node: &BodyStructure, path: &mut Vec<NonZeroU32>, depth: usize) {
        let index = self.next;
        self.next = self.next.saturating_add(1);
        let Some(part) = self.message.part(index) else {
            self.agrees = false;
            return;
        };
        // Deeper than this, the IMAP layer has no BODYSTRUCTURE anyway.
        if depth > 64 {
            self.agrees = false;
            return;
        }
        match node {
            BodyStructure::Multipart(multipart) if part.is_multipart() => {
                let same_type =
                    server_type(&multipart.common.media_type, &multipart.common.subtype)
                        == media_type(&part);
                if !same_type || part.children().len() != multipart.parts.len() {
                    self.agrees = false;
                    return;
                }
                for (position, (child, child_part)) in
                    multipart.parts.iter().zip(part.children()).enumerate()
                {
                    // Children are numbered in pre-order: the next number.
                    if self.message.index(&child_part) != Some(self.next) {
                        self.agrees = false;
                        return;
                    }
                    let Some(number) = u32::try_from(position)
                        .ok()
                        .and_then(|p| p.checked_add(1))
                        .and_then(NonZeroU32::new)
                    else {
                        self.agrees = false;
                        return;
                    };
                    path.push(number);
                    self.node(child, path, depth.saturating_add(1));
                    path.pop();
                    if !self.agrees {
                        return;
                    }
                }
            }
            BodyStructure::Single(single) if !part.is_multipart() => {
                if server_type(&single.common.media_type, &single.common.subtype)
                    != media_type(&part)
                {
                    self.agrees = false;
                    return;
                }
                if let Some(section) = Self::leaf(single, &part, path) {
                    self.out.insert(index, section);
                }
            }
            _ => self.agrees = false,
        }
    }

    /// A leaf's section, if it can be streamed.
    fn leaf(
        single: &SinglePart,
        part: &MessagePart<'_>,
        path: &[NonZeroU32],
    ) -> Option<PartSection> {
        let in_multipart = !path.is_empty();
        if part.has_problems() || (!in_multipart && single.common.media_type == "message") {
            return None;
        }
        let encoding = encoding_of(&single.encoding)?;
        if encoding != parsed_encoding(part) {
            return None;
        }
        // The body as written in the message (a part of the root message:
        // its offsets are in the raw input).
        let encoded = part.raw_body();
        if usize::try_from(single.size).ok()? != encoded.len() {
            return None;
        }
        let decoded = Decoder::decode_all(encoding, encoded);
        if decoded != part.decoded().as_ref() {
            return None;
        }
        let at = if in_multipart {
            path.to_vec()
        } else {
            vec![NonZeroU32::MIN]
        };
        PartSection::new(at, u64::try_from(decoded.len()).ok()?)
    }
}

/// The parts of `message` that can be downloaded by IMAP section, by part
/// number. Empty if the server's structure disagrees.
#[must_use]
pub fn streamable_parts(
    message: &ParsedMessage<'_>,
    structure: &MessageStructure,
) -> BTreeMap<u32, PartSection> {
    let mut walk = Walk {
        message,
        next: 0,
        agrees: true,
        out: BTreeMap::new(),
    };
    walk.node(&structure.0, &mut Vec::new(), 0);
    if walk.agrees && usize::try_from(walk.next).is_ok_and(|next| next == message.len()) {
        walk.out
    } else {
        BTreeMap::new()
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::{Multipart, PartCommon};

    use super::*;

    const MIXED: &[u8] = b"From: a@example.com\r\nSubject: x\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
--A\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHola \xc3\xb1and\xc3\xba\r\n\
--A\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<p>Hola=20</p>\r\n--A--\r\n\
--B\r\nContent-Type: application/pdf; name=\"f.pdf\"\r\n\
Content-Disposition: attachment; filename=\"f.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVs\r\nbG8=\r\n--B--\r\n";

    fn common(media_type: &str, subtype: &str) -> PartCommon {
        PartCommon {
            media_type: media_type.to_owned(),
            subtype: subtype.to_owned(),
            params: Vec::new(),
            disposition: None,
            language: Vec::new(),
            location: None,
        }
    }

    fn leaf(
        media_type: &str,
        subtype: &str,
        encoding: TransferEncoding,
        size: u32,
    ) -> BodyStructure {
        BodyStructure::Single(Box::new(SinglePart {
            common: common(media_type, subtype),
            id: None,
            description: None,
            md5: None,
            encoding,
            size,
            lines: None,
            message: None,
        }))
    }

    fn multi(subtype: &str, parts: Vec<BodyStructure>) -> BodyStructure {
        BodyStructure::Multipart(Multipart {
            common: common("multipart", subtype),
            parts,
        })
    }

    fn mixed(pdf_size: u32, html: TransferEncoding) -> MessageStructure {
        MessageStructure(multi(
            "mixed",
            vec![
                multi(
                    "alternative",
                    vec![
                        leaf("text", "plain", TransferEncoding::SevenBit, 12),
                        leaf("text", "html", html, 14),
                    ],
                ),
                leaf("application", "pdf", TransferEncoding::Base64, pdf_size),
            ],
        ))
    }

    fn path(numbers: &[u32]) -> Vec<NonZeroU32> {
        numbers
            .iter()
            .map(|n| NonZeroU32::new(*n).unwrap())
            .collect()
    }

    #[test]
    fn same_tree_gives_sections() {
        let message = ParsedMessage::parse(MIXED).unwrap();
        let found = streamable_parts(&message, &mixed(10, TransferEncoding::QuotedPrintable));
        let summary: Vec<(u32, Vec<NonZeroU32>, u64)> = found
            .iter()
            .map(|(i, s)| (*i, s.path().to_vec(), s.size()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (2, path(&[1, 1]), 12),
                (3, path(&[1, 2]), 12),
                (4, path(&[2]), 5),
            ]
        );
        let tree = mixed(10, TransferEncoding::Base64);
        assert!(find(&tree.0, &path(&[2])).is_some_and(|(_, nested)| nested));
        assert!(find(&tree.0, &path(&[1])).is_none(), "a multipart");
        assert!(find(&tree.0, &path(&[3])).is_none());
    }

    #[test]
    fn a_different_leaf_is_left_out() {
        let message = ParsedMessage::parse(MIXED).unwrap();
        // Another size for the attachment, another encoding for the HTML.
        let found = streamable_parts(&message, &mixed(11, TransferEncoding::Base64));
        assert_eq!(found.keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn a_different_shape_gives_nothing() {
        let message = ParsedMessage::parse(MIXED).unwrap();
        let fewer = MessageStructure(multi(
            "mixed",
            vec![
                leaf("text", "plain", TransferEncoding::SevenBit, 12),
                leaf("application", "pdf", TransferEncoding::Base64, 10),
            ],
        ));
        assert!(streamable_parts(&message, &fewer).is_empty());
        let other_type = MessageStructure(multi(
            "related",
            vec![
                multi(
                    "alternative",
                    vec![
                        leaf("text", "plain", TransferEncoding::SevenBit, 12),
                        leaf("text", "html", TransferEncoding::QuotedPrintable, 14),
                    ],
                ),
                leaf("application", "pdf", TransferEncoding::Base64, 10),
            ],
        ));
        assert!(streamable_parts(&message, &other_type).is_empty());
        // A server that sees one part where mail-parser sees a tree.
        let flat = MessageStructure(leaf("text", "plain", TransferEncoding::SevenBit, 10));
        assert!(streamable_parts(&message, &flat).is_empty());
    }

    #[test]
    fn single_part_message_is_section_one() {
        let raw = b"Subject: x\r\nContent-Type: application/zip\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n";
        let message = ParsedMessage::parse(raw).unwrap();
        let structure = MessageStructure(leaf("application", "zip", TransferEncoding::Base64, 10));
        let found = streamable_parts(&message, &structure);
        let section = found.get(&0).unwrap();
        assert_eq!(section.path(), path(&[1]).as_slice());
        assert_eq!(section.size(), 5);
        assert!(find(&structure.0, &path(&[1])).is_some_and(|(_, nested)| !nested));
        // A multipart whose boundary never appears: mail-parser reads text.
        let broken =
            b"Subject: x\r\nContent-Type: multipart/mixed; boundary=\"Z\"\r\n\r\nhello\r\n";
        let message = ParsedMessage::parse(broken).unwrap();
        let structure = MessageStructure(multi(
            "mixed",
            vec![leaf("text", "plain", TransferEncoding::SevenBit, 7)],
        ));
        assert!(streamable_parts(&message, &structure).is_empty());
    }
}
