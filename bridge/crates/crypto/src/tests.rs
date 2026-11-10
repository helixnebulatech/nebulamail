// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of the whole key hierarchy: keyring, data keys and sealed fields.

use proptest::prelude::*;
use secrecy::ExposeSecret;

use crate::format::KEY_LEN;
use crate::{
    AccountId, Binding, CryptoError, DataKey, Field, FormatError, KeyVersion, Keyring, MasterKey,
    SealedField, WrappedDataKey,
};

fn master(version: u16, byte: u8) -> MasterKey {
    MasterKey::from_bytes(KeyVersion::new(version).unwrap(), &[byte; KEY_LEN]).unwrap()
}

fn account(id: &str) -> AccountId {
    AccountId::new(id.as_bytes()).unwrap()
}

fn flip(bytes: &[u8], index: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[index] ^= 0x01;
    out
}

#[test]
fn wrap_unwrap_round_trip() {
    let keyring = Keyring::new(master(1, 1), []).unwrap();
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let wrapped = keyring.wrap(&alice, &dek).unwrap();
    assert_eq!(wrapped.as_bytes().len(), WrappedDataKey::LEN);
    assert_eq!(wrapped.key_version().get(), 1);

    let stored = WrappedDataKey::from_bytes(wrapped.as_bytes()).unwrap();
    let unwrapped = keyring.unwrap(&alice, &stored).unwrap();
    assert_eq!(unwrapped.bytes(), dek.bytes());
}

#[test]
fn unwrap_with_wrong_master_key_or_account_fails() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let wrapped = Keyring::new(master(1, 1), [])
        .unwrap()
        .wrap(&alice, &dek)
        .unwrap();

    let other_key = Keyring::new(master(1, 2), []).unwrap();
    assert_eq!(
        other_key.unwrap(&alice, &wrapped).unwrap_err(),
        CryptoError::Decrypt
    );
    let right_key = Keyring::new(master(1, 1), []).unwrap();
    assert_eq!(
        right_key.unwrap(&account("bob"), &wrapped).unwrap_err(),
        CryptoError::Decrypt
    );
}

#[test]
fn rotation_unwraps_old_and_wraps_with_new() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let old = Keyring::new(master(1, 1), [])
        .unwrap()
        .wrap(&alice, &dek)
        .unwrap();

    let rotated = Keyring::new(master(2, 2), [master(1, 1)]).unwrap();
    assert_eq!(rotated.current_version().get(), 2);
    assert!(rotated.needs_rewrap(&old));
    let unwrapped = rotated.unwrap(&alice, &old).unwrap();
    let new = rotated.wrap(&alice, &unwrapped).unwrap();
    assert_eq!(new.key_version().get(), 2);
    assert!(!rotated.needs_rewrap(&new));

    // Secrets sealed before the rotation still open: the DEK didn't change.
    let sealed = dek.seal(&alice, Field::ImapPassword, b"pw").unwrap();
    let after = rotated.unwrap(&alice, &new).unwrap();
    assert_eq!(
        after
            .open(&alice, Field::ImapPassword, &sealed)
            .unwrap()
            .expose_secret(),
        b"pw"
    );

    // `rewrap` does the same in one step, and nothing for current keys.
    let again = rotated.rewrap(&alice, &old).unwrap().unwrap();
    assert_eq!(again.key_version().get(), 2);
    assert!(rotated.rewrap(&alice, &again).unwrap().is_none());
    assert_eq!(
        rotated.rewrap(&account("bob"), &old).unwrap_err(),
        CryptoError::Decrypt
    );
    assert_eq!(rotated.previous_versions(), [KeyVersion::new(1).unwrap()]);

    // Once the old key is removed, old values say which key is missing.
    let only_new = Keyring::new(master(2, 2), []).unwrap();
    assert_eq!(
        only_new.unwrap(&alice, &old).unwrap_err(),
        CryptoError::UnknownKeyVersion(KeyVersion::new(1).unwrap())
    );
}

