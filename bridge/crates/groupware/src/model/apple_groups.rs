// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Contact groups the Apple way: `X-ADDRESSBOOKSERVER-KIND:group` and one
//! `X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:…` per member. vCard 3.0 has no
//! `KIND` or `MEMBER` (RFC 6350 added them), so Apple's servers and
//! clients (iCloud, macOS Contacts Server, iOS) and many 3.0 clients use
//! these, and Nextcloud (sabre/dav) stores a 4.0 group in this form.
//!
//! - Reading: a card with `X-ADDRESSBOOKSERVER-KIND` and no `KIND` gets
//!   `KIND` and `MEMBER` from them before calcard converts it, so
//!   `ContactCard/get` sees `kind: "group"` and `members`. The X- lines
//!   that were turned into standard ones are gone, so a write never
//!   leaves stale members behind. An empty or over-long member, or one
//!   with control characters, stays an X- line (kept as an unknown
//!   property, not shown as a member).
//! - Writing any 3.0 card (RFC 2426, `super::convert::to_rfc2426`):
//!   `KIND` and `MEMBER` become the X- lines (an individual just has
//!   none), never 4.0-only properties in a 3.0 card. If the card had
//!   `KIND` too, its old X- lines that read as a kind or a member (not
//!   read as the group) go, so they are not written twice.
//!
//! No limit of its own: it is linear in the card, which is already capped
//! at [`MAX_VCARD_BYTES`](super::convert::MAX_VCARD_BYTES).

use calcard::common::{IanaParse as _, IanaString as _};
use calcard::vcard::{VCard, VCardEntry, VCardKind, VCardProperty, VCardValue};

const KIND: &str = "X-ADDRESSBOOKSERVER-KIND";
const MEMBER: &str = "X-ADDRESSBOOKSERVER-MEMBER";

/// Longest member taken from an X- line, in bytes (a longer one stays an
/// X- line).
pub(crate) const MAX_MEMBER_URI: usize = 4096;

fn named(entry: &VCardEntry, name: &str) -> bool {
    matches!(&entry.name, VCardProperty::Other(other) if other.eq_ignore_ascii_case(name))
}

/// The single text value of an entry, trimmed.
fn text(entry: &VCardEntry) -> Option<&str> {
    match entry.values.as_slice() {
        [value] => value.as_text().map(str::trim),
        _ => None,
    }
}

/// The kind an `X-ADDRESSBOOKSERVER-KIND` line says (any case).
fn kind_of(entry: &VCardEntry) -> Option<VCardKind> {
    named(entry, KIND)
        .then(|| text(entry))
        .flatten()
        .and_then(|value| VCardKind::parse(value.as_bytes()))
}

/// A member as JSContact keys it (a `uid`, usually `urn:uuid:…`, but any
/// text): not empty, no control characters, at most [`MAX_MEMBER_URI`]
/// bytes.
fn is_member(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_MEMBER_URI && !value.chars().any(char::is_control)
}

/// The card stores its group the Apple way: an `X-ADDRESSBOOKSERVER-KIND`
/// calcard understands and no `KIND`.
pub(crate) fn is_apple_group(card: &VCard) -> bool {
    !card
        .entries
        .iter()
        .any(|entry| entry.name == VCardProperty::Kind)
        && card.entries.iter().any(|entry| kind_of(entry).is_some())
}

fn entry(name: VCardProperty, value: VCardValue) -> VCardEntry {
    VCardEntry {
        group: None,
        name,
        params: Vec::new(),
        values: std::iter::once(value).collect(),
    }
}

/// Turns Apple's group lines into `KIND` and `MEMBER` (see the module
/// docs). Nothing changes on a card that is not [`is_apple_group`].
pub(crate) fn read(card: &mut VCard) {
    if !is_apple_group(card) {
        return;
    }
    let mut kind_done = false;
    let entries = std::mem::take(&mut card.entries);
    card.entries = entries
        .into_iter()
        .filter_map(|old| {
            if named(&old, KIND) {
                // The first one that parses; the others would go stale.
                return match kind_of(&old) {
                    Some(kind) if !kind_done => {
                        kind_done = true;
                        Some(entry(VCardProperty::Kind, VCardValue::Kind(kind)))
                    }
                    Some(_) => None,
                    None => Some(old),
                };
            }
            if named(&old, MEMBER)
                && let Some(uri) = text(&old).filter(|uri| is_member(uri))
            {
                return Some(entry(
                    VCardProperty::Member,
                    VCardValue::Text(uri.to_owned()),
                ));
            }
            Some(old)
        })
        .collect();
}

/// Writes `KIND` and `MEMBER` the Apple way (for a 3.0 card):
/// `X-ADDRESSBOOKSERVER-KIND` in lower case as Apple writes it, nothing
/// for an individual (the default).
pub(crate) fn write(card: &mut VCard) {
    let has_kind = card
        .entries
        .iter()
        .any(|entry| entry.name == VCardProperty::Kind);
    let entries = std::mem::take(&mut card.entries);
    card.entries = entries
        .into_iter()
        // With a `KIND`, X- lines that read as a kind or a member were
        // not read (see [`read`]): writing them next to the new ones would
        // say two things. The ones that never read stay, as unknown lines.
        .filter(|old| {
            let stale =
                kind_of(old).is_some() || (named(old, MEMBER) && text(old).is_some_and(is_member));
            !(has_kind && stale)
        })
        .filter_map(|old| match old.name {
            VCardProperty::Kind => {
                let kind = old.values.iter().find_map(|value| match value {
                    VCardValue::Kind(kind) => Some(*kind),
                    other => other
                        .as_text()
                        .and_then(|text| VCardKind::parse(text.trim().as_bytes())),
                });
                match kind {
                    Some(VCardKind::Individual) => None,
                    Some(kind) => Some(entry(
                        VCardProperty::Other(KIND.to_owned()),
                        VCardValue::Text(kind.as_str().to_ascii_lowercase()),
                    )),
                    // Not a kind calcard knows: kept as it is.
                    None => Some(old),
                }
            }
            VCardProperty::Member => match text(&old) {
                Some(uri) => Some(entry(
                    VCardProperty::Other(MEMBER.to_owned()),
                    VCardValue::Text(uri.to_owned()),
                )),
                None => Some(old),
            },
            _ => Some(old),
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members() {
        for good in [
            "urn:uuid:x1",
            "urn:uuid:4E3B8F2A-1C2D-4E5F-8A9B-0C1D2E3F4A5B",
            "mailto:ana@example.com",
            "nebula-test-apple-1",
            "not a uri",
        ] {
            assert!(is_member(good), "{good}");
        }
        let long = format!("urn:uuid:{}", "a".repeat(MAX_MEMBER_URI));
        for bad in ["", "urn:a\u{7}", "urn:\u{0}", "a\u{85}b", long.as_str()] {
            assert!(!is_member(bad), "{bad:?}");
        }
    }
}
