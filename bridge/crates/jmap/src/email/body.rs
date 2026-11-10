// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The MIME structure of a message as JMAP body parts (RFC 8621 §4.1.4).
//!
//! `mail-parser` already sorts the leaf parts into `text_body`,
//! `html_body` and `attachments` with the RFC 8621 §4.1.4 algorithm
//! (Stalwart's JMAP server uses those lists directly), so we only turn its
//! parts into `EmailBodyPart`s. A part's `partId` is its number in the root
//! message (`nebula_mail::ParsedMessage`), and its `blobId` is
//! `bp{email}_{number}` (see `blob`). Nested `message/rfc822` parts are
//! leaves: their insides are not shown, like Stalwart.

use std::collections::BTreeMap;

use mail_parser::{MessagePart, PartKind};
use nebula_jmap_proto::UnsignedInt;
use nebula_jmap_proto::object::email::{
    BodyPartField, EmailBodyPart, EmailBodyPartProperty, EmailBodyValue, HeaderProperties, PartId,
};
use nebula_mail::{ParsedMessage, PartSection};
use nebula_store::EmailId;

use super::headers;
use crate::blob::BlobRef;
use crate::error::Bug;

/// Deepest `subParts` nesting returned (a hostile message can nest
/// multiparts thousands deep; JMAP clients don't need that).
const MAX_DEPTH: usize = 32;

/// Parses a whole message (`None` if it isn't one, or if `mail-parser`
/// panics on it).
pub(crate) fn parse(raw: &[u8]) -> Option<ParsedMessage<'_>> {
    ParsedMessage::parse(raw)
}

/// `type/subtype` of a part, lower case, with the RFC 2045 defaults.
pub(crate) fn media_type(part: &MessagePart<'_>) -> String {
    nebula_mail::media_type(part)
}

/// The content of a leaf part: its body with the transfer encoding
/// decoded (RFC 8621 `blobId`), in its own charset. A nested message is
/// the message itself.
fn part_bytes(part: &MessagePart<'_>) -> Vec<u8> {
    part.decoded().into_owned()
}

/// The decoded content and media type of part `index` (for downloads).
/// `None` for multiparts and unknown parts.
pub(crate) fn part_content(raw: &[u8], index: u32) -> Option<(Vec<u8>, String)> {
    // `parse` already protects against mail-parser's panics.
    let message = parse(raw)?;
    let part = message.part(index)?;
    if part.is_multipart() {
        return None;
    }
    Some((part_bytes(&part), media_type(&part)))
}

/// Whose parts these are: a stored email's (`bp…`, `bs…` blob ids), or
/// those of a message in a blob (`Email/parse`, `bn…`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum PartOwner<'m> {
    Email(EmailId),
    Blob(&'m BlobRef),
}

/// What to put in each body part.
pub(crate) struct PartBuilder<'m, 'x> {
    pub(crate) message: &'m ParsedMessage<'x>,
    pub(crate) owner: PartOwner<'m>,
    pub(crate) properties: &'m [EmailBodyPartProperty],
    /// Parts that can be streamed from their IMAP section (big messages).
    pub(crate) sections: &'m BTreeMap<u32, PartSection>,
}

impl PartBuilder<'_, '_> {
    fn wants(&self, field: BodyPartField) -> bool {
        self.properties
            .iter()
            .any(|p| matches!(p, EmailBodyPartProperty::Field(f) if *f == field))
    }

    /// The blob of leaf part `index`: by IMAP section if it can be
    /// streamed.
    fn blob_ref(&self, index: u32) -> BlobRef {
        match self.owner {
            PartOwner::Email(email) => match self.sections.get(&index) {
                Some(section) => BlobRef::Section(email, index, section.clone()),
                None => BlobRef::Part(email, index),
            },
            PartOwner::Blob(blob) => blob.clone().nested(index),
        }
    }

