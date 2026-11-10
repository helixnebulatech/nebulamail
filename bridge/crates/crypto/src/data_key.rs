// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use secrecy::{ExposeSecret, ExposeSecretMut, SecretBox, SecretSlice, SecretString};

use crate::account::AccountId;
use crate::binding::Binding;
use crate::error::{CryptoError, FormatError};
use crate::format::{self, KEY_LEN, Kind, TAG_LEN};
use crate::master_key::{KeyVersion, MasterKey};
use crate::random;
use crate::sealed::{Field, SealedField};

/// A per-account data key (DEK): 256 random bits that encrypt the account's
/// secrets. It is only stored wrapped by a master key ([`WrappedDataKey`]).
///
/// The bytes are wiped from memory when it is dropped, and `Debug` shows
/// nothing.
pub struct DataKey(SecretBox<[u8; KEY_LEN]>);

impl DataKey {
    /// Generates a new random data key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Random`] if the operating system's RNG fails.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut key = SecretBox::new(Box::new([0u8; KEY_LEN]));
        random::fill(key.expose_secret_mut())?;
        Ok(Self(key))
    }

    /// A copy of the key (also wiped on drop).
    pub(crate) fn duplicate(&self) -> Self {
        Self(SecretBox::new(Box::new(*self.0.expose_secret())))
    }

    /// Encrypts `secret` as `field` of `account`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::SecretTooLong`] above
    /// [`Field::max_len`], [`CryptoError::WrongBinding`] for a field that
    /// needs a [`Binding`] (use [`Self::seal_bound`]),
    /// [`CryptoError::Random`] if no nonce could be generated.
    pub fn seal(
        &self,
        account: &AccountId,
        field: Field,
        secret: &[u8],
    ) -> Result<SealedField, CryptoError> {
        SealedField::seal(self.bytes(), account, field, None, secret)
    }

    /// Encrypts `data` as `field` of `account`, bound to one record of
    /// the account (for the fields that [need one](Field::needs_binding)).
    ///
    /// # Errors
    ///
    /// As [`Self::seal`]; [`CryptoError::WrongBinding`] for a field that
    /// takes no binding.
    pub fn seal_bound(
        &self,
        account: &AccountId,
        field: Field,
        binding: &Binding<'_>,
        data: &[u8],
    ) -> Result<SealedField, CryptoError> {
        SealedField::seal(self.bytes(), account, field, Some(binding), data)
    }

    /// Decrypts a sealed `field` of `account`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::FieldMismatch`] if `sealed` holds another field,
    /// [`CryptoError::WrongBinding`] for a bound field (use
    /// [`Self::open_bound`]), and [`CryptoError::Decrypt`] if the key, the
    /// account or the data don't match.
    pub fn open(
        &self,
        account: &AccountId,
        field: Field,
        sealed: &SealedField,
    ) -> Result<SecretSlice<u8>, CryptoError> {
        sealed.open(self.bytes(), account, field, None)
    }

    /// Decrypts a sealed `field` of `account` bound to `binding`.
    ///
    /// # Errors
    ///
    /// As [`Self::open`]; [`CryptoError::Decrypt`] also if it was sealed
    /// for another record.
    pub fn open_bound(
        &self,
        account: &AccountId,
        field: Field,
        binding: &Binding<'_>,
        sealed: &SealedField,
    ) -> Result<SecretSlice<u8>, CryptoError> {
        sealed.open(self.bytes(), account, field, Some(binding))
    }

    /// Like [`Self::open`], for secrets that are text (passwords, tokens).
    ///
    /// # Errors
    ///
    /// As [`Self::open`], plus [`CryptoError::NotUtf8`].
    pub fn open_string(
        &self,
        account: &AccountId,
        field: Field,
        sealed: &SealedField,
    ) -> Result<SecretString, CryptoError> {
        let bytes = self.open(account, field, sealed)?;
        let text = std::str::from_utf8(bytes.expose_secret()).map_err(|_| CryptoError::NotUtf8)?;
        Ok(SecretString::from(text))
    }

    pub(crate) fn bytes(&self) -> &[u8; KEY_LEN] {
        self.0.expose_secret()
    }
}

impl fmt::Debug for DataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DataKey(..)")
    }
}

/// A [`DataKey`] encrypted with a master key, ready to store.
///
/// Its bytes are a self-describing value (see the crate's format notes):
/// format version, master key version, nonce, and the encrypted key with its
/// tag. Always exactly [`Self::LEN`] bytes. The AAD binds it to the account
/// and to the master key version.
#[derive(Clone, PartialEq, Eq)]
pub struct WrappedDataKey {
    /// Parsed from `bytes` once, so it is always valid.
    version: KeyVersion,
    bytes: Box<[u8]>,
}

impl WrappedDataKey {
    /// Length of every wrapped data key.
    pub const LEN: usize = 2 + KEY_VERSION_LEN + format::NONCE_LEN + KEY_LEN + TAG_LEN;

    /// Parses a stored wrapped data key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Format`] if the bytes aren't a wrapped data key.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let parts = Self::split(bytes)?;
        let version = KeyVersion::new(parse_key_version(parts.kind_header)?)?;
        Ok(Self {
            version,
            bytes: bytes.into(),
        })
    }

    /// The bytes to store.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The version of the master key that wrapped this key.
    #[must_use]
    pub const fn key_version(&self) -> KeyVersion {
        self.version
    }

    pub(crate) fn wrap(
        master: &MasterKey,
        account: &AccountId,
        data_key: &DataKey,
    ) -> Result<Self, CryptoError> {
        let header = format::header(Kind::WrappedDataKey, &master.version().get().to_be_bytes());
        let bytes = format::seal(master.bytes(), header, account, None, data_key.bytes())?;
        Ok(Self {
            version: master.version(),
            bytes: bytes.into(),
        })
    }

    pub(crate) fn unwrap_with(
        &self,
        master: &MasterKey,
        account: &AccountId,
    ) -> Result<DataKey, CryptoError> {
        let parts = Self::split(&self.bytes)?;
        let plaintext = format::open(master.bytes(), &parts, account, None)?;
        if plaintext.len() != KEY_LEN {
            return Err(CryptoError::Decrypt);
        }
        let mut key = SecretBox::new(Box::new([0u8; KEY_LEN]));
        key.expose_secret_mut().copy_from_slice(&plaintext);
        Ok(DataKey(key))
    }

    fn split(bytes: &[u8]) -> Result<format::Parts<'_>, FormatError> {
        let ciphertext_len = KEY_LEN + TAG_LEN;
        format::split(
            bytes,
            Kind::WrappedDataKey,
            KEY_VERSION_LEN,
            ciphertext_len..=ciphertext_len,
        )
    }
}

impl fmt::Debug for WrappedDataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrappedDataKey")
            .field("key_version", &self.key_version())
            .finish_non_exhaustive()
    }
}

const KEY_VERSION_LEN: usize = 2;

fn parse_key_version(kind_header: &[u8]) -> Result<u16, FormatError> {
    let bytes =
        <[u8; KEY_VERSION_LEN]>::try_from(kind_header).map_err(|_| FormatError::Truncated)?;
    Ok(u16::from_be_bytes(bytes))
}
