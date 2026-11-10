// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Body parts and body values (RFC 8621 §4.1.4), as returned and as sent
//! to create an Email.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::address::{EmailHeader, is_single_line};
use super::header::{HeaderProperties, HeaderProperty, HeaderValue};
use crate::macros::{serde_via_str, string_enum};
use crate::types::{BlobId, UnsignedInt};

/// Deepest `subParts` nesting accepted when creating an Email.
pub const MAX_BODY_DEPTH: usize = 10;

/// A `partId`: names a body part within one Email. Also the key of
/// `bodyValues`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartId(String);

/// Why a string is not a valid [`PartId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("partId must be 1 to 255 bytes without control characters")]
pub struct PartIdError;

impl PartId {
    /// The part id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PartId {
    type Err = PartIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || s.len() > 255 || s.chars().any(char::is_control) {
            return Err(PartIdError);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for PartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for PartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PartId").field(&self.0).finish()
    }
}

serde_via_str!(PartId);

/// The decoded text of a body part (`bodyValues`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailBodyValue {
    /// The text.
    pub value: String,
    /// The charset was unknown or the bytes were invalid for it.
    #[serde(default)]
    pub is_encoding_problem: bool,
    /// The value was cut at `maxBodyValueBytes`.
    #[serde(default)]
    pub is_truncated: bool,
}

string_enum! {
    /// `EmailBodyPart` properties without the `header:` ones.
    pub enum BodyPartField ("body part property") {
        /// `partId`.
        PartId = "partId",
        /// `blobId`.
        BlobId = "blobId",
        /// `size`.
        Size = "size",
        /// `headers`.
        Headers = "headers",
        /// `name`.
        Name = "name",
        /// `type`.
        Type = "type",
        /// `charset`.
        Charset = "charset",
        /// `disposition`.
        Disposition = "disposition",
        /// `cid`.
        Cid = "cid",
        /// `language`.
        Language = "language",
        /// `location`.
        Location = "location",
        /// `subParts`.
        SubParts = "subParts",
    }
}

/// A property of `bodyProperties` (`Email/get`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum EmailBodyPartProperty {
    /// A fixed property.
    Field(BodyPartField),
    /// A `header:…` property.
    Header(HeaderProperty),
}

impl EmailBodyPartProperty {
    /// `bodyProperties` when the client doesn't send it (RFC 8621 §4.2).
    pub const DEFAULT: &'static [BodyPartField] = &[
        BodyPartField::PartId,
        BodyPartField::BlobId,
        BodyPartField::Size,
        BodyPartField::Name,
        BodyPartField::Type,
        BodyPartField::Charset,
        BodyPartField::Disposition,
        BodyPartField::Cid,
        BodyPartField::Language,
        BodyPartField::Location,
    ];
}

impl FromStr for EmailBodyPartProperty {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("header:") {
            return s.parse().map(Self::Header).map_err(|e| format!("{e}"));
        }
        s.parse().map(Self::Field).map_err(|e| format!("{e}"))
    }
}

impl fmt::Display for EmailBodyPartProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(field) => f.write_str(field.as_str()),
            Self::Header(header) => f.write_str(header.as_str()),
        }
    }
}

serde_via_str!(EmailBodyPartProperty);

/// A body part as returned by `Email/get`. Only the `bodyProperties` asked
/// for are set (`None` = not sent; `Some(None)` = `null`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyPart {
    /// `partId` (`null` for multipart parts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_id: Option<Option<PartId>>,
    /// `blobId` (`null` for multipart parts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Option<BlobId>>,
    /// `size` of the decoded content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<UnsignedInt>,
    /// `headers` in Raw form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// `name` (file name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Option<String>>,
    /// `type` (media type without parameters).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// `charset`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<Option<String>>,
    /// `disposition`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<Option<String>>,
    /// `cid` (Content-ID without `<>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<Option<String>>,
    /// `language`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Option<Vec<String>>>,
    /// `location`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Option<String>>,
    /// `subParts` (`null` unless multipart).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_parts: Option<Option<Vec<EmailBodyPart>>>,
    /// Requested `header:…` properties.
    #[serde(flatten)]
    pub header_properties: HeaderProperties,
}

