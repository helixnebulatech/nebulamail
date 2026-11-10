// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use data_encoding::BASE64;
use secrecy::{ExposeSecret, ExposeSecretMut, SecretBox, SecretString};
use zeroize::Zeroizing;

use crate::account::AccountId;
use crate::data_key::{DataKey, WrappedDataKey};
use crate::error::CryptoError;
use crate::format::KEY_LEN;

/// The version of a master key, so keys can be rotated: every wrapped data
/// key records the version of the master key that wrapped it.
///
/// Versions start at 1; 0 is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyVersion(u16);

impl KeyVersion {
    /// Creates a key version.
    ///
    /// # Errors
    ///
    /// [`CryptoError::KeyVersionZero`] for 0.
    pub const fn new(version: u16) -> Result<Self, CryptoError> {
        if version == 0 {
            return Err(CryptoError::KeyVersionZero);
        }
        Ok(Self(version))
    }

    /// The version number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for KeyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The master key (KEK): 32 bytes that only wrap and unwrap data keys.
///
/// It comes from a file or an environment variable as base64 text, never
/// from the database. The bytes are wiped from memory when it is dropped,
/// and `Debug` shows only the version.
pub struct MasterKey {
    version: KeyVersion,
    key: SecretBox<[u8; KEY_LEN]>,
}

impl MasterKey {
    /// Parses a master key from standard base64 text (padding required).
    /// Surrounding whitespace, such as the newline at the end of a key file,
    /// is ignored.
    ///
    /// A new key can be made with `openssl rand -base64 32` or
    /// `head -c 32 /dev/urandom | base64`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::MasterKeyEncoding`] if the text isn't base64, and
    /// [`CryptoError::MasterKeyLength`] if it isn't 32 bytes long.
    pub fn from_base64(version: KeyVersion, text: &SecretString) -> Result<Self, CryptoError> {
        let decoded = Zeroizing::new(
            BASE64
                .decode(text.expose_secret().trim().as_bytes())
                .map_err(|_| CryptoError::MasterKeyEncoding)?,
        );
        Self::from_bytes(version, &decoded)
    }

    /// Creates a master key from raw bytes.
    ///
    /// # Errors
    ///
    /// [`CryptoError::MasterKeyLength`] if `bytes` isn't 32 bytes long.
    pub fn from_bytes(version: KeyVersion, bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != KEY_LEN {
            return Err(CryptoError::MasterKeyLength {
                actual: bytes.len(),
            });
        }
        let mut key = SecretBox::new(Box::new([0u8; KEY_LEN]));
        key.expose_secret_mut().copy_from_slice(bytes);
        Ok(Self { version, key })
    }

    /// The version of this key.
    #[must_use]
    pub const fn version(&self) -> KeyVersion {
        self.version
    }

