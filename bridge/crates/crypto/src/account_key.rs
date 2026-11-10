// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use secrecy::SecretSlice;

use crate::account::AccountId;
use crate::binding::Binding;
use crate::data_key::DataKey;
use crate::error::CryptoError;
use crate::sealed::{Field, SealedField};

/// An account's unwrapped [`DataKey`] together with the id it is bound to:
/// what a service that keeps data of a running account needs (the
/// groupware layer seals its local copy of calendars and contacts with it).
///
/// It only exists while the account runs, in memory; the key bytes are
/// wiped when it is dropped, and `Debug` shows nothing.
pub struct AccountKey {
    account: AccountId,
    key: DataKey,
}

impl AccountKey {
    /// Pairs a data key with its account.
    #[must_use]
    pub const fn new(account: AccountId, key: DataKey) -> Self {
        Self { account, key }
    }

    /// A second copy of the key, for another service of the same running
    /// account (Web Push seals its subscriptions with it). Wiped on drop,
    /// like the first.
    #[must_use]
    pub fn duplicate(&self) -> Self {
        Self {
            account: self.account.clone(),
            key: self.key.duplicate(),
        }
    }

    /// The account the key is bound to.
    #[must_use]
    pub const fn account(&self) -> &AccountId {
        &self.account
    }

    /// Encrypts `data` as `field` of this account.
    ///
    /// # Errors
    ///
    /// As [`DataKey::seal`].
    pub fn seal(&self, field: Field, data: &[u8]) -> Result<SealedField, CryptoError> {
        self.key.seal(&self.account, field, data)
    }

    /// Encrypts `data` as `field` of one record of this account.
    ///
    /// # Errors
    ///
    /// As [`DataKey::seal_bound`].
    pub fn seal_bound(
        &self,
        field: Field,
        binding: &Binding<'_>,
        data: &[u8],
    ) -> Result<SealedField, CryptoError> {
        self.key.seal_bound(&self.account, field, binding, data)
    }

    /// Decrypts a sealed `field` of this account.
    ///
    /// # Errors
    ///
    /// As [`DataKey::open`].
    pub fn open(&self, field: Field, sealed: &SealedField) -> Result<SecretSlice<u8>, CryptoError> {
        self.key.open(&self.account, field, sealed)
    }

    /// Decrypts a sealed `field` of one record of this account.
    ///
    /// # Errors
    ///
    /// As [`DataKey::open_bound`].
    pub fn open_bound(
        &self,
        field: Field,
        binding: &Binding<'_>,
        sealed: &SealedField,
    ) -> Result<SecretSlice<u8>, CryptoError> {
        self.key.open_bound(&self.account, field, binding, sealed)
    }
}

impl fmt::Debug for AccountKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountKey(..)")
    }
}