/// What a body part to create contains.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyPartContent {
    /// Text from `bodyValues` (`partId`).
    Value(PartId),
    /// An uploaded blob (`blobId`).
    Blob(BlobId),
    /// A `multipart/*` part with children (`subParts`).
    Multipart(Vec<EmailBodyPartCreate>),
}

/// A body part sent to create an Email (RFC 8621 §4.6 rules applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailBodyPartCreate {
    /// The content.
    pub content: BodyPartContent,
    /// Media type, lowercased (`text/plain` if not given).
    pub media_type: String,
    /// Parameters given with `type` (`text/calendar; method=REPLY`, as
    /// Stalwart accepts and the web sends for iMIP, RFC 6047 §2.4): names
    /// lowercased, values unquoted. Never `charset`, `name` or `boundary`
    /// (their own properties, or the server's).
    pub media_type_params: Vec<(String, String)>,
    /// `charset` (only for blob parts).
    pub charset: Option<String>,
    /// `disposition` (`inline`, `attachment`…).
    pub disposition: Option<String>,
    /// `cid`.
    pub cid: Option<String>,
    /// File name.
    pub name: Option<String>,
    /// `language` tags.
    pub language: Option<Vec<String>>,
    /// `location`.
    pub location: Option<String>,
    /// Other header fields for this part.
    pub header_properties: HeaderProperties,
}

/// Most parameters accepted in `type`.
const MAX_TYPE_PARAMS: usize = 8;
/// Longest parameter value accepted in `type`.
const MAX_TYPE_PARAM_VALUE: usize = 256;

/// A MIME token (RFC 2045 §5.1): no spaces, controls or `tspecials`.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?=".contains(&b))
}

/// A quoted parameter value's inside: printable ASCII and spaces, without
/// `"` or `\` (so nothing needs escaping).
fn is_plain_quoted(text: &str) -> bool {
    text.bytes()
        .all(|b| (b == b' ' || b.is_ascii_graphic()) && b != b'"' && b != b'\\')
}

/// `type/subtype` and its parameters: `text/calendar; method=REPLY` or
/// `…; method="REPLY"`. Strict: a value is a token or a plain quoted
/// string, and `;` never appears inside one.
fn parse_media_type(text: &str) -> Result<(String, Vec<(String, String)>), String> {
    let bad = || "type must be a media type like text/plain".to_owned();
    let mut pieces = text.split(';');
    let base = pieces.next().unwrap_or_default().trim();
    if !is_media_type(base) {
        return Err(bad());
    }
    let mut params: Vec<(String, String)> = Vec::new();
    for piece in pieces {
        let (name, value) = piece.split_once('=').ok_or_else(bad)?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        let value = match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            Some(quoted) if is_plain_quoted(quoted) => quoted,
            None if is_token(value) => value,
            _ => return Err(bad()),
        };
        if !is_token(&name) || value.len() > MAX_TYPE_PARAM_VALUE {
            return Err(bad());
        }
        if matches!(name.as_str(), "charset" | "name" | "boundary") {
            return Err(format!("type: give {name} on its own, not as a parameter"));
        }
        if params.iter().any(|(seen, _)| *seen == name) {
            return Err(format!("type: parameter {name} is repeated"));
        }
        params.push((name, value.to_owned()));
    }
    if params.len() > MAX_TYPE_PARAMS {
        return Err("type has too many parameters".to_owned());
    }
    Ok((base.to_ascii_lowercase(), params))
}

/// A `type/subtype` token pair: no spaces, parameters or control
/// characters.
fn is_media_type(text: &str) -> bool {
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    matches!(text.split_once('/'), Some((kind, sub)) if token(kind) && token(sub))
}

