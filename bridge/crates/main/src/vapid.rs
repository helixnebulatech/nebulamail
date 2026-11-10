// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The bridge's VAPID key (RFC 8292): generated the first
//! time Web Push is enabled and kept in the database, sealed with a data
//! key of its own that the master key wraps (as an account's). A master
//! key rotation re-wraps it with the accounts' keys
//! (`rewrap-keys`, or at start); `rotate-vapid-key` replaces it.

use anyhow::Context as _;
use nebula_crypto::{
    AccountId as CryptoAccountId, DataKey, Field, Keyring, SealedField, WrappedDataKey,
};
use nebula_store::{Store, SystemSecret, UnixTime};
use nebula_webpush::VapidKey;
use secrecy::ExposeSecret as _;

/// Its row in `system_secrets`.
const NAME: &str = "vapid";

/// The id its data key and value are bound to (never an account's: those
/// start with `login:`).
fn crypto_id() -> anyhow::Result<CryptoAccountId> {
    Ok(CryptoAccountId::new(b"system:vapid")?)
}

fn open(keyring: &Keyring, secret: &SystemSecret) -> anyhow::Result<VapidKey> {
    let id = crypto_id()?;
    let data_key = keyring
        .unwrap(&id, &WrappedDataKey::from_bytes(&secret.wrapped_dek)?)
        .context("the VAPID key's data key does not unwrap with the configured master keys")?;
    let private = data_key.open(
        &id,
        Field::VapidKey,
        &SealedField::from_bytes(&secret.sealed)?,
    )?;
    Ok(VapidKey::from_private_bytes(private.expose_secret())?)
}

/// The stored key, or a new one (stored) the first time.
pub(crate) async fn load_or_create(store: &Store, keyring: &Keyring) -> anyhow::Result<VapidKey> {
    if let Some(secret) = store.system_secret(NAME).await? {
        return open(keyring, &secret);
    }
    let id = crypto_id()?;
    let key = VapidKey::generate()?;
    let data_key = DataKey::generate()?;
    let sealed = data_key.seal(&id, Field::VapidKey, &key.private_bytes()?)?;
    let wrapped = keyring.wrap(&id, &data_key)?;
    let stored = store
        .put_system_secret_if_absent(
            NAME,
            SystemSecret {
                wrapped_dek: wrapped.as_bytes().to_vec(),
                sealed: sealed.as_bytes().to_vec(),
            },
            UnixTime::now()?,
        )
        .await?;
    tracing::info!("web push: new VAPID key generated");
    // Another process may have stored one first: use whatever is stored.
    open(keyring, &stored)
}

/// Re-wraps the key's data key with the current master key. `Ok(true)`
/// if it was re-wrapped; `Err` if it can't be unwrapped (its master key
/// is gone: rotate the VAPID key).
pub(crate) async fn rewrap(store: &Store, keyring: &Keyring) -> anyhow::Result<bool> {
    let Some(secret) = store.system_secret(NAME).await? else {
        return Ok(false);
    };
    let wrapped = WrappedDataKey::from_bytes(&secret.wrapped_dek)?;
    match keyring.rewrap(&crypto_id()?, &wrapped)? {
        None => Ok(false),
        Some(new) => Ok(store
            .replace_system_wrapped_key(NAME, &secret.wrapped_dek, new.as_bytes())
            .await?),
    }
}

/// Deletes the key and every subscription (they belong to it); the next
/// start makes a new one. Returns whether there was a key and how many
/// subscriptions went.
pub(crate) async fn rotate(store: &Store) -> anyhow::Result<(bool, usize)> {
    let existed = store.delete_system_secret(NAME).await?;
    let subscriptions = store.delete_all_push_subscriptions().await?;
    Ok((existed, subscriptions))
}

#[cfg(test)]
mod tests {
    use nebula_crypto::{KeyVersion, MasterKey};

    use super::*;

    fn keyring(byte: u8, older: Vec<MasterKey>) -> Keyring {
        let version = KeyVersion::new(u16::from(byte)).unwrap();
        Keyring::new(MasterKey::from_bytes(version, &[byte; 32]).unwrap(), older).unwrap()
    }

    #[tokio::test]
    async fn generated_once_rewrapped_and_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("b.db")).await.unwrap();
        let first = keyring(1, vec![]);
        let key = load_or_create(&store, &first).await.unwrap();
        let again = load_or_create(&store, &first).await.unwrap();
        assert_eq!(key.public_key(), again.public_key());
        // Nothing stored in clear.
        let row = store.system_secret(NAME).await.unwrap().unwrap();
        let private = key.private_bytes().unwrap();
        assert!(!row.sealed.windows(32).any(|w| w == private.as_slice()));

        // A new master key (the old one still configured): re-wrapped once.
        let old = MasterKey::from_bytes(KeyVersion::new(1).unwrap(), &[1; 32]).unwrap();
        let second = keyring(2, vec![old]);
        assert!(rewrap(&store, &second).await.unwrap());
        assert!(!rewrap(&store, &second).await.unwrap());
        let only_new = keyring(2, vec![]);
        assert_eq!(
            load_or_create(&store, &only_new)
                .await
                .unwrap()
                .public_key(),
            key.public_key()
        );
        // A master key that never wrapped it: an error, not a new key.
        assert!(load_or_create(&store, &keyring(3, vec![])).await.is_err());

        let (existed, _) = rotate(&store).await.unwrap();
        assert!(existed);
        let fresh = load_or_create(&store, &only_new).await.unwrap();
        assert_ne!(fresh.public_key(), key.public_key());
    }
}
