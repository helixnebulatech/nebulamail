// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use crate::error::CryptoError;

/// The record a sealed value belongs to, inside its account: part of the
/// AAD of the fields that need one ([`crate::Field::needs_binding`]), so
/// someone who can write to the database can't swap the ciphertext of one
/// record for another's of the same account and field (one contact's
/// vCard for another's, say). The caller builds it from what identifies
/// the record and never changes for it (for a DAV resource: its
/// collection's id and its href).
///
/// Opaque bytes, up to [`Binding::MAX_LEN`]. `Debug` hides them: they may
/// name things (an href).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Binding<'a>(&'a [u8]);

impl<'a> Binding<'a> {
    /// The longest binding: 64 KiB, far above an href.
    pub const MAX_LEN: usize = 64 * 1024;

    /// A binding from its bytes.
    ///
    /// # Errors
    ///
    /// [`CryptoError::BindingLength`] if `bytes` is longer than
    /// [`Self::MAX_LEN`].
    pub const fn new(bytes: &'a [u8]) -> Result<Self, CryptoError> {
        if bytes.len() > Self::MAX_LEN {
            return Err(CryptoError::BindingLength { max: Self::MAX_LEN });
        }
        Ok(Self(bytes))
    }

    /// The bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.0
    }

    /// Appends the bytes with a 4-byte big-endian length prefix, so the
    /// end of the account id and the start of the binding can't be
    /// confused.
    pub(crate) fn write_aad(&self, aad: &mut Vec<u8>) {
        // The constructor caps the length at 64 KiB, so this never fails.
        let len = u32::try_from(self.0.len()).unwrap_or(u32::MAX);
        aad.extend_from_slice(&len.to_be_bytes());
        aad.extend_from_slice(self.0);
    }
}

impl fmt::Debug for Binding<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Binding({} bytes)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_prefix() {
        assert!(Binding::new(&vec![0; Binding::MAX_LEN]).is_ok());
        assert_eq!(
            Binding::new(&vec![0; Binding::MAX_LEN + 1]).unwrap_err(),
            CryptoError::BindingLength {
                max: Binding::MAX_LEN
            }
        );
        let mut aad = Vec::new();
        Binding::new(b"ab").unwrap().write_aad(&mut aad);
        assert_eq!(aad, b"\0\0\0\x02ab");
        assert_eq!(
            format!("{:?}", Binding::new(b"secret").unwrap()),
            "Binding(6 bytes)"
        );
    }
}