/// Header fields the fixed properties already write: a `header:` property
/// for them would be a second copy (RFC 8621 §4.6).
fn fields_written_by(part: &PartFields) -> BTreeSet<&'static str> {
    // The builder always writes a Content-Type, `text/plain` by default.
    let mut fields = BTreeSet::from(["content-type"]);
    if part.disposition.is_some() || part.name.is_some() {
        fields.insert("content-disposition");
    }
    if part.cid.is_some() {
        fields.insert("content-id");
    }
    if part.language.is_some() {
        fields.insert("content-language");
    }
    if part.location.is_some() {
        fields.insert("content-location");
    }
    fields
}

#[derive(Default)]
struct PartFields {
    part_id: Option<PartId>,
    blob_id: Option<BlobId>,
    size: bool,
    media_type: Option<String>,
    media_type_params: Vec<(String, String)>,
    charset: Option<String>,
    disposition: Option<String>,
    cid: Option<String>,
    name: Option<String>,
    language: Option<Vec<String>>,
    location: Option<String>,
    sub_parts: Option<Vec<Value>>,
    headers: HeaderProperties,
}

fn field<T: serde::de::DeserializeOwned>(name: &str, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("{name}: {error}"))
}

/// A single-line string property (`null` = not given).
fn line(name: &str, value: Value) -> Result<Option<String>, String> {
    let text: Option<String> = field(name, value)?;
    match text {
        Some(text) if !is_single_line(&text) => Err(format!("{name} has a line break")),
        other => Ok(other),
    }
}

impl EmailBodyPartCreate {
    /// Parses a body part to create. `depth` is the nesting level.
    ///
    /// # Errors
    /// A description of the first problem found.
    pub(crate) fn parse(object: Value, depth: usize) -> Result<Self, String> {
        if depth >= MAX_BODY_DEPTH {
            return Err("body parts are nested too deeply".to_owned());
        }
        let Value::Object(map) = object else {
            return Err("a body part must be an object".to_owned());
        };
        let fields = read_fields(map)?;
        let written = fields_written_by(&fields);
        let mut seen = BTreeSet::new();
        for (property, _) in fields.headers.iter() {
            let lower = property.name().to_ascii_lowercase();
            if lower == "content-transfer-encoding" {
                return Err("Content-Transfer-Encoding must not be given".to_owned());
            }
            if written.contains(lower.as_str()) || !seen.insert(lower) {
                return Err(format!("{property} duplicates another property"));
            }
        }
        Self::build(fields, depth)
    }

    fn build(fields: PartFields, depth: usize) -> Result<Self, String> {
        let media_type = fields
            .media_type
            .map_or_else(|| "text/plain".to_owned(), |t| t.to_ascii_lowercase());
        if media_type.starts_with("multipart/") && !fields.media_type_params.is_empty() {
            return Err("a multipart/* type takes no parameters".to_owned());
        }
        let content = match (fields.part_id, fields.blob_id, fields.sub_parts) {
            (Some(part_id), None, None) => {
                if fields.charset.is_some() || fields.size {
                    return Err("charset and size must be omitted with partId".to_owned());
                }
                BodyPartContent::Value(part_id)
            }
            (None, Some(blob_id), None) => BodyPartContent::Blob(blob_id),
            (None, None, Some(parts)) => {
                if !media_type.starts_with("multipart/") {
                    return Err("subParts needs a multipart/* type".to_owned());
                }
                let depth = depth.saturating_add(1);
                let children = parts
                    .into_iter()
                    .map(|part| Self::parse(part, depth))
                    .collect::<Result<Vec<_>, _>>()?;
                BodyPartContent::Multipart(children)
            }
            (None, None, None) => {
                return Err("a body part needs partId, blobId or subParts".to_owned());
            }
            _ => return Err("give only one of partId, blobId and subParts".to_owned()),
        };
        if media_type.starts_with("multipart/") && !matches!(content, BodyPartContent::Multipart(_))
        {
            return Err("a multipart/* part needs subParts".to_owned());
        }
        Ok(Self {
            content,
            media_type,
            media_type_params: fields.media_type_params,
            charset: fields.charset,
            disposition: fields.disposition,
            cid: fields.cid,
            name: fields.name,
            language: fields.language,
            location: fields.location,
            header_properties: fields.headers,
        })
    }
}