    /// Part `index` (and, for `bodyStructure`, its children).
    pub(crate) fn part(&self, index: u32, depth: usize) -> Result<Option<EmailBodyPart>, Bug> {
        let Some(part) = self.message.part(index) else {
            return Ok(None);
        };
        let leaf = !part.is_multipart();
        let mut out = EmailBodyPart::default();
        if self.wants(BodyPartField::PartId) {
            out.part_id = Some(
                leaf.then(|| index.to_string().parse::<PartId>())
                    .transpose()
                    .map_err(|_| Bug("part id"))?,
            );
        }
        if self.wants(BodyPartField::BlobId) {
            out.blob_id = Some(
                leaf.then(|| self.blob_ref(index).to_blob_id())
                    .transpose()?,
            );
        }
        if self.wants(BodyPartField::Size) {
            let size = if leaf { part.decoded_len() } else { 0 };
            out.size = Some(UnsignedInt::try_from(size).unwrap_or(UnsignedInt::MAX));
        }
        if self.wants(BodyPartField::Headers) {
            out.headers = Some(headers::all_raw(part.headers()));
        }
        if self.wants(BodyPartField::Name) {
            out.name = Some(part.attachment_name().map(str::to_owned));
        }
        if self.wants(BodyPartField::Type) {
            out.media_type = Some(media_type(&part));
        }
        if self.wants(BodyPartField::Charset) {
            let declared = part
                .content_type()
                .and_then(|ct| ct.attribute("charset"))
                .map(str::to_owned);
            out.charset = Some(declared.or_else(|| part.is_text().then(|| "us-ascii".to_owned())));
        }
        if self.wants(BodyPartField::Disposition) {
            out.disposition = Some(
                part.content_disposition()
                    .map(|cd| cd.ctype().to_ascii_lowercase()),
            );
        }
        if self.wants(BodyPartField::Cid) {
            out.cid = Some(
                part.content_id()
                    .map(|id| id.trim_start_matches('<').trim_end_matches('>').to_owned()),
            );
        }
        if self.wants(BodyPartField::Language) {
            out.language = Some(
                part.content_language()
                    .map(|list| list.iter().map(str::to_owned).collect()),
            );
        }
        if self.wants(BodyPartField::Location) {
            out.location = Some(part.content_location().map(str::to_owned));
        }
        let mut header_properties = HeaderProperties::new();
        for property in self.properties {
            if let EmailBodyPartProperty::Header(header) = property {
                header_properties.insert(header.clone(), headers::property(part.headers(), header));
            }
        }
        out.header_properties = header_properties;
        if self.wants(BodyPartField::SubParts) {
            out.sub_parts = Some(match leaf {
                false if depth < MAX_DEPTH => {
                    let mut list = Vec::with_capacity(part.children().len());
                    for child in self.message.indexes(part.children()) {
                        if let Some(child) = self.part(child, depth.saturating_add(1))? {
                            list.push(child);
                        }
                    }
                    Some(list)
                }
                false => Some(Vec::new()),
                true => None,
            });
        }
        Ok(Some(out))
    }

