// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The only source of randomness: the operating system's CSPRNG.

use crate::error::CryptoError;

/// Fills `dest` with random bytes from the operating system.
pub(crate) fn fill(dest: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(dest).map_err(|_| CryptoError::Random)
}

/// Returns `N` random bytes.
pub(crate) fn bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut out = [0u8; N];
    fill(&mut out)?;
    Ok(out)
}
