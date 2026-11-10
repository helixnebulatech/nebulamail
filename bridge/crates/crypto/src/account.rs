// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use crate::error::CryptoError;

/// The account a key or secret belongs to, as opaque bytes.
///
/// It goes into the AAD of every encryption, so a ciphertext of one account
/// can't be opened as another's. The store owns the real account ids; it
/// passes their stable byte form here (it must never change for an account,
/// or its secrets can no longer be opened).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccountId(Box<[u8]>);

impl AccountId {
    /// The maximum length, so it fits the one-byte length prefix in the AAD.
    pub const MAX_LEN: usize = 255;

    /// Wraps the stable byte form of an account id.
    ///
    /// # Errors
    ///
    /// [`CryptoError::AccountIdLength`] if `id` is empty or longer than
    /// [`Self::MAX_LEN`].
    pub fn new(id: &[u8]) -> Result<Self, CryptoError> {
        if id.is_empty() || id.len() > Self::MAX_LEN {
            return Err(CryptoError::AccountIdLength { max: Self::MAX_LEN });
        }
        Ok(Self(id.into()))
    }

    /// The bytes of the id.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Appends the id with a length prefix, so `ab` + `c` and `a` + `bc` give
    /// different AADs.
    pub(crate) fn write_aad(&self, aad: &mut Vec<u8>) {
        // The constructor caps the length at 255, so this never fails.
        let len = u8::try_from(self.0.len()).unwrap_or(u8::MAX);
        aad.push(len);
        aad.extend_from_slice(&self.0);
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match std::str::from_utf8(&self.0) {
            Ok(text) => f.debug_tuple("AccountId").field(&text).finish(),
            Err(_) => f.debug_tuple("AccountId").field(&self.0).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_too_long() {
        assert!(AccountId::new(b"").is_err());
        assert!(AccountId::new(&[b'a'; 256]).is_err());
        assert!(AccountId::new(&[b'a'; 255]).is_ok());
    }

    #[test]
    fn aad_is_length_prefixed() {
        let mut aad = Vec::new();
        AccountId::new(b"abc").unwrap().write_aad(&mut aad);
        assert_eq!(aad, b"\x03abc");
    }
}
