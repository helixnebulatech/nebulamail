// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Blobs (RFC 8620 §6) and their ids.
//!
//! - `bu{n}`: an upload (`uploads` row `n`).
//! - `bm{e}`: the whole raw message of email `e` (its `blobId`).
//! - `bp{e}_{p}`: body part `p` of email `e`, where `p` is the part's
//!   `partId` (its index in the parsed message). The part is found again by
//!   parsing the message, so nothing is stored for it.
//! - `bs{e}_{p}_{size}_{1-2}`: the same part `p`, which can also be
//!   streamed from the IMAP section `1.2` and is `size` bytes once decoded.
//!   Given out only for big messages, after checking that the server's
//!   structure agrees with ours (`nebula_mail::streamable_parts`). A
//!   forged one only reaches the account's own emails, and a wrong size or
//!   section never gives other bytes: the download is checked against the
//!   server's structure and cut short if the size doesn't match.
//!
//! - `bn{p}_{blob}`: part `p` of the message that blob `{blob}` (any of
//!   these ids) holds, for the parts `Email/parse` shows (RFC 8621 §4.9):
//!   an uploaded message, or a message attached to another. At most
//!   [`MAX_NESTING`] levels.
//!
//! Emails never change and their ids are never reused, so a message or
//! part blob id always means the same bytes. Parsing is strict (the exact
//! form we write, no leading zeros), so one text is one blob id; but the
//! `bs…` ids of one part (any size or section) and its `bp…` id all name
//! the same bytes: [`BlobRef::content_key`] says which bytes a blob id
//! means, so they are loaded (and counted) once.

pub(crate) mod copy;
pub(crate) mod download;
pub(crate) mod upload;

use std::num::NonZeroU32;

use nebula_jmap_proto::BlobId;
use nebula_mail::PartSection;
use nebula_store::{EmailId, UploadId};

use crate::core::ids::parse_number;
use crate::error::Bug;

/// What a blob id points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlobRef {
    Upload(UploadId),
    Message(EmailId),
    Part(EmailId, u32),
    Section(EmailId, u32, PartSection),
    /// `bz{n}_{hash}`: Sieve script `n` with this content.
    Sieve(nebula_store::SieveScriptId, String),
    /// `bn{p}_{blob}`: part `p` of the message in another blob.
    Nested(Box<BlobRef>, u32),
}

/// The bytes a blob id means: `bp…` and every `bs…` of the same part
/// share one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ContentKey {
    Upload(UploadId),
    Message(EmailId),
    Part(EmailId, u32),
    Sieve(nebula_store::SieveScriptId, String),
    Nested(Box<ContentKey>, u32),
}

/// Deepest IMAP part path in a blob id.
const MAX_PATH: usize = 32;

/// Most `bn…` levels in one blob id (a message attached to a message
/// attached to… a message).
pub(crate) const MAX_NESTING: usize = 8;

