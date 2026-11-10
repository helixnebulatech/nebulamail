// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The binary format shared by [`crate::WrappedDataKey`] and
//! [`crate::SealedField`], and the AEAD calls behind both.
//!
//! ```text
//! byte 0      format version (1)
//! byte 1      kind (1 = wrapped data key, 2 = sealed field)
//! kind header wrapped data key: master key version, u16 big-endian
//!             sealed field: field tag, 1 byte
//! 24 bytes    XChaCha20 nonce (random)
//! rest        ciphertext followed by the 16-byte Poly1305 tag
//! ```
//!
//! The AAD is everything before the nonce followed by the account id (with a
//! length prefix) and, for the fields that need one, the record it belongs
//! to ([`crate::Binding`], with a 4-byte length prefix). So the format
//! version, the kind, the key version or field, the account and the record
//! are all authenticated: changing any of them makes decryption fail.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::account::AccountId;
use crate::binding::Binding;
use crate::error::{CryptoError, FormatError};
use crate::random;

/// The only format version so far.
pub(crate) const FORMAT_V1: u8 = 1;
/// `XChaCha20` nonce length (192 bits).
pub(crate) const NONCE_LEN: usize = 24;
/// Poly1305 tag length.
pub(crate) const TAG_LEN: usize = 16;
/// Length of a symmetric key (256 bits).
pub(crate) const KEY_LEN: usize = 32;

/// What a stored value holds. Part of the AAD, so one kind can never be
/// opened as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    WrappedDataKey,
    SealedField,
}

impl Kind {
    const fn tag(self) -> u8 {
        match self {
            Self::WrappedDataKey => 1,
            Self::SealedField => 2,
        }
    }
}

/// A parsed value, borrowing from the input bytes.
#[derive(Debug)]
pub(crate) struct Parts<'a> {
    /// Format version, kind and kind header: the first half of the AAD.
    pub(crate) header: &'a [u8],
    /// Only the kind header (key version or field tag).
    pub(crate) kind_header: &'a [u8],
    pub(crate) nonce: [u8; NONCE_LEN],
    /// Ciphertext and tag.
    pub(crate) ciphertext: &'a [u8],
}

/// Starts the header of a new value.
pub(crate) fn header(kind: Kind, kind_header: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + kind_header.len());
    out.push(FORMAT_V1);
    out.push(kind.tag());
    out.extend_from_slice(kind_header);
    out
}

/// Splits `bytes` into its parts, checking the version, the kind and that
/// the ciphertext length (tag included) is in `ciphertext_len`.
pub(crate) fn split(
    bytes: &[u8],
    kind: Kind,
    kind_header_len: usize,
    ciphertext_len: std::ops::RangeInclusive<usize>,
) -> Result<Parts<'_>, FormatError> {
    let (&version, rest) = bytes.split_first().ok_or(FormatError::Truncated)?;
    if version != FORMAT_V1 {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let (&found_kind, rest) = rest.split_first().ok_or(FormatError::Truncated)?;
    if found_kind != kind.tag() {
        return Err(FormatError::WrongKind);
    }
    let (kind_header, rest) = rest
        .split_at_checked(kind_header_len)
        .ok_or(FormatError::Truncated)?;
    let (nonce, ciphertext) = rest
        .split_at_checked(NONCE_LEN)
        .ok_or(FormatError::Truncated)?;
    if ciphertext.len() < *ciphertext_len.start() {
        return Err(FormatError::Truncated);
    }
    if ciphertext.len() > *ciphertext_len.end() {
        return Err(FormatError::TooLong);
    }
    let header_len = bytes
        .len()
        .saturating_sub(ciphertext.len())
        .saturating_sub(NONCE_LEN);
    let header = bytes.get(..header_len).ok_or(FormatError::Truncated)?;
    let nonce = <[u8; NONCE_LEN]>::try_from(nonce).map_err(|_| FormatError::Truncated)?;
    Ok(Parts {
        header,
        kind_header,
        nonce,
        ciphertext,
    })
}

/// Encrypts `plaintext` and returns the whole value: `header`, a fresh
/// random nonce, ciphertext and tag.
pub(crate) fn seal(
    key: &[u8; KEY_LEN],
    header: Vec<u8>,
    account: &AccountId,
    binding: Option<&Binding<'_>>,
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let nonce = random::bytes::<NONCE_LEN>()?;
    let aad = aad(&header, account, binding);
    let ciphertext = XChaCha20Poly1305::new(key.into())
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = header;
    out.reserve_exact(NONCE_LEN.saturating_add(ciphertext.len()));
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts and authenticates `parts`. The plaintext is wiped when dropped.
pub(crate) fn open(
    key: &[u8; KEY_LEN],
    parts: &Parts<'_>,
    account: &AccountId,
    binding: Option<&Binding<'_>>,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let aad = aad(parts.header, account, binding);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            &XNonce::from(parts.nonce),
            Payload {
                msg: parts.ciphertext,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Decrypt)
}

fn aad(header: &[u8], account: &AccountId, binding: Option<&Binding<'_>>) -> Vec<u8> {
    let extra = binding.map_or(0, |b| b.as_bytes().len().saturating_add(4));
    let mut aad = Vec::with_capacity(
        header
            .len()
            .saturating_add(1 + AccountId::MAX_LEN)
            .saturating_add(extra),
    );
    aad.extend_from_slice(header);
    account.write_aad(&mut aad);
    if let Some(binding) = binding {
        binding.write_aad(&mut aad);
    }
    aad
}
