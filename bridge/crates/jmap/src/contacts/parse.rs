// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `ContactCard/parse` (RFC 9610 §3.6): the first vCard of each blob
//! (uploads, messages, parts) as JSContact, as Stalwart does. Only calcard
//! and the blob; no DAV server.

use std::collections::{BTreeMap, HashSet};

use nebula_groupware::model::convert::MAX_VCARD_BYTES;
use nebula_jmap_proto::object::js_body::{JsBody, JsContact};
use nebula_jmap_proto::object::{ContactCard, ContactCardParseArguments, ContactCardProperty};
use nebula_jmap_proto::{BlobId, MethodResponse};

use super::convert::card_body;
use crate::core::context::Ctx;
use crate::email::build::blob_bytes;
use crate::error::{Bug, CallError};

pub(crate) async fn parse(
    ctx: &Ctx<'_>,
    args: ContactCardParseArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let properties = args.properties.as_deref();
    let mut not_found = Vec::new();
    let mut parsed = BTreeMap::new();
    let mut not_parsable = Vec::new();
    // One blob in memory at a time: read, convert off the async threads,
    // keep only the JSON.
    for blob in wanted_blobs(args.blob_ids) {
        // Held while this blob is: its memory is counted.
        let Ok(mut lease) = crate::email::build::lease(ctx) else {
            not_found.push(blob);
            continue;
        };
        let Ok(Some(bytes)) = blob_bytes(ctx, &blob, &mut lease).await else {
            not_found.push(blob);
            continue;
        };
        let body = tokio::task::spawn_blocking(move || parse_one(&bytes))
            .await
            .map_err(|_| Bug("card conversion"))?;
        let Some(body) = body else {
            not_parsable.push(blob);
            continue;
        };
        let body = match properties {
            None => body,
            Some(list) => body.select(list.iter().filter_map(|p| match p {
                ContactCardProperty::Body(name) => Some(name.as_str()),
                _ => None,
            })),
        };
        parsed.insert(
            blob,
            ContactCard {
                body,
                ..ContactCard::default()
            },
        );
    }
    Ok(MethodResponse::ContactCardParse(
        nebula_jmap_proto::object::ContactCardParseResponse {
            account_id: ctx.jmap_id(),
            parsed,
            not_parsable,
            not_found,
        },
    ))
}

/// The blob ids to read: each once, in the order asked.
fn wanted_blobs(ids: Vec<BlobId>) -> Vec<BlobId> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|blob| seen.insert(blob.clone()))
        .collect()
}

/// The card of one blob, or `None` if it isn't one (also when it is
/// larger than a vCard may be: refused before calcard sees it).
fn parse_one(bytes: &[u8]) -> Option<JsBody<JsContact>> {
    if bytes.len() > MAX_VCARD_BYTES {
        return None;
    }
    card_body(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_are_read_once_in_order() {
        let id = |s: &str| s.parse::<BlobId>().unwrap();
        assert_eq!(
            wanted_blobs(vec![id("b2"), id("b1"), id("b2"), id("b3"), id("b1")]),
            vec![id("b2"), id("b1"), id("b3")]
        );
    }

    #[test]
    fn too_large_blobs_are_not_parsable() {
        let card = b"BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ana\r\nUID:u1\r\nEND:VCARD\r\n";
        assert!(parse_one(card).is_some());
        let mut big = b"BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ana\r\nNOTE:".to_vec();
        big.resize(MAX_VCARD_BYTES + 1 - b"\r\nEND:VCARD\r\n".len(), b'a');
        big.extend_from_slice(b"\r\nEND:VCARD\r\n");
        assert_eq!(big.len(), MAX_VCARD_BYTES + 1);
        assert!(parse_one(&big).is_none());
    }
}