#[test]
fn changed_version_nonce_or_tag_fails() {
    let alice = account("alice");
    // Same key bytes under both versions: only the AAD tells them apart.
    let keyring = Keyring::new(master(2, 2), [master(1, 2)]).unwrap();
    let dek = DataKey::generate().unwrap();
    let wrapped = keyring.wrap(&alice, &dek).unwrap();
    let bytes = wrapped.as_bytes();

    // Byte 3 is the low byte of the key version: 2 becomes 3 (unknown)...
    let unknown = WrappedDataKey::from_bytes(&flip(bytes, 3)).unwrap();
    assert_eq!(
        keyring.unwrap(&alice, &unknown).unwrap_err(),
        CryptoError::UnknownKeyVersion(KeyVersion::new(3).unwrap())
    );
    // ...or 1, which exists with the same key bytes, but the AAD differs.
    let mut downgraded = bytes.to_vec();
    downgraded[3] = 1;
    let downgraded = WrappedDataKey::from_bytes(&downgraded).unwrap();
    assert_eq!(
        keyring.unwrap(&alice, &downgraded).unwrap_err(),
        CryptoError::Decrypt
    );
    for index in 4..WrappedDataKey::LEN {
        let changed = WrappedDataKey::from_bytes(&flip(bytes, index)).unwrap();
        assert_eq!(
            keyring.unwrap(&alice, &changed).unwrap_err(),
            CryptoError::Decrypt,
            "byte {index}"
        );
    }
}

#[test]
fn wrapped_format_is_parsed_strictly() {
    let keyring = Keyring::new(master(1, 1), []).unwrap();
    let alice = account("alice");
    let wrapped = keyring.wrap(&alice, &DataKey::generate().unwrap()).unwrap();
    let bytes = wrapped.as_bytes();

    for len in 0..WrappedDataKey::LEN {
        assert!(
            matches!(
                WrappedDataKey::from_bytes(&bytes[..len]),
                Err(CryptoError::Format(FormatError::Truncated))
            ),
            "length {len}"
        );
    }
    let mut longer = bytes.to_vec();
    longer.push(0);
    assert_eq!(
        WrappedDataKey::from_bytes(&longer).unwrap_err(),
        CryptoError::Format(FormatError::TooLong)
    );
    assert_eq!(
        WrappedDataKey::from_bytes(&flip(bytes, 0)).unwrap_err(),
        CryptoError::Format(FormatError::UnsupportedVersion(0))
    );
    let mut kind = bytes.to_vec();
    kind[1] = 2;
    assert_eq!(
        WrappedDataKey::from_bytes(&kind).unwrap_err(),
        CryptoError::Format(FormatError::WrongKind)
    );
    let mut zero_version = bytes.to_vec();
    zero_version[2] = 0;
    zero_version[3] = 0;
    assert_eq!(
        WrappedDataKey::from_bytes(&zero_version).unwrap_err(),
        CryptoError::KeyVersionZero
    );
}

#[test]
fn seal_open_round_trip() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let sealed = dek.seal(&alice, Field::ImapPassword, b"hunter2").unwrap();
    let stored = SealedField::from_bytes(sealed.as_bytes()).unwrap();
    assert_eq!(stored.field(), Field::ImapPassword);
    let opened = dek.open(&alice, Field::ImapPassword, &stored).unwrap();
    assert_eq!(opened.expose_secret(), b"hunter2");
    let text = dek
        .open_string(&alice, Field::ImapPassword, &stored)
        .unwrap();
    assert_eq!(text.expose_secret(), "hunter2");
}

#[test]
fn sealing_twice_gives_different_ciphertexts() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let a = dek.seal(&alice, Field::ImapPassword, b"same").unwrap();
    let b = dek.seal(&alice, Field::ImapPassword, b"same").unwrap();
    assert_ne!(a.as_bytes(), b.as_bytes());
}