/// A decimal without sign or leading zeros (`0` itself allowed).
fn parse_decimal(text: &str) -> Option<u64> {
    if text == "0" {
        return Some(0);
    }
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_section(email: EmailId, rest: &str) -> Option<BlobRef> {
    let mut fields = rest.splitn(3, '_');
    let part = u32::try_from(parse_decimal(fields.next()?)?).ok()?;
    let size = parse_decimal(fields.next()?)?;
    let path_text = fields.next()?;
    let path = path_text
        .split('-')
        .map(|n| {
            parse_decimal(n)
                .and_then(|n| u32::try_from(n).ok())
                .and_then(NonZeroU32::new)
        })
        .collect::<Option<Vec<_>>>()?;
    if path.len() > MAX_PATH {
        return None;
    }
    Some(BlobRef::Section(email, part, PartSection::new(path, size)?))
}

impl BlobRef {
    /// Which bytes this blob means ([`ContentKey`]).
    pub(crate) fn content_key(&self) -> ContentKey {
        match self {
            Self::Upload(id) => ContentKey::Upload(*id),
            Self::Message(id) => ContentKey::Message(*id),
            Self::Part(email, part) | Self::Section(email, part, _) => {
                ContentKey::Part(*email, *part)
            }
            Self::Sieve(id, hash) => ContentKey::Sieve(*id, hash.clone()),
            Self::Nested(blob, part) => ContentKey::Nested(Box::new(blob.content_key()), *part),
        }
    }

    /// Part `part` of the message this blob holds.
    pub(crate) fn nested(self, part: u32) -> Self {
        Self::Nested(Box::new(self), part)
    }

    pub(crate) fn to_blob_id(&self) -> Result<BlobId, Bug> {
        let text = match self {
            Self::Upload(id) => format!("bu{}", id.get()),
            Self::Message(id) => format!("bm{}", id.get()),
            Self::Part(email, part) => format!("bp{}_{part}", email.get()),
            Self::Sieve(id, hash) => format!("bz{}_{hash}", id.get()),
            Self::Nested(blob, part) => format!("bn{part}_{}", blob.to_blob_id()?.as_str()),
            Self::Section(email, part, section) => {
                let path: Vec<String> = section.path().iter().map(ToString::to_string).collect();
                format!(
                    "bs{}_{part}_{}_{}",
                    email.get(),
                    section.size(),
                    path.join("-")
                )
            }
        };
        text.parse().map_err(|_| Bug("blob id"))
    }

    pub(crate) fn parse(id: &BlobId) -> Option<Self> {
        Self::parse_text(id.as_str(), 0)
    }

    fn parse_text(text: &str, depth: usize) -> Option<Self> {
        let text = text.strip_prefix('b')?;
        if let Some(rest) = text.strip_prefix('n') {
            if depth >= MAX_NESTING {
                return None;
            }
            let (part, inner) = rest.split_once('_')?;
            let part = u32::try_from(parse_decimal(part)?).ok()?;
            return Self::parse_text(inner, depth.saturating_add(1)).map(|blob| blob.nested(part));
        }
        if let Some(rest) = text.strip_prefix('u') {
            return parse_number(&format!("u{rest}"), 'u')
                .and_then(UploadId::from_raw)
                .map(Self::Upload);
        }
        if let Some(rest) = text.strip_prefix('m') {
            return parse_number(&format!("m{rest}"), 'm')
                .and_then(EmailId::from_raw)
                .map(Self::Message);
        }
        if text.starts_with('z') {
            return crate::sieve::parse_blob(&format!("b{text}"))
                .map(|(id, hash)| Self::Sieve(id, hash));
        }
        if let Some(rest) = text.strip_prefix('s') {
            let (email, rest) = rest.split_once('_')?;
            let email = parse_number(&format!("e{email}"), 'e').and_then(EmailId::from_raw)?;
            return parse_section(email, rest);
        }
        let rest = text.strip_prefix('p')?;
        let (email, part) = rest.split_once('_')?;
        let email = parse_number(&format!("e{email}"), 'e').and_then(EmailId::from_raw)?;
        let part = if part == "0" {
            0
        } else {
            u32::try_from(parse_number(&format!("p{part}"), 'p')?).ok()?
        };
        Some(Self::Part(email, part))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value, reason = "reads better in the calls")]
    fn round_trip(blob: BlobRef) {
        let id = blob.to_blob_id().unwrap();
        assert_eq!(BlobRef::parse(&id), Some(blob.clone()), "{id}");
    }

    fn section(path: &[u32], size: u64) -> PartSection {
        PartSection::new(
            path.iter().map(|n| NonZeroU32::new(*n).unwrap()).collect(),
            size,
        )
        .unwrap()
    }

    #[test]
    fn ids_round_trip() {
        round_trip(BlobRef::Upload(UploadId::from_raw(3).unwrap()));
        round_trip(BlobRef::Message(EmailId::from_raw(12).unwrap()));
        round_trip(BlobRef::Part(EmailId::from_raw(12).unwrap(), 0));
        round_trip(BlobRef::Part(EmailId::from_raw(12).unwrap(), 7));
        let email = EmailId::from_raw(12).unwrap();
        round_trip(BlobRef::Section(email, 0, section(&[1], 5)));
        round_trip(BlobRef::Section(email, 4, section(&[2, 10, 1], 1_000_000)));
        assert_eq!(
            BlobRef::Section(email, 4, section(&[2, 10], 77))
                .to_blob_id()
                .unwrap()
                .as_str(),
            "bs12_4_77_2-10"
        );
        assert_eq!(
            BlobRef::Part(EmailId::from_raw(5).unwrap(), 2)
                .to_blob_id()
                .unwrap()
                .as_str(),
            "bp5_2"
        );
    }

    /// `Email/parse` parts (RFC 8621 §4.9): `bn{part}_{blob}`.
    #[test]
    fn nested_ids() {
        let email = EmailId::from_raw(12).unwrap();
        let upload = BlobRef::Upload(UploadId::from_raw(3).unwrap());
        round_trip(upload.clone().nested(2));
        round_trip(BlobRef::Part(email, 4).nested(0).nested(7));
        round_trip(BlobRef::Section(email, 4, section(&[2], 9)).nested(1));
        assert_eq!(
            upload.clone().nested(2).to_blob_id().unwrap().as_str(),
            "bn2_bu3"
        );
        let id = |text: &str| text.parse::<BlobId>().unwrap();
        for bad in ["bn_bu3", "bn02_bu3", "bn2", "bn2_", "bn2_x", "bnx_bu3"] {
            assert_eq!(BlobRef::parse(&id(bad)), None, "{bad}");
        }
        let deep = |levels: usize| {
            let mut blob = upload.clone();
            for _ in 0..levels {
                blob = blob.nested(1);
            }
            blob.to_blob_id().unwrap()
        };
        assert!(BlobRef::parse(&deep(MAX_NESTING)).is_some());
        assert_eq!(BlobRef::parse(&deep(MAX_NESTING + 1)), None);
        assert_ne!(
            upload.clone().nested(1).content_key(),
            upload.nested(2).content_key()
        );
    }

    #[test]
    fn aliases_share_a_content_key() {
        let key = |text: &str| BlobRef::parse(&text.parse().unwrap()).map(|b| b.content_key());
        let email = EmailId::from_raw(1).unwrap();
        assert_eq!(key("bs1_1_1_1"), Some(ContentKey::Part(email, 1)));
        assert_eq!(key("bs1_1_1_1"), key("bs1_1_2_1"));
        assert_eq!(key("bs1_1_1_1"), key("bs1_1_9_2-3"));
        assert_eq!(key("bs1_1_1_1"), key("bp1_1"));
        assert_ne!(key("bp1_1"), key("bp1_2"));
        assert_ne!(key("bp1_0"), key("bm1"));
    }

    #[test]
    fn strict_parsing() {
        for bad in [
            "b",
            "bu",
            "bu0",
            "bu01",
            "bm-1",
            "bp1",
            "bp1_",
            "bp1_01",
            "bp0_1",
            "bx1",
            "u1",
            "bp1_2_3",
            "bp1_99999999999",
            "bs1_2_3",
            "bs1_2_0_1",
            "bs1_2_3_0",
            "bs1_2_3_01",
            "bs1_02_3_1",
            "bs1_2_03_1",
            "bs1_2_3_1-",
            "bs1_2_3_-1",
            "bs1_2_3_1--2",
            "bs0_2_3_1",
            "bs1_2_3_1_4",
            "bs1_2_3_1-2-3-4-5-6-7-8-9-10-11-12-13-14-15-16-17-18-19-20-21-22-23-24-25-26-27-28-29-30-31-32-33",
        ] {
            assert_eq!(BlobRef::parse(&bad.parse().unwrap()), None, "{bad}");
        }
    }
}
