// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! vCard ↔ `ContactCard` without I/O: the JSContact body of a stored
//! vCard, the vCard of a new card, and a JMAP patch applied to a stored
//! vCard. CPU work (calcard): call these off the async threads.
//!
//! - A patch starts from the stored text, never from scratch: calcard
//!   keeps what it does not convert under `vCard` (Apple's `X-ABUID`,
//!   `item1.X-ABLabel`, `PRODID`…), so it is written back untouched.
//! - The version stays the stored card's (3.0 from Apple stays 3.0); a
//!   new card gets 4.0 if the book lists it, else 3.0 (RFC 6352 §6.2.2).
//!   A 3.0 card is written as RFC 2426 says: a group the Apple way
//!   (`X-ADDRESSBOOKSERVER-KIND`/`-MEMBER`, read as `kind` and
//!   `members`), never 4.0's `KIND`/`MEMBER`.
//! - Photos are `data:` URIs or links (never downloaded),
//!   at most [`MAX_PHOTO_BYTES`] each; `blobId` in `media` is refused, as
//!   Stalwart does.

use std::fmt::Write as _;

use nebula_dav::proto::ResourceName;
use nebula_groupware::ConvertError;
use nebula_groupware::model::convert::{
    VcardForm, VcardVersion, jscontact_to_vcard_in, vcard_form, vcard_to_jscontact,
};
use nebula_jmap_proto::object::js_body::{BodyPatch, JsBody, JsContact};
use nebula_jmap_proto::{SetError, SetErrorType, UtcDate};
use nebula_store::VcardVersions;
use serde_json::{Map, Value};

/// Largest `data:` URI of one photo (or other media), in bytes.
pub(crate) const MAX_PHOTO_BYTES: usize = 1024 * 1024;

/// The version of a new card in a book that takes `versions`: 4.0 only
/// if the book lists it in `supported-address-data`. Without that
/// property the server "MUST only accept" vCard 3.0 (RFC 6352 §6.2.2),
/// the version every CardDAV server supports (§5.1); a list without any
/// `text/vcard` we know (only jCard, say) gets 3.0 for the same reason.
pub(crate) fn version_for(versions: Option<VcardVersions>) -> VcardVersion {
    match versions {
        Some(versions) if versions.contains(VcardVersions::V4) => VcardVersion::V4,
        _ => VcardVersion::V3,
    }
}

/// A random version 4 UUID, lower case.
pub(crate) fn uuid() -> Result<String, SetError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0f) | 0x40;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3f) | 0x80;
    }
    let hex = bytes.iter().fold(String::with_capacity(32), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    });
    let part = |from: usize, to: usize| hex.get(from..to).unwrap_or_default();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        part(0, 8),
        part(8, 12),
        part(12, 16),
        part(16, 20),
        part(20, 32)
    ))
}

/// The name of a new card's resource: `{uuid}.vcf`.
pub(crate) fn new_name() -> Result<ResourceName, SetError> {
    ResourceName::try_from(format!("{}.vcf", uuid()?).as_str())
        .map_err(|_| SetError::new(SetErrorType::Forbidden))
}

/// Photos and other media: links or `data:` URIs up to the limit.
pub(crate) fn check_media(body: &Map<String, Value>) -> Result<(), SetError> {
    let Some(Value::Object(media)) = body.get("media") else {
        return Ok(());
    };
    for entry in media.values() {
        if entry.get("blobId").is_some() {
            return Err(SetError::invalid_properties(["media"])
                .with_description("photos must be data: URIs or links, not blobs"));
        }
        if let Some(uri) = entry.get("uri").and_then(Value::as_str)
            && uri
                .get(..5)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
            && uri.len() > MAX_PHOTO_BYTES
        {
            return Err(SetError::new(SetErrorType::TooLarge)
                .with_description("a photo may be at most 1 MiB"));
        }
    }
    Ok(())
}

fn export_error(error: &ConvertError) -> SetError {
    match error {
        ConvertError::TooLarge { .. } => {
            SetError::new(SetErrorType::TooLarge).with_description("the card is too large")
        }
        _ => SetError::new(SetErrorType::InvalidProperties)
            .with_description("the card can't be written as a vCard"),
    }
}