#[test]
fn open_with_wrong_key_account_or_field_fails() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let sealed = dek.seal(&alice, Field::ImapPassword, b"hunter2").unwrap();

    let other_dek = DataKey::generate().unwrap();
    assert_eq!(
        other_dek
            .open(&alice, Field::ImapPassword, &sealed)
            .unwrap_err(),
        CryptoError::Decrypt
    );
    assert_eq!(
        dek.open(&account("bob"), Field::ImapPassword, &sealed)
            .unwrap_err(),
        CryptoError::Decrypt
    );
    assert_eq!(
        dek.open(&alice, Field::SmtpPassword, &sealed).unwrap_err(),
        CryptoError::FieldMismatch {
            expected: Field::SmtpPassword,
            found: Field::ImapPassword
        }
    );
    // Rewriting the field tag in the stored bytes doesn't help either.
    let mut moved = sealed.as_bytes().to_vec();
    moved[2] = 2;
    let moved = SealedField::from_bytes(&moved).unwrap();
    assert_eq!(
        dek.open(&alice, Field::SmtpPassword, &moved).unwrap_err(),
        CryptoError::Decrypt
    );
}

#[test]
fn changed_nonce_ciphertext_or_tag_fails() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let sealed = dek
        .seal(&alice, Field::OAuthRefreshToken, b"refresh")
        .unwrap();
    let bytes = sealed.as_bytes();
    for index in 3..bytes.len() {
        let changed = SealedField::from_bytes(&flip(bytes, index)).unwrap();
        assert_eq!(
            dek.open(&alice, Field::OAuthRefreshToken, &changed)
                .unwrap_err(),
            CryptoError::Decrypt,
            "byte {index}"
        );
    }
}

#[test]
fn a_wrapped_key_is_not_a_sealed_field() {
    let alice = account("alice");
    let keyring = Keyring::new(master(1, 1), []).unwrap();
    let wrapped = keyring.wrap(&alice, &DataKey::generate().unwrap()).unwrap();
    assert_eq!(
        SealedField::from_bytes(wrapped.as_bytes()).unwrap_err(),
        CryptoError::Format(FormatError::WrongKind)
    );
}

#[test]
fn sealed_format_is_parsed_strictly() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let sealed = dek.seal(&alice, Field::ImapPassword, b"").unwrap();
    let bytes = sealed.as_bytes();
    // Empty secret: header, nonce and tag only.
    assert_eq!(bytes.len(), 3 + 24 + 16);
    for len in 0..bytes.len() {
        assert!(
            SealedField::from_bytes(&bytes[..len]).is_err(),
            "length {len}"
        );
    }
    let mut unknown = bytes.to_vec();
    unknown[2] = 200;
    assert_eq!(
        SealedField::from_bytes(&unknown).unwrap_err(),
        CryptoError::Format(FormatError::UnknownField(200))
    );
    let mut version = bytes.to_vec();
    version[0] = 2;
    assert_eq!(
        SealedField::from_bytes(&version).unwrap_err(),
        CryptoError::Format(FormatError::UnsupportedVersion(2))
    );
    let oversized = vec![1u8; SealedField::sealed_len(SealedField::MAX_MESSAGE_LEN) + 1];
    assert_eq!(
        SealedField::from_bytes(&oversized).unwrap_err(),
        CryptoError::Format(FormatError::TooLong)
    );
}

/// Calendars and contacts may be far longer than secrets, but only in
/// their own field: a long value relabelled as a password is refused.
#[test]
fn dav_resources_have_their_own_limit() {
    let alice = account("alice");
    let key = crate::AccountKey::new(alice.clone(), DataKey::generate().unwrap());
    assert_eq!(format!("{key:?}"), "AccountKey(..)");
    let card = vec![b'v'; SealedField::MAX_SECRET_LEN * 4];
    let here = Binding::new(b"7 https://dav.example/card/a.vcf").unwrap();
    let sealed = key.seal_bound(Field::DavResource, &here, &card).unwrap();
    let stored = SealedField::from_bytes(sealed.as_bytes()).unwrap();
    assert_eq!(
        key.open_bound(Field::DavResource, &here, &stored)
            .unwrap()
            .expose_secret(),
        card.as_slice()
    );
    let mut relabelled = sealed.as_bytes().to_vec();
    relabelled[2] = 1;
    assert_eq!(
        SealedField::from_bytes(&relabelled).unwrap_err(),
        CryptoError::Format(FormatError::TooLong)
    );
    let max = vec![b'x'; SealedField::MAX_DATA_LEN];
    assert!(key.seal_bound(Field::DavResource, &here, &max).is_ok());
    let too_long = vec![b'x'; SealedField::MAX_DATA_LEN + 1];
    assert_eq!(
        key.seal_bound(Field::DavResource, &here, &too_long)
            .unwrap_err(),
        CryptoError::SecretTooLong
    );
    // Bound to the account, like every sealed field.
    let bob = crate::AccountKey::new(account("bob"), DataKey::generate().unwrap());
    assert!(bob.open_bound(Field::DavResource, &here, &stored).is_err());
    assert_eq!(key.account(), &alice);
}