    pub(crate) fn bytes(&self) -> &[u8; KEY_LEN] {
        self.key.expose_secret()
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MasterKey")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// The configured master keys: one current key that wraps new data keys,
/// and older ones that can still unwrap what they wrapped.
///
/// To rotate: add a new key with a higher version as current, keep the old
/// one as previous, re-wrap every data key for which
/// [`Keyring::needs_rewrap`] is true, then remove the old key.
#[derive(Debug)]
pub struct Keyring {
    current: MasterKey,
    previous: Vec<MasterKey>,
}

impl Keyring {
    /// Creates a keyring.
    ///
    /// # Errors
    ///
    /// [`CryptoError::DuplicateKeyVersion`] if two keys share a version.
    pub fn new(
        current: MasterKey,
        previous: impl IntoIterator<Item = MasterKey>,
    ) -> Result<Self, CryptoError> {
        let previous: Vec<MasterKey> = previous.into_iter().collect();
        let mut versions: Vec<KeyVersion> = previous.iter().map(MasterKey::version).collect();
        versions.push(current.version);
        versions.sort_unstable();
        if let Some(pair) = versions.windows(2).find(|pair| pair.first() == pair.last()) {
            let version = pair.first().copied().unwrap_or(current.version);
            return Err(CryptoError::DuplicateKeyVersion(version));
        }
        Ok(Self { current, previous })
    }

    /// The version used to wrap new data keys.
    #[must_use]
    pub const fn current_version(&self) -> KeyVersion {
        self.current.version
    }

    /// Wraps `data_key` for `account` with the current master key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Random`] if no nonce could be generated.
    pub fn wrap(
        &self,
        account: &AccountId,
        data_key: &DataKey,
    ) -> Result<WrappedDataKey, CryptoError> {
        WrappedDataKey::wrap(&self.current, account, data_key)
    }

    /// Unwraps a data key with the master key of the version it records.
    ///
    /// # Errors
    ///
    /// [`CryptoError::UnknownKeyVersion`] if that key isn't configured, and
    /// [`CryptoError::Decrypt`] if the key, the account or the data don't
    /// match.
    pub fn unwrap(
        &self,
        account: &AccountId,
        wrapped: &WrappedDataKey,
    ) -> Result<DataKey, CryptoError> {
        let version = wrapped.key_version();
        let key = self
            .find(version)
            .ok_or(CryptoError::UnknownKeyVersion(version))?;
        wrapped.unwrap_with(key, account)
    }

    /// Whether `wrapped` was made with an older master key and should be
    /// wrapped again with the current one.
    #[must_use]
    pub fn needs_rewrap(&self, wrapped: &WrappedDataKey) -> bool {
        wrapped.key_version() != self.current.version
    }

    /// Wraps the data key again with the current master key, if an older
    /// one wrapped it. `None` if it is already current. The data key itself
    /// doesn't change, so the secrets it sealed stay readable.
    ///
    /// # Errors
    ///
    /// As [`Self::unwrap`] (the old key must still be configured) and
    /// [`Self::wrap`].
    pub fn rewrap(
        &self,
        account: &AccountId,
        wrapped: &WrappedDataKey,
    ) -> Result<Option<WrappedDataKey>, CryptoError> {
        if !self.needs_rewrap(wrapped) {
            return Ok(None);
        }
        let data_key = self.unwrap(account, wrapped)?;
        self.wrap(account, &data_key).map(Some)
    }

    /// The versions of the older keys, which can only unwrap.
    #[must_use]
    pub fn previous_versions(&self) -> Vec<KeyVersion> {
        self.previous.iter().map(MasterKey::version).collect()
    }

    fn find(&self, version: KeyVersion) -> Option<&MasterKey> {
        std::iter::once(&self.current)
            .chain(&self.previous)
            .find(|key| key.version == version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(n: u16) -> KeyVersion {
        KeyVersion::new(n).unwrap()
    }

    #[test]
    fn version_zero_is_rejected() {
        assert_eq!(KeyVersion::new(0), Err(CryptoError::KeyVersionZero));
    }

    #[test]
    fn parses_base64_with_trailing_newline() {
        let text = SecretString::from(format!("{}\n", BASE64.encode(&[7u8; 32])));
        let key = MasterKey::from_base64(version(1), &text).unwrap();
        assert_eq!(key.bytes(), &[7u8; 32]);
    }

    #[test]
    fn rejects_wrong_lengths_and_bad_base64() {
        for len in [0usize, 16, 31, 33, 64] {
            let text = SecretString::from(BASE64.encode(&vec![1u8; len]));
            assert_eq!(
                MasterKey::from_base64(version(1), &text).unwrap_err(),
                CryptoError::MasterKeyLength { actual: len }
            );
        }
        let text = SecretString::from("not base64!");
        assert_eq!(
            MasterKey::from_base64(version(1), &text).unwrap_err(),
            CryptoError::MasterKeyEncoding
        );
    }

    #[test]
    fn debug_hides_the_key() {
        let key = MasterKey::from_bytes(version(3), &[0xAB; 32]).unwrap();
        let debug = format!("{key:?}");
        assert!(debug.contains("version"));
        assert!(!debug.contains("171"), "{debug}");
        assert!(!debug.to_lowercase().contains("ab, "), "{debug}");
    }

    #[test]
    fn duplicate_versions_are_rejected() {
        let a = MasterKey::from_bytes(version(1), &[1; 32]).unwrap();
        let b = MasterKey::from_bytes(version(2), &[2; 32]).unwrap();
        let c = MasterKey::from_bytes(version(1), &[3; 32]).unwrap();
        assert_eq!(
            Keyring::new(a, [b, c]).unwrap_err(),
            CryptoError::DuplicateKeyVersion(version(1))
        );
    }
}