    /// Leaf parts by index (`textBody`, `htmlBody`, `attachments`).
    pub(crate) fn list(&self, indexes: &[u32]) -> Result<Vec<EmailBodyPart>, Bug> {
        let mut out = Vec::with_capacity(indexes.len());
        for index in indexes {
            if let Some(part) = self.part(*index, MAX_DEPTH)? {
                out.push(part);
            }
        }
        Ok(out)
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary.
fn truncate(text: &str, max: usize) -> (String, bool) {
    if max == 0 || text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (text.get(..end).unwrap_or_default().to_owned(), true)
}

/// `bodyValues` for these parts (only text parts have one).
pub(crate) fn values(
    message: &ParsedMessage<'_>,
    indexes: impl IntoIterator<Item = u32>,
    max_bytes: usize,
) -> Result<BTreeMap<PartId, EmailBodyValue>, Bug> {
    let mut out = BTreeMap::new();
    for index in indexes {
        let Some(part) = message.part(index) else {
            continue;
        };
        let Some((text, problems)) = part.text_checked() else {
            continue;
        };
        let (value, is_truncated) = truncate(&text, max_bytes);
        let id: PartId = index.to_string().parse().map_err(|_| Bug("part id"))?;
        out.insert(
            id,
            EmailBodyValue {
                value,
                // RFC 8621: malformed transfer encoding, an unknown charset
                // or bytes not valid in it (`mail-parser` decodes anyway).
                is_encoding_problem: !problems.is_empty(),
                is_truncated,
            },
        );
    }
    Ok(out)
}

/// Every text part (`fetchAllBodyValues`).
pub(crate) fn text_parts(message: &ParsedMessage<'_>) -> Vec<u32> {
    message
        .parts()
        .filter(|(_, part)| matches!(part.kind(), PartKind::Text | PartKind::Html))
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const MIXED: &[u8] = b"From: a@example.com\r\nSubject: x\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
--A\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHola \xc3\xb1and\xc3\xba\r\n\
--A\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hola</p>\r\n--A--\r\n\
--B\r\nContent-Type: application/pdf; name=\"f.pdf\"\r\n\
Content-Disposition: attachment; filename=\"f.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--B--\r\n";

    fn all_fields() -> Vec<EmailBodyPartProperty> {
        let mut list: Vec<EmailBodyPartProperty> = EmailBodyPartProperty::DEFAULT
            .iter()
            .map(|f| EmailBodyPartProperty::Field(*f))
            .collect();
        list.push(EmailBodyPartProperty::Field(BodyPartField::SubParts));
        list
    }

    #[test]
    fn structure_and_lists() {
        let message = parse(MIXED).unwrap();
        let properties = all_fields();
        let sections = BTreeMap::new();
        let builder = PartBuilder {
            message: &message,
            owner: PartOwner::Email(EmailId::from_raw(9).unwrap()),
            properties: &properties,
            sections: &sections,
        };
        let root = serde_json::to_value(builder.part(0, 0).unwrap().unwrap()).unwrap();
        assert_eq!(root["type"], "multipart/mixed");
        assert_eq!(root["partId"], json!(null));
        assert_eq!(root["subParts"][0]["type"], "multipart/alternative");
        assert_eq!(root["subParts"][0]["subParts"][1]["type"], "text/html");
        let text = builder.list(&message.text_body()).unwrap();
        assert_eq!(text.len(), 1);
        let text = serde_json::to_value(&text[0]).unwrap();
        assert_eq!(text["type"], "text/plain");
        assert_eq!(text["charset"], "utf-8");
        let attachments =
            serde_json::to_value(builder.list(&message.attachments()).unwrap()).unwrap();
        assert_eq!(attachments[0]["name"], "f.pdf");
        assert_eq!(attachments[0]["disposition"], "attachment");
        assert_eq!(attachments[0]["size"], 5);
        let blob = attachments[0]["blobId"].as_str().unwrap();
        let index = blob.rsplit('_').next().unwrap().parse::<u32>().unwrap();
        assert_eq!(
            part_content(MIXED, index).unwrap(),
            (b"Hello".to_vec(), "application/pdf".to_owned())
        );
        assert!(part_content(MIXED, 0).is_none());
    }

    #[test]
    fn body_values_are_cut_on_characters() {
        let message = parse(MIXED).unwrap();
        let values = values(&message, message.text_body(), 7).unwrap();
        let value = values.values().next().unwrap();
        assert!(value.is_truncated);
        // "Hola ñ" is 7 bytes; "Hola ña" would be 8.
        assert_eq!(value.value, "Hola ñ");
        assert_eq!(truncate("abc", 0), ("abc".to_owned(), false));
        assert_eq!(text_parts(&message).len(), 2);
    }
}
