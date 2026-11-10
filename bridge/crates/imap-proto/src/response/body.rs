// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BODYSTRUCTURE: an owned tree of MIME parts.

use std::borrow::Cow;

use imap_proto::{BodyContentCommon, BodyContentSinglePart, BodyParams, ContentEncoding};

/// `Content-Transfer-Encoding` of a part.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransferEncoding {
    /// `7bit` (also used when the server sends NIL).
    SevenBit,
    /// `8bit`
    EightBit,
    /// `binary`
    Binary,
    /// `base64`
    Base64,
    /// `quoted-printable`
    QuotedPrintable,
    /// Anything else, lower-cased.
    Other(String),
}

/// `Content-Disposition` of a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disposition {
    /// `attachment`, `inline`… (lower-cased).
    pub kind: String,
    /// Parameters such as `filename`.
    pub params: Vec<(String, String)>,
}

/// Fields shared by every part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartCommon {
    /// Media type, lower-cased (`text`, `multipart`…).
    pub media_type: String,
    /// Media subtype, lower-cased (`plain`, `mixed`…).
    pub subtype: String,
    /// `Content-Type` parameters (`charset`, `boundary`, `name`…).
    pub params: Vec<(String, String)>,
    /// `Content-Disposition`.
    pub disposition: Option<Disposition>,
    /// `Content-Language` tags.
    pub language: Vec<String>,
    /// `Content-Location`.
    pub location: Option<String>,
}

/// A leaf part (not multipart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinglePart {
    /// Type, parameters, disposition…
    pub common: PartCommon,
    /// `Content-ID`.
    pub id: Option<String>,
    /// `Content-Description`.
    pub description: Option<String>,
    /// `Content-MD5`.
    pub md5: Option<String>,
    /// Transfer encoding.
    pub encoding: TransferEncoding,
    /// Size in bytes of the encoded content.
    pub size: u32,
    /// Lines, for `text/*` and `message/rfc822`.
    pub lines: Option<u32>,
    /// The structure of an attached `message/rfc822`.
    pub message: Option<Box<BodyStructure>>,
}

/// A multipart part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Multipart {
    /// Type (`multipart`), subtype (`mixed`, `alternative`…), parameters…
    pub common: PartCommon,
    /// The children.
    pub parts: Vec<BodyStructure>,
}

/// The MIME structure of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyStructure {
    /// A leaf part.
    Single(Box<SinglePart>),
    /// A multipart with children.
    Multipart(Multipart),
}

impl BodyStructure {
    /// Whether any part is an attachment: `Content-Disposition: attachment`,
    /// or a non-text, non-multipart part that is not inline.
    #[must_use]
    pub fn has_attachment(&self) -> bool {
        match self {
            Self::Multipart(multipart) => multipart.parts.iter().any(Self::has_attachment),
            Self::Single(part) => {
                let disposition = part.common.disposition.as_ref().map(|d| d.kind.as_str());
                match disposition {
                    Some("attachment") => true,
                    Some("inline") => false,
                    _ => part.common.media_type != "text" && part.common.media_type != "multipart",
                }
            }
        }
    }
}

/// Converts the tree of `imap-proto`. Its depth is bounded by the nesting
/// limit checked before parsing.
pub(crate) fn convert_body_structure(body: imap_proto::BodyStructure<'_>) -> BodyStructure {
    use imap_proto::BodyStructure as Proto;
    match body {
        Proto::Basic { common, other, .. } => single(common, other, None, None),
        Proto::Text {
            common,
            other,
            lines,
            ..
        } => single(common, other, Some(lines), None),
        Proto::Message {
            common,
            other,
            body,
            lines,
            ..
        } => {
            let inner = convert_body_structure(*body);
            single(common, other, Some(lines), Some(Box::new(inner)))
        }
        Proto::Multipart { common, bodies, .. } => BodyStructure::Multipart(Multipart {
            common: convert_common(common),
            parts: bodies.into_iter().map(convert_body_structure).collect(),
        }),
    }
}

fn single(
    common: BodyContentCommon<'_>,
    other: BodyContentSinglePart<'_>,
    lines: Option<u32>,
    message: Option<Box<BodyStructure>>,
) -> BodyStructure {
    BodyStructure::Single(Box::new(SinglePart {
        common: convert_common(common),
        id: other.id.map(Cow::into_owned),
        description: other.description.map(Cow::into_owned),
        md5: other.md5.map(Cow::into_owned),
        encoding: convert_encoding(other.transfer_encoding),
        size: other.octets,
        lines,
        message,
    }))
}

fn convert_common(common: BodyContentCommon<'_>) -> PartCommon {
    PartCommon {
        media_type: common.ty.ty.to_ascii_lowercase(),
        subtype: common.ty.subtype.to_ascii_lowercase(),
        params: convert_params(common.ty.params),
        disposition: common.disposition.map(|disposition| Disposition {
            kind: disposition.ty.to_ascii_lowercase(),
            params: convert_params(disposition.params),
        }),
        language: common
            .language
            .unwrap_or_default()
            .into_iter()
            .map(Cow::into_owned)
            .collect(),
        location: common.location.map(Cow::into_owned),
    }
}

fn convert_params(params: BodyParams<'_>) -> Vec<(String, String)> {
    params
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
        .collect()
}

fn convert_encoding(encoding: ContentEncoding<'_>) -> TransferEncoding {
    match encoding {
        ContentEncoding::SevenBit => TransferEncoding::SevenBit,
        ContentEncoding::EightBit => TransferEncoding::EightBit,
        ContentEncoding::Binary => TransferEncoding::Binary,
        ContentEncoding::Base64 => TransferEncoding::Base64,
        ContentEncoding::QuotedPrintable => TransferEncoding::QuotedPrintable,
        ContentEncoding::Other(other) => TransferEncoding::Other(other.to_ascii_lowercase()),
    }
}