/// Removes `@type` from nested objects (`Name`, `EmailAddress`,
/// `Phone`…): RFC 9553 lets it be left out where the property implies it,
/// and calcard would write each one as a `JSPROP` line that other clients
/// see as noise. Kept under `date`, where `PartialDate` and `Timestamp`
/// differ (calcard reads it). The card's own `@type` stays.
fn strip_implied_types(body: &mut Map<String, Value>) {
    let mut stack: Vec<&mut Value> = body.values_mut().collect();
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                object.remove("@type");
                for (key, child) in object.iter_mut() {
                    if key == "date" {
                        // Its own @type stays; nothing below it to clean.
                        continue;
                    }
                    stack.push(child);
                }
            }
            Value::Array(items) => stack.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// The vCard text of a JSContact body.
pub(crate) fn card_text(
    body: &Map<String, Value>,
    version: VcardVersion,
) -> Result<String, SetError> {
    card_text_in(body, VcardForm::standard(version))
}

fn card_text_in(body: &Map<String, Value>, form: VcardForm) -> Result<String, SetError> {
    let mut body = body.clone();
    strip_implied_types(&mut body);
    let json = Value::Object(body).to_string();
    jscontact_to_vcard_in(&json, form).map_err(|error| export_error(&error))
}

/// The JSContact body of a vCard. Members calcard writes that the JMAP
/// layer would refuse are left out (logged), rather than hiding the card.
///
/// # Errors
/// The conversion error (not a vCard, too large…), or `None` in it if the
/// body breaks a limit.
pub(crate) fn card_body(text: &[u8]) -> Result<JsBody<JsContact>, Option<ConvertError>> {
    card_parts(text).map(|(body, _)| body)
}

/// A card's body and the members its read left out.
type CardParts = (JsBody<JsContact>, Map<String, Value>);

/// [`card_body`] and the members it left out (name → value as calcard
/// read them), so that an edit can write them back.
fn card_parts(text: &[u8]) -> Result<CardParts, Option<ConvertError>> {
    let Value::Object(mut members) = vcard_to_jscontact(text).map_err(Some)? else {
        return Err(None);
    };
    match JsBody::new(members.clone()) {
        Ok(body) => Ok((body, Map::new())),
        Err(error) => {
            let names: Vec<String> = error.problems().iter().map(|(n, _)| n.clone()).collect();
            if names.iter().any(|name| name == "body") {
                return Err(None);
            }
            tracing::info!(count = names.len(), "contacts: card members left out");
            let mut left_out = Map::new();
            for name in &names {
                if let Some(value) = members.remove(name) {
                    left_out.insert(name.clone(), value);
                }
            }
            JsBody::new(members)
                .map(|body| (body, left_out))
                .map_err(|_| None)
        }
    }
}

