// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The sealed local copy of a resource: its
//! text encrypted with the account's data key as `Field::DavResource`,
//! bound to the account and to **the resource** (its collection's id and
//! its href). Someone who can write to the database can't read it, move it
//! to another account, or swap one card's or event's text for another's
//! of the same account: the AAD of each one names its own place.
//!
//! A resource moved by another client (same `UID`, another collection)
//! keeps its id, but its text is fetched again and sealed for the new
//! place, as is the text after a MOVE by the bridge.

use nebula_crypto::{AccountKey, Binding, CryptoError, Field, SealedField};
use nebula_store::{CollectionId, DavHref};
use secrecy::SecretSlice;

/// The bytes of the binding: the collection id (8 bytes, big-endian), then
/// the href. The id has a fixed length, so the two can't run together.
#[must_use]
pub fn binding(collection: CollectionId, href: &DavHref) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8_usize.saturating_add(href.as_str().len()));
    bytes.extend_from_slice(&collection.get().to_be_bytes());
    bytes.extend_from_slice(href.as_str().as_bytes());
    bytes
}

/// Seals the text of the resource at `href` in `collection`.
///
/// # Errors
/// [`CryptoError`] if sealing fails (too long, no randomness).
pub fn seal(
    key: &AccountKey,
    collection: CollectionId,
    href: &DavHref,
    text: &[u8],
) -> Result<SealedField, CryptoError> {
    let bytes = binding(collection, href);
    key.seal_bound(Field::DavResource, &Binding::new(&bytes)?, text)
}

/// Opens the stored sealed text of the resource at `href` in
/// `collection`.
///
/// # Errors
/// [`CryptoError::Decrypt`] if it was sealed for another account or
/// resource (or changed); [`CryptoError::Format`] if it is not a sealed
/// value (or one of the retired, unbound format).
pub fn open(
    key: &AccountKey,
    collection: CollectionId,
    href: &DavHref,
    stored: &[u8],
) -> Result<SecretSlice<u8>, CryptoError> {
    let sealed = SealedField::from_bytes(stored)?;
    let bytes = binding(collection, href);
    key.open_bound(Field::DavResource, &Binding::new(&bytes)?, &sealed)
}

#[cfg(test)]
mod tests {
    use nebula_crypto::{AccountId, DataKey};
    use secrecy::ExposeSecret as _;

    use super::*;

    #[test]
    fn bound_to_collection_and_href() {
        let key = AccountKey::new(AccountId::new(b"a").unwrap(), DataKey::generate().unwrap());
        let one = CollectionId::from_raw(1).unwrap();
        let two = CollectionId::from_raw(2).unwrap();
        let a = DavHref::new("https://dav.example/books/a.vcf").unwrap();
        let b = DavHref::new("https://dav.example/books/b.vcf").unwrap();
        let sealed = seal(&key, one, &a, b"BEGIN:VCARD").unwrap();
        let bytes = sealed.as_bytes();
        assert_eq!(
            open(&key, one, &a, bytes).unwrap().expose_secret(),
            b"BEGIN:VCARD"
        );
        assert_eq!(
            open(&key, one, &b, bytes).unwrap_err(),
            CryptoError::Decrypt
        );
        assert_eq!(
            open(&key, two, &a, bytes).unwrap_err(),
            CryptoError::Decrypt
        );
        assert_eq!(binding(one, &a)[..8], [0, 0, 0, 0, 0, 0, 0, 1]);
    }
}
