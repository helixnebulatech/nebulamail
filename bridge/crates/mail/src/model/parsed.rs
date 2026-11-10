// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A message parsed with `mail-parser`, and the part numbers JMAP sees.
//!
//! `mail-parser` 1.0 numbers the parts of every message in one list: the
//! parts of a nested `message/rfc822` come right after it. JMAP part ids
//! (and the blob ids built from them) count only the parts of the message
//! itself, in document order, a nested `message/rfc822` being one leaf: the
//! numbering of `mail-parser` 0.11, which clients may still hold. A part
//! number is the position in that list, so a part id never points into a
//! nested message, and never to another part than before.

use mail_parser::{Message, MessageParser, MessagePart, PartId, PartKind};

/// A parsed message and the parts of its root message.
#[derive(Debug)]
pub struct ParsedMessage<'x> {
    message: Message<'x>,
    /// `mail-parser` ids of the root message's parts, in document order
    /// (so sorted).
    parts: Vec<PartId>,
}

impl<'x> ParsedMessage<'x> {
    /// Parses a whole message. `None` for input that is not a message (no
    /// header and no blank line, empty, or 4 GiB or more). A panic inside
    /// `mail-parser` on hostile input (it has `unsafe` SIMD code and
    /// `overflow-checks` are on in release) also gives `None`: that message
    /// is unparsable instead of failing the request.
    #[must_use]
    pub fn parse(raw: &'x [u8]) -> Option<Self> {
        let message = std::panic::catch_unwind(|| MessageParser::default().parse(raw))
            .ok()
            .flatten()?;
        let parts = message.root().parts().map(|part| part.id()).collect();
        Some(Self { message, parts })
    }

    /// The message as `mail-parser` gives it (headers, raw bytes).
    #[must_use]
    pub fn message(&self) -> &Message<'x> {
        &self.message
    }

    /// Part number `index` of the root message.
    #[must_use]
    pub fn part(&self, index: u32) -> Option<MessagePart<'_>> {
        let id = *self.parts.get(usize::try_from(index).ok()?)?;
        self.message.part(id)
    }

    /// The number of a part of the root message; `None` for a part of a
    /// nested message.
    #[must_use]
    pub fn index(&self, part: &MessagePart<'_>) -> Option<u32> {
        let position = self.parts.binary_search(&part.id()).ok()?;
        u32::try_from(position).ok()
    }

    /// How many parts the root message has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Whether the root message has no part (never: it has its root part).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The parts of the root message with their numbers.
    pub fn parts(&self) -> impl Iterator<Item = (u32, MessagePart<'_>)> {
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(index, id)| Some((u32::try_from(index).ok()?, self.message.part(*id)?)))
    }

    /// The numbers of these parts (dropping any outside the root message).
    pub fn indexes<'p>(&self, parts: impl Iterator<Item = MessagePart<'p>>) -> Vec<u32> {
        parts.filter_map(|part| self.index(&part)).collect()
    }

    /// `textBody` (RFC 8621 §4.1.4), as part numbers.
    #[must_use]
    pub fn text_body(&self) -> Vec<u32> {
        self.indexes(self.message.text_body())
    }

    /// `htmlBody`, as part numbers.
    #[must_use]
    pub fn html_body(&self) -> Vec<u32> {
        self.indexes(self.message.html_body())
    }

    /// `attachments`, as part numbers.
    #[must_use]
    pub fn attachments(&self) -> Vec<u32> {
        self.indexes(self.message.attachments())
    }
}

/// `type/subtype` of a part, lower case. Without a Content-Type, the type
/// `mail-parser` read it as (`text/plain`, or `message/rfc822` in a
/// `multipart/digest`).
#[must_use]
pub fn media_type(part: &MessagePart<'_>) -> String {
    match part.content_type() {
        Some(ct) => match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub),
            None => ct.ctype().to_owned(),
        },
        None => match part.kind() {
            PartKind::Html => "text/html",
            PartKind::Message(_) => "message/rfc822",
            PartKind::Multipart => "multipart/mixed",
            PartKind::Text | PartKind::Binary | PartKind::InlineBinary => "text/plain",
        }
        .to_owned(),
    }
    .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_parts_keep_the_old_numbers() {
        // 0 mixed, 1 text, 2 message/rfc822 (its own 2 parts are not
        // numbered), 3 pdf.
        let raw = b"Subject: x\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: text/plain\r\n\r\nhola\r\n\
--B\r\nContent-Type: message/rfc822\r\n\r\n\
Subject: inner\r\nContent-Type: multipart/mixed; boundary=\"C\"\r\n\r\n\
--C\r\nContent-Type: text/plain\r\n\r\ninner\r\n--C--\r\n\
--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment\r\n\r\npdf\r\n--B--\r\n";
        let parsed = ParsedMessage::parse(raw).unwrap();
        assert_eq!(parsed.len(), 4);
        assert!(parsed.message().parts().len() > 4);
        assert!(parsed.part(2).unwrap().is_message());
        assert!(
            parsed
                .part(3)
                .unwrap()
                .is_content_type("application", "pdf")
        );
        assert!(parsed.part(4).is_none());
        assert_eq!(parsed.text_body(), vec![1]);
        assert_eq!(parsed.attachments(), vec![2, 3]);
        let inner = parsed.part(2).unwrap().nested().unwrap().root_part();
        assert_eq!(parsed.index(&inner), None);
    }

    #[test]
    fn not_a_message() {
        assert!(ParsedMessage::parse(b"").is_none());
        assert!(ParsedMessage::parse(b"no header and no blank line").is_none());
    }
}