/// A DAV resource opens only as the record it was sealed for: swapping
/// two ciphertexts of the same account in the database fails.
#[test]
fn bound_fields_open_only_for_their_record() {
    let key = crate::AccountKey::new(account("alice"), DataKey::generate().unwrap());
    let first = Binding::new(b"7 https://dav.example/card/a.vcf").unwrap();
    let second = Binding::new(b"7 https://dav.example/card/b.vcf").unwrap();
    let sealed = key.seal_bound(Field::DavResource, &first, b"A").unwrap();
    assert_eq!(
        key.open_bound(Field::DavResource, &second, &sealed)
            .unwrap_err(),
        CryptoError::Decrypt
    );
    // The prefix keeps "ab" + "c" apart from "a" + "bc".
    let empty = Binding::new(b"").unwrap();
    let sealed_empty = key.seal_bound(Field::DavResource, &empty, b"A").unwrap();
    assert!(
        key.open_bound(Field::DavResource, &first, &sealed_empty)
            .is_err()
    );
    // A bound field needs its binding; a secret takes none.
    assert_eq!(
        key.seal(Field::DavResource, b"A").unwrap_err(),
        CryptoError::WrongBinding(Field::DavResource)
    );
    assert_eq!(
        key.open(Field::DavResource, &sealed).unwrap_err(),
        CryptoError::WrongBinding(Field::DavResource)
    );
    assert_eq!(
        key.seal_bound(Field::ImapPassword, &first, b"pw")
            .unwrap_err(),
        CryptoError::WrongBinding(Field::ImapPassword)
    );
    // Web Push subscriptions are bound too; the VAPID key is not. A
    // duplicate of the key opens what the original sealed.
    let push_id = 7_i64.to_be_bytes();
    let push = Binding::new(&push_id).unwrap();
    let url = key
        .seal_bound(
            Field::PushSubscription,
            &push,
            b"https://fcm.googleapis.com/x",
        )
        .unwrap();
    assert!(key.seal(Field::PushSubscription, b"x").is_err());
    let copy = key.duplicate();
    assert_eq!(
        copy.open_bound(Field::PushSubscription, &push, &url)
            .unwrap()
            .expose_secret(),
        b"https://fcm.googleapis.com/x"
    );
    assert!(
        copy.open_bound(Field::PushSubscription, &first, &url)
            .is_err()
    );
    let vapid = key.seal(Field::VapidKey, &[1; 32]).unwrap();
    assert_eq!(SealedField::from_bytes(vapid.as_bytes()).unwrap(), vapid);
    // Calendar links are bound: one can't be read as another's.
    let link_place = b"https://dav.example/cal/work/\0publish";
    let link = key
        .seal_bound(
            Field::DavCollectionLink,
            &Binding::new(link_place).unwrap(),
            b"https://cloud.example/p/X",
        )
        .unwrap();
    assert!(key.seal(Field::DavCollectionLink, b"x").is_err());
    let parsed = SealedField::from_bytes(link.as_bytes()).unwrap();
    assert_eq!(parsed.field(), Field::DavCollectionLink);
    assert!(
        key.open_bound(Field::DavCollectionLink, &push, &parsed)
            .is_err()
    );
    // Held messages are bound to their submission and may be large.
    let first_hold = Binding::new(b"submission\x001").unwrap();
    let other_hold = Binding::new(b"submission\x002").unwrap();
    let big = vec![b'm'; SealedField::MAX_DATA_LEN + 1];
    let held = key
        .seal_bound(Field::HeldMessage, &first_hold, &big)
        .unwrap();
    assert!(key.seal(Field::HeldMessage, b"x").is_err());
    let parsed = SealedField::from_bytes(held.as_bytes()).unwrap();
    assert_eq!(parsed.field(), Field::HeldMessage);
    assert_eq!(
        key.open_bound(Field::HeldMessage, &first_hold, &parsed)
            .unwrap()
            .expose_secret()
            .len(),
        big.len()
    );
    assert!(
        key.open_bound(Field::HeldMessage, &other_hold, &parsed)
            .is_err()
    );
    // Values of the retired unbound tag (5) no longer parse.
    let mut retired = sealed.as_bytes().to_vec();
    retired[2] = 5;
    assert_eq!(
        SealedField::from_bytes(&retired).unwrap_err(),
        CryptoError::Format(FormatError::UnknownField(5))
    );
}