fn read_fields(map: Map<String, Value>) -> Result<PartFields, String> {
    let mut fields = PartFields::default();
    for (key, value) in map {
        match key.as_str() {
            "partId" => fields.part_id = field(&key, value)?,
            "blobId" => fields.blob_id = field(&key, value)?,
            "size" => {
                // Ignored for blobs (the server measures them), but it must
                // still be a number.
                let size: Option<UnsignedInt> = field(&key, value)?;
                fields.size = size.is_some();
            }
            "type" => {
                if let Some(text) = line(&key, value)? {
                    let (media_type, params) = parse_media_type(&text)?;
                    fields.media_type = Some(media_type);
                    fields.media_type_params = params;
                }
            }
            "charset" => fields.charset = line(&key, value)?,
            "disposition" => fields.disposition = line(&key, value)?,
            "cid" => fields.cid = line(&key, value)?,
            "name" => fields.name = line(&key, value)?,
            "location" => fields.location = line(&key, value)?,
            "language" => {
                let language: Option<Vec<String>> = field(&key, value)?;
                if language.iter().flatten().any(|tag| !is_single_line(tag)) {
                    return Err("language has a line break".to_owned());
                }
                fields.language = language;
            }
            "subParts" => fields.sub_parts = field(&key, value)?,
            "headers" => {
                return Err("headers must not be given; use header:… properties".to_owned());
            }
            other if other.starts_with("header:") => {
                let property: HeaderProperty =
                    other.parse().map_err(|e| format!("{other}: {e}"))?;
                let value = HeaderValue::from_json(&property, value)?;
                fields.headers.insert(property, value);
            }
            other => return Err(format!("unknown body part property {other:?}")),
        }
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: Value) -> Result<EmailBodyPartCreate, String> {
        EmailBodyPartCreate::parse(value, 0)
    }

    #[test]
    fn the_web_clients_structure_parses() {
        let part = parse(json!({
            "type": "multipart/mixed",
            "subParts": [
                {
                    "type": "multipart/related",
                    "subParts": [
                        {"type": "multipart/alternative", "subParts": [
                            {"partId": "text", "type": "text/plain"},
                            {"partId": "html", "type": "text/html"}
                        ]},
                        {"blobId": "b1", "cid": "img1", "disposition": "inline",
                         "name": "a.png", "type": "image/png"}
                    ]
                },
                {"blobId": "b2", "disposition": "attachment", "name": "x.pdf",
                 "type": "application/pdf"}
            ]
        }))
        .unwrap();
        let BodyPartContent::Multipart(children) = part.content else {
            panic!("expected multipart")
        };
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn rfc_rules() {
        for bad in [
            json!({"partId": "a", "blobId": "b"}),
            json!({"partId": "a", "charset": "utf-8"}),
            json!({"partId": "a", "size": 3}),
            json!({"type": "text/plain"}),
            json!({"type": "multipart/mixed", "partId": "a"}),
            json!({"type": "text/plain", "subParts": []}),
            json!({"partId": "a", "headers": []}),
            json!({"partId": "a", "header:Content-Transfer-Encoding": " base64"}),
            json!({"partId": "a", "type": "text/html", "header:Content-Type": " text/html"}),
            // The builder writes a Content-Type even without `type`.
            json!({"partId": "a", "header:Content-Type": " text/html"}),
            json!({"partId": "a", "header:content-type:asRaw": " text/html"}),
            json!({"partId": "a", "header:X-A": " 1", "header:x-a:asText": "2"}),
            json!({"partId": "a", "type": "text/html\r\nBcc: x"}),
            json!({"partId": "a", "type": "text html"}),
            json!({"blobId": "b", "name": "a\nb"}),
            json!({"partId": "a", "unknown": 1}),
            json!("text"),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        let blob = parse(json!({"blobId": "b", "size": 10, "type": "Image/PNG"})).unwrap();
        assert_eq!(blob.media_type, "image/png");
        let custom = parse(json!({"partId": "a", "header:X-Note:asText": "hi"})).unwrap();
        assert_eq!(custom.media_type, "text/plain");
        assert!(!custom.header_properties.is_empty());
    }

    /// iMIP parts (RFC 6047 §2.4): the web sends the method in `type`, as
    /// Stalwart accepts.
    #[test]
    fn type_parameters() {
        let part = parse(json!({"partId": "ics", "type": "text/calendar; method=REPLY"})).unwrap();
        assert_eq!(part.media_type, "text/calendar");
        assert_eq!(
            part.media_type_params,
            [("method".to_owned(), "REPLY".to_owned())]
        );
        let quoted = parse(
            json!({"partId": "a", "type": "Text/Calendar;Method=\"REQUEST\"; component=VEVENT"}),
        )
        .unwrap();
        assert_eq!(quoted.media_type, "text/calendar");
        assert_eq!(
            quoted.media_type_params,
            [
                ("method".to_owned(), "REQUEST".to_owned()),
                ("component".to_owned(), "VEVENT".to_owned())
            ]
        );
        for bad in [
            "text/calendar;",
            "text/calendar; method",
            "text/calendar; method=",
            "text/calendar; method=a b",
            "text/calendar; method=\"a\\\"b\"",
            "text/calendar; method=\"a;b\"",
            "text/calendar; method=REPLY; method=REQUEST",
            "text/plain; charset=utf-8",
            "text/plain; name=x.txt",
            "text/plain; boundary=x",
            "text/plain; =x",
            "text/plain; a@b=x",
            "text/calendar; method=REPLY\r\nBcc: x",
        ] {
            assert!(parse(json!({"partId": "a", "type": bad})).is_err(), "{bad}");
        }
        let many = (0..=MAX_TYPE_PARAMS)
            .map(|i| format!("; p{i}=x"))
            .collect::<Vec<_>>()
            .concat();
        assert!(parse(json!({"partId": "a", "type": format!("text/plain{many}")})).is_err());
        let long = "x".repeat(MAX_TYPE_PARAM_VALUE + 1);
        assert!(parse(json!({"partId": "a", "type": format!("text/plain; p={long}")})).is_err());
        // Never on a multipart (the boundary is the server's).
        assert!(
            parse(json!({"type": "multipart/mixed; x=y", "subParts": [{"partId": "a"}]})).is_err()
        );
    }

    #[test]
    fn depth_limit() {
        let mut part = json!({"partId": "a"});
        for _ in 0..MAX_BODY_DEPTH {
            part = json!({"type": "multipart/mixed", "subParts": [part]});
        }
        assert!(parse(part).is_err());
    }

    #[test]
    fn response_shape() {
        let part = EmailBodyPart {
            part_id: Some(Some("1".parse().unwrap())),
            blob_id: Some(Some("B841623871".parse().unwrap())),
            size: Some(UnsignedInt::from(283_331_u32)),
            media_type: Some("text/html".to_owned()),
            ..EmailBodyPart::default()
        };
        assert_eq!(
            serde_json::to_value(&part).unwrap(),
            json!({"partId": "1", "blobId": "B841623871", "size": 283_331, "type": "text/html"})
        );
        let multipart = EmailBodyPart {
            part_id: Some(None),
            sub_parts: Some(Some(vec![])),
            ..EmailBodyPart::default()
        };
        assert_eq!(
            serde_json::to_value(&multipart).unwrap(),
            json!({"partId": null, "subParts": []})
        );
    }

    #[test]
    fn body_part_properties() {
        let property: EmailBodyPartProperty = "header:Content-Type:asRaw".parse().unwrap();
        assert!(matches!(property, EmailBodyPartProperty::Header(_)));
        assert_eq!(
            "type".parse::<EmailBodyPartProperty>().unwrap(),
            EmailBodyPartProperty::Field(BodyPartField::Type)
        );
        assert!("colour".parse::<EmailBodyPartProperty>().is_err());
    }
}