/// Applies `patches` to the card stored as `text`: the new vCard text, or
/// `None` if the patch changes nothing. The version stays the stored one
/// (else `fallback`); a 3.0 card keeps its group in Apple's lines
/// (`X-ADDRESSBOOKSERVER-KIND`/`-MEMBER`, never `KIND`/`MEMBER` in 3.0).
/// `REV` (JSContact `updated`) becomes `now` (RFC 6350 §6.7.4, RFC 2426
/// §3.6.4: the revision of the card), unless the patch sets it.
pub(crate) fn patch_text(
    text: &str,
    patches: &[BodyPatch],
    fallback: VcardVersion,
    now: i64,
) -> Result<Option<String>, SetError> {
    let (mut body, left_out) = card_parts(text.as_bytes()).map_err(|_| {
        SetError::new(SetErrorType::Forbidden).with_description("this card can't be edited here")
    })?;
    let before = body.clone();
    body.apply(patches)?;
    if body == before {
        return Ok(None);
    }
    let touches_media = patches
        .iter()
        .any(|patch| patch.path.tokens().first().is_some_and(|t| t == "media"));
    if touches_media {
        check_media(body.as_map())?;
    }
    let form = vcard_form(text.as_bytes()).unwrap_or(VcardForm::standard(fallback));
    // What the read left out goes back as it was, unless a patch set that
    // member (then the new, checked value wins).
    let mut members = body.as_map().clone();
    for (name, value) in left_out {
        let touched = patches
            .iter()
            .any(|patch| patch.path.tokens().first().is_some_and(|t| *t == name));
        if !touched {
            members.entry(name).or_insert(value);
        }
    }
    let sets_updated = patches
        .iter()
        .any(|patch| patch.path.tokens().first().is_some_and(|t| t == "updated"));
    if !sets_updated {
        let rev = UtcDate::from_unix_timestamp(now)
            .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("clock"))?;
        members.insert("updated".to_owned(), Value::from(rev.to_string()));
    }
    card_text_in(&members, form).map(Some)
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::object::ContactCardPatch;
    use serde_json::json;

    use super::*;

    /// 2026-10-14T17:46:40Z.
    const NOW: i64 = 1_792_000_000;

    const APPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Apple Inc.//iPhone OS 17.0//EN\r\n\
        N:Pérez;Ana;;;\r\nFN:Ana Pérez\r\nORG:ACME;\r\n\
        item1.EMAIL;type=INTERNET;type=pref:ana@example.com\r\nitem1.X-ABLabel:_$!<Other>!$_\r\n\
        TEL;type=CELL;type=VOICE;type=pref:+34 600 000 000\r\nX-ABUID:1234-ABCD\r\n\
        X-CUSTOM-THING:keep me\r\nUID:apple-uid-1\r\nEND:VCARD\r\n";

    fn patches(value: Value) -> Vec<BodyPatch> {
        ContactCardPatch::parse(&serde_json::from_value(value).unwrap())
            .unwrap()
            .body
    }

    #[test]
    fn versions_and_names() {
        assert_eq!(version_for(None), VcardVersion::V3);
        assert_eq!(version_for(Some(VcardVersions::V4)), VcardVersion::V4);
        assert_eq!(version_for(Some(VcardVersions::V3)), VcardVersion::V3);
        assert_eq!(
            version_for(Some(VcardVersions::V3.union(VcardVersions::V4))),
            VcardVersion::V4
        );
        assert_eq!(version_for(Some(VcardVersions::empty())), VcardVersion::V3);
        let id = uuid().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(uuid().unwrap(), id);
        assert_eq!(
            std::path::Path::new(new_name().unwrap().as_str())
                .extension()
                .and_then(|e| e.to_str()),
            Some("vcf")
        );
    }

    /// `updateContact` on a card another client (Apple) wrote: what the
    /// web does not touch stays, version 3.0 included.
    #[test]
    fn web_edit_keeps_apple_properties() {
        let patch = patches(json!({
            "name": {"@type": "Name", "full": "Ana P. Pérez",
                     "components": [{"kind": "given", "value": "Ana"}, {"kind": "surname", "value": "Pérez"}]},
            "titles": {"t1": {"kind": "title", "name": "CTO"}},
            "notes": null, "addresses": null, "anniversaries": null, "members": null,
        }));
        let text = patch_text(APPLE, &patch, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"), "{text}");
        for kept in [
            "X-ABUID:1234-ABCD",
            "X-CUSTOM-THING:keep me",
            "item1.EMAIL;TYPE=INTERNET,PREF:ana@example.com",
            "item1.X-ABLABEL:_$!<Other>!$_",
            "PRODID:-//Apple Inc.//iPhone OS 17.0//EN",
            "UID:apple-uid-1",
            "ORG:ACME",
        ] {
            assert!(text.contains(kept), "{kept} missing:\n{text}");
        }
        assert!(
            text.contains("Ana P. Pérez") && text.contains("CTO"),
            "{text}"
        );
        // Read back (calcard names map entries itself: `t1` may be `k1`).
        let again = card_body(text.as_bytes()).unwrap();
        let titles = again.get("titles").unwrap().as_object().unwrap();
        assert!(titles.values().any(|title| title["name"] == "CTO"));
    }

    /// RFC 6350 §6.7.4 (and RFC 2426 §3.6.4 in 3.0): an edit renews `REV`,
    /// in UTC; a patch that sets `updated` wins; no change, no new `REV`.
    #[test]
    fn edits_renew_rev() {
        let stored = APPLE.replace(
            "UID:apple-uid-1\r\n",
            "UID:apple-uid-1\r\nREV:2020-01-01T00:00:00Z\r\n",
        );
        let rename = patches(json!({"name": {"@type": "Name", "full": "Ana B."}}));
        let text = patch_text(&stored, &rename, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"), "{text}");
        assert!(
            !text.contains("2020-01-01") && !text.contains("20200101"),
            "{text}"
        );
        assert_eq!(text.matches("REV").count(), 1, "{text}");
        assert!(text.contains("\r\nREV:20261014T174640Z\r\n"), "{text}");
        // Also on a card without one.
        let text = patch_text(APPLE, &rename, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert_eq!(text.matches("\r\nREV").count(), 1, "{text}");
        let own = patches(json!({"name": {"@type": "Name", "full": "Ana C."},
                                 "updated": "2025-05-05T05:05:05Z"}));
        let text = patch_text(&stored, &own, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.contains("2025"), "{text}");
        let same = patches(json!({"uid": "apple-uid-1"}));
        assert_eq!(
            patch_text(&stored, &same, VcardVersion::V4, NOW).unwrap(),
            None
        );
    }

    #[test]
    fn nothing_changed_and_deep_patches() {
        let same = patches(json!({"uid": "apple-uid-1"}));
        assert_eq!(
            patch_text(APPLE, &same, VcardVersion::V4, NOW).unwrap(),
            None
        );
        let body = card_body(APPLE.as_bytes()).unwrap();
        let email_key = body
            .get("emails")
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let deep = patches(json!({format!("emails/{email_key}/address"): "ana@new.example"}));
        let text = patch_text(APPLE, &deep, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.contains("ana@new.example") && !text.contains("ana@example.com"));
        let bad = patches(json!({"emails/nope/address": "x@y"}));
        assert_eq!(
            patch_text(APPLE, &bad, VcardVersion::V4, NOW)
                .unwrap_err()
                .kind(),
            SetErrorType::InvalidPatch
        );
    }

    /// `setGroupMembers` on a group card, and a group written new.
    #[test]
    fn groups() {
        let body = json!({"@type": "Card", "version": "1.0", "kind": "group", "uid": "urn:uuid:g",
                          "name": {"full": "Equipo"}, "members": {"urn:uuid:a": true}});
        let text = card_text(body.as_object().unwrap(), VcardVersion::V4).unwrap();
        assert!(
            text.contains("KIND:GROUP") && text.contains("MEMBER:urn:uuid:a"),
            "{text}"
        );
        let more = patches(json!({"members": {"urn:uuid:a": true, "urn:uuid:b": true}}));
        let text = patch_text(&text, &more, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        let back = card_body(text.as_bytes()).unwrap();
        assert_eq!(back.get("members").unwrap().as_object().unwrap().len(), 2);
        let none = patches(json!({"members": null}));
        let text = patch_text(&text, &none, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(!text.contains("MEMBER"), "{text}");
    }

    /// The group as Nextcloud 34 gives it back by GET or a plain
    /// multiget (it stores the 4.0 card the bridge wrote as 3.0).
    const NEXTCLOUD_GROUP: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\n\
        PRODID:-//Sabre//Sabre VObject 4.5.6//EN\r\nUID:urn:uuid:g1\r\nFN:Equipo\r\n\
        N:Equipo;;;;\r\nX-ADDRESSBOOKSERVER-KIND:GROUP\r\n\
        X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:x1\r\nEND:VCARD\r\n";

    /// A group as iCloud and macOS Contacts write it.
    const ICLOUD_GROUP: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\n\
        PRODID:-//Apple Inc.//iOS 17.0//EN\r\nN:Amigos\r\nFN:Amigos\r\n\
        X-ADDRESSBOOKSERVER-KIND:group\r\n\
        X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B\r\n\
        X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:9A8B7C6D-5E4F-4A3B-8C2D-1E0F9A8B7C6D\r\n\
        REV:2026-10-01T10:00:00Z\r\nUID:F1E2D3C4-B5A6-4978-8695-A4B3C2D1E0F9\r\nEND:VCARD\r\n";

    fn members_of(body: &JsBody<JsContact>) -> Vec<String> {
        let mut out: Vec<String> = body
            .get("members")
            .and_then(Value::as_object)
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    fn lines(text: &str, name: &str) -> Vec<String> {
        text.split("\r\n")
            .filter(|line| {
                line.split([':', ';'])
                    .next()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn apple_groups_are_read() {
        let body = card_body(NEXTCLOUD_GROUP.as_bytes()).unwrap();
        assert_eq!(body.get("kind").unwrap(), "group");
        assert_eq!(members_of(&body), ["urn:uuid:x1"]);
        // Not left as unknown properties as well.
        let leftovers = body.get("vCard").map(Value::to_string).unwrap_or_default();
        assert!(
            !leftovers.to_uppercase().contains("ADDRESSBOOKSERVER"),
            "{leftovers}"
        );

        let body = card_body(ICLOUD_GROUP.as_bytes()).unwrap();
        assert_eq!(body.get("kind").unwrap(), "group");
        assert_eq!(
            members_of(&body),
            [
                "urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B",
                "urn:uuid:9A8B7C6D-5E4F-4A3B-8C2D-1E0F9A8B7C6D"
            ]
        );
        // What Nextcloud gives when asked for 4.0: the same.
        let four = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:uuid:g1\r\nFN:Equipo\r\n\
            KIND:group\r\nMEMBER:urn:uuid:x1\r\nEND:VCARD\r\n";
        let body = card_body(four.as_bytes()).unwrap();
        assert_eq!(body.get("kind").unwrap(), "group");
        assert_eq!(members_of(&body), ["urn:uuid:x1"]);
    }

    /// `setGroupMembers` on a 3.0 group stored the Apple way: the X- lines
    /// change, no `KIND`/`MEMBER` appear, nothing stale is left.
    #[test]
    fn apple_groups_round_trip() {
        let add = patches(json!({"members": {
            "urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B": true,
            "urn:uuid:new-1": true
        }}));
        let text = patch_text(ICLOUD_GROUP, &add, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"), "{text}");
        assert!(
            lines(&text, "KIND").is_empty() && lines(&text, "MEMBER").is_empty(),
            "{text}"
        );
        assert_eq!(
            lines(&text, "X-ADDRESSBOOKSERVER-KIND"),
            ["X-ADDRESSBOOKSERVER-KIND:group"]
        );
        let mut members = lines(&text, "X-ADDRESSBOOKSERVER-MEMBER");
        members.sort();
        assert_eq!(
            members,
            [
                "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B",
                "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:new-1"
            ],
            "{text}"
        );
        for kept in [
            "PRODID:-//Apple Inc.//iOS 17.0//EN",
            "UID:F1E2D3C4-B5A6-4978-8695-A4B3C2D1E0F9",
        ] {
            assert!(text.contains(kept), "{kept} missing:\n{text}");
        }
        // Read again: the same group.
        let back = card_body(text.as_bytes()).unwrap();
        assert_eq!(back.get("kind").unwrap(), "group");
        assert_eq!(
            members_of(&back),
            [
                "urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B",
                "urn:uuid:new-1"
            ]
        );
        // Every member out: no member lines, still a group.
        let none = patches(json!({"members": null}));
        let empty = patch_text(&text, &none, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(
            lines(&empty, "X-ADDRESSBOOKSERVER-MEMBER").is_empty(),
            "{empty}"
        );
        assert_eq!(
            card_body(empty.as_bytes()).unwrap().get("kind").unwrap(),
            "group"
        );
        // A rename keeps it a group the Apple way.
        let rename = patches(json!({"name": {"full": "Amigos 2"}}));
        let renamed = patch_text(NEXTCLOUD_GROUP, &rename, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(
            renamed.contains("X-ADDRESSBOOKSERVER-KIND:group"),
            "{renamed}"
        );
        assert!(
            renamed.contains("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:x1"),
            "{renamed}"
        );
        assert!(lines(&renamed, "KIND").is_empty(), "{renamed}");
        // An individual again: no kind line at all (the default).
        let person = patches(json!({"kind": "individual", "members": null}));
        let person = patch_text(NEXTCLOUD_GROUP, &person, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(
            !person.contains("ADDRESSBOOKSERVER") && lines(&person, "KIND").is_empty(),
            "{person}"
        );
        // A 4.0 card stays 4.0 with KIND and MEMBER.
        let four = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:g\r\nFN:E\r\nKIND:group\r\n\
            MEMBER:urn:uuid:a\r\nEND:VCARD\r\n";
        let more = patches(json!({"members": {"urn:uuid:a": true, "urn:uuid:b": true}}));
        let four = patch_text(four, &more, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(
            four.contains("KIND:GROUP") && !four.contains("ADDRESSBOOKSERVER"),
            "{four}"
        );
        assert_eq!(lines(&four, "MEMBER").len(), 2, "{four}");
    }

    #[test]
    fn hostile_apple_groups() {
        // Odd casing everywhere, an unknown kind, an empty member.
        let odd = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:g\r\nFN:G\r\n\
            x-AddressBookServer-Kind:  GrOuP \r\n\
            X-ADDRESSBOOKSERVER-KIND:nonsense\r\n\
            x-addressbookserver-member:URN:UUID:A\r\n\
            X-ADDRESSBOOKSERVER-MEMBER:not a uri\r\n\
            X-ADDRESSBOOKSERVER-MEMBER:\r\n\
            X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:a\r\nEND:VCARD\r\n";
        let body = card_body(odd.as_bytes()).unwrap();
        assert_eq!(body.get("kind").unwrap(), "group");
        assert_eq!(members_of(&body), ["URN:UUID:A", "not a uri", "urn:uuid:a"]);
        // The lines it does not understand are kept, and written back.
        let rename = patches(json!({"name": {"full": "G2"}}));
        let text = patch_text(odd, &rename, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.contains("X-ADDRESSBOOKSERVER-KIND:nonsense"), "{text}");
        assert!(
            text.contains("X-ADDRESSBOOKSERVER-MEMBER:not a uri"),
            "{text}"
        );
        assert!(text.contains("X-ADDRESSBOOKSERVER-KIND:group"), "{text}");
        assert_eq!(
            lines(&text, "X-ADDRESSBOOKSERVER-MEMBER").len(),
            4,
            "{text}"
        );

        // A real KIND wins: the X- lines are left alone, unread.
        let both = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:g\r\nFN:G\r\nKIND:individual\r\n\
            X-ADDRESSBOOKSERVER-KIND:group\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:a\r\nEND:VCARD\r\n";
        let body = card_body(both.as_bytes()).unwrap();
        assert_eq!(body.get("kind").unwrap(), "individual");
        assert!(members_of(&body).is_empty());

        // A huge member list (inside the 4 MiB card limit): linear, all read.
        let mut huge = String::from(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:g\r\nFN:G\r\nX-ADDRESSBOOKSERVER-KIND:group\r\n",
        );
        for n in 0..20_000 {
            let _ = write!(huge, "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:{n:08}\r\n");
        }
        huge.push_str("END:VCARD\r\n");
        let body = card_body(huge.as_bytes()).unwrap();
        assert_eq!(members_of(&body).len(), 20_000);
        let text = patch_text(&huge, &rename, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert_eq!(lines(&text, "X-ADDRESSBOOKSERVER-MEMBER").len(), 20_000);
        assert!(lines(&text, "MEMBER").is_empty());
    }

    #[test]
    fn photos() {
        let small = json!({"media": {"m1": {"kind": "photo", "uri": "data:image/png;base64,iVBORw0KGgo="}}});
        assert!(check_media(small.as_object().unwrap()).is_ok());
        let link = json!({"media": {"m1": {"kind": "photo", "uri": "https://example.com/a.png"}}});
        assert!(check_media(link.as_object().unwrap()).is_ok());
        let big = format!("data:image/png;base64,{}", "A".repeat(MAX_PHOTO_BYTES));
        let large = json!({"media": {"m1": {"kind": "photo", "uri": big}}});
        assert_eq!(
            check_media(large.as_object().unwrap()).unwrap_err().kind(),
            SetErrorType::TooLarge
        );
        let blob = json!({"media": {"m1": {"kind": "photo", "blobId": "bu1"}}});
        assert_eq!(
            check_media(blob.as_object().unwrap())
                .unwrap_err()
                .properties(),
            ["media"]
        );
        // A photo added by a patch is checked; an untouched one is not.
        let card = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:X\r\nUID:x\r\nEND:VCARD\r\n";
        let add = patches(json!({"media": {"m1": {"kind": "photo", "blobId": "bu1"}}}));
        assert!(patch_text(card, &add, VcardVersion::V4, NOW).is_err());
        let ok = patches(
            json!({"media": {"m1": {"kind": "photo", "uri": "data:image/png;base64,iVBORw0KGgo="}}}),
        );
        let text = patch_text(card, &ok, VcardVersion::V4, NOW)
            .unwrap()
            .unwrap();
        assert!(text.contains("PHOTO"), "{text}");
    }

    /// The web sends `@type` everywhere (`newCard`): no `JSPROP` noise in
    /// the vCard, and birthdays keep their kind of date.
    #[test]
    fn implied_types_are_not_written() {
        let body = json!({"@type": "Card", "version": "1.0", "kind": "individual", "uid": "u",
        "name": {"@type": "Name", "full": "Carla Gómez",
                 "components": [{"@type": "NameComponent", "kind": "given", "value": "Carla"}]},
        "emails": {"e1": {"@type": "EmailAddress", "address": "carla@example.com"}},
        "phones": {"p1": {"@type": "Phone", "number": "+34 600", "features": {"mobile": true}}},
        "anniversaries": {
            "a1": {"@type": "Anniversary", "kind": "birth",
                   "date": {"@type": "PartialDate", "month": 3, "day": 14}},
            "a2": {"kind": "wedding", "date": {"@type": "Timestamp", "utc": "2020-01-01T00:00:00Z"}}
        }});
        let text = card_text(body.as_object().unwrap(), VcardVersion::V4).unwrap();
        assert!(!text.contains("JSPROP"), "{text}");
        assert!(text.contains("BDAY;PROP-ID=a1:--0314"), "{text}");
        assert!(
            text.contains("ANNIVERSARY;PROP-ID=a2:20200101T000000Z"),
            "{text}"
        );
        let back = card_body(text.as_bytes()).unwrap();
        assert_eq!(back.get("@type").unwrap(), "Card");
        assert_eq!(back.get("name").unwrap()["full"], "Carla Gómez");
    }

    #[test]
    fn bad_input() {
        assert!(card_body(b"not a vcard").is_err());
        assert_eq!(
            patch_text("garbage", &[], VcardVersion::V4, NOW)
                .unwrap_err()
                .kind(),
            SetErrorType::Forbidden
        );
    }

    /// A member the read leaves out (a `UID` longer than a line JMAP
    /// accepts) is written back by an edit that doesn't touch it.
    #[test]
    fn edit_keeps_members_left_out() {
        let uid = "u".repeat(1100);
        let text = format!("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ana\r\nUID:{uid}\r\nEND:VCARD\r\n");
        assert!(
            card_body(text.as_bytes())
                .unwrap()
                .as_map()
                .get("uid")
                .is_none()
        );
        let unfold = |text: &str| text.replace("\r\n ", "");
        let renamed = patch_text(
            &text,
            &patches(json!({"name": {"@type": "Name", "full": "Ana B."}})),
            VcardVersion::V4,
            NOW,
        )
        .unwrap()
        .unwrap();
        assert!(
            unfold(&renamed).contains(&format!("UID:{uid}\r\n")),
            "{renamed}"
        );
        // A patch that sets the member replaces it.
        let replaced = patch_text(
            &text,
            &patches(json!({"uid": "new-uid"})),
            VcardVersion::V4,
            NOW,
        )
        .unwrap()
        .unwrap();
        let replaced = unfold(&replaced);
        assert!(replaced.contains("UID:new-uid\r\n"), "{replaced}");
        assert!(!replaced.contains(&uid), "{replaced}");
    }

    /// A name with a line break from JSON (`vCard.properties`) is refused,
    /// not written as a second card.
    #[test]
    fn names_with_line_breaks_are_invalid() {
        let mut body = Map::new();
        body.insert("@type".into(), json!("Card"));
        body.insert("version".into(), json!("1.0"));
        body.insert("uid".into(), json!("u1"));
        body.insert(
            "vCard".into(),
            json!({"properties": [["x-a:b\r\nEND:VCARD\r\nBEGIN:VCARD\r\nX-B", {}, "text", "v"]]}),
        );
        for version in [VcardVersion::V3, VcardVersion::V4] {
            let error = card_text(&body, version).unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{error:?}");
        }
    }
}