#[test]
fn secret_size_limit() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let max = vec![b'x'; SealedField::MAX_SECRET_LEN];
    let sealed = dek.seal(&alice, Field::OAuthAccessToken, &max).unwrap();
    assert_eq!(sealed.as_bytes().len(), SealedField::MAX_LEN);
    assert!(SealedField::from_bytes(sealed.as_bytes()).is_ok());
    let too_long = vec![b'x'; SealedField::MAX_SECRET_LEN + 1];
    assert_eq!(
        dek.seal(&alice, Field::OAuthAccessToken, &too_long)
            .unwrap_err(),
        CryptoError::SecretTooLong
    );
}

#[test]
fn open_string_rejects_non_utf8() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    let sealed = dek
        .seal(&alice, Field::ImapPassword, &[0xFF, 0xFE])
        .unwrap();
    assert_eq!(
        dek.open_string(&alice, Field::ImapPassword, &sealed)
            .unwrap_err(),
        CryptoError::NotUtf8
    );
}

#[test]
fn debug_output_hides_secrets() {
    let alice = account("alice");
    let dek = DataKey::generate().unwrap();
    assert_eq!(format!("{dek:?}"), "DataKey(..)");
    let sealed = dek.seal(&alice, Field::ImapPassword, b"hunter2").unwrap();
    assert_eq!(
        format!("{sealed:?}"),
        "SealedField { field: ImapPassword, .. }"
    );
    let wrapped = Keyring::new(master(5, 1), [])
        .unwrap()
        .wrap(&alice, &dek)
        .unwrap();
    assert_eq!(
        format!("{wrapped:?}"),
        "WrappedDataKey { key_version: KeyVersion(5), .. }"
    );
}

proptest! {
    #[test]
    fn round_trip_any_secret(
        secret in proptest::collection::vec(any::<u8>(), 0..2048),
        id in proptest::collection::vec(any::<u8>(), 1..=255),
    ) {
        let account = AccountId::new(&id).unwrap();
        let keyring = Keyring::new(master(7, 9), []).unwrap();
        let dek = DataKey::generate().unwrap();
        let wrapped = keyring.wrap(&account, &dek).unwrap();
        let wrapped = WrappedDataKey::from_bytes(wrapped.as_bytes()).unwrap();
        let dek = keyring.unwrap(&account, &wrapped).unwrap();
        let sealed = dek.seal(&account, Field::SmtpPassword, &secret).unwrap();
        let sealed = SealedField::from_bytes(sealed.as_bytes()).unwrap();
        let opened = dek.open(&account, Field::SmtpPassword, &sealed).unwrap();
        prop_assert_eq!(opened.expose_secret(), secret.as_slice());
    }

    #[test]
    fn parsing_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = WrappedDataKey::from_bytes(&bytes);
        let _ = SealedField::from_bytes(&bytes);
    }
}
