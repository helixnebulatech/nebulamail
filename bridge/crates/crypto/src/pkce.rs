// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! PKCE (RFC 7636) with the `S256` method only: `plain` is not accepted.

use std::fmt;

use data_encoding::BASE64URL_NOPAD;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::CryptoError;

/// A PKCE `code_verifier` sent by the client to the token endpoint:
/// 43 to 128 characters of `A-Z a-z 0-9 - . _ ~` (RFC 7636 §4.1).
///
/// It is a secret until the code is used, so `Debug` hides it.
pub struct CodeVerifier(SecretString);

impl CodeVerifier {
    const MIN_LEN: usize = 43;
    const MAX_LEN: usize = 128;

    /// Parses a code verifier.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidCodeVerifier`] if the length or a character is
    /// not allowed.
    pub fn parse(text: &str) -> Result<Self, CryptoError> {
        let length_ok = (Self::MIN_LEN..=Self::MAX_LEN).contains(&text.len());
        if !length_ok || !text.bytes().all(is_unreserved) {
            return Err(CryptoError::InvalidCodeVerifier);
        }
        Ok(Self(SecretString::from(text)))
    }

    /// The S256 challenge of this verifier: `BASE64URL(SHA256(verifier))`.
    #[must_use]
    pub fn challenge(&self) -> CodeChallenge {
        CodeChallenge(Sha256::digest(self.0.expose_secret().as_bytes()).into())
    }
}

impl fmt::Debug for CodeVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodeVerifier(..)")
    }
}

/// A PKCE `code_challenge` with method `S256`, as sent to the authorization
/// endpoint: 43 characters of unpadded base64url (a SHA-256 digest).
#[derive(Clone, Copy)]
pub struct CodeChallenge([u8; 32]);

impl CodeChallenge {
    /// Parses an S256 code challenge.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidCodeChallenge`] unless it is the base64url form
    /// of 32 bytes.
    pub fn parse(text: &str) -> Result<Self, CryptoError> {
        if text.len() != 43 {
            return Err(CryptoError::InvalidCodeChallenge);
        }
        let bytes = BASE64URL_NOPAD
            .decode(text.as_bytes())
            .map_err(|_| CryptoError::InvalidCodeChallenge)?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| CryptoError::InvalidCodeChallenge)
    }

    /// The challenge as text, to store with the authorization code.
    #[must_use]
    pub fn to_text(&self) -> String {
        BASE64URL_NOPAD.encode(&self.0)
    }
}

impl fmt::Debug for CodeChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CodeChallenge")
            .field(&self.to_text())
            .finish()
    }
}

/// Checks a code verifier against the S256 challenge stored with the
/// authorization code, in constant time.
///
/// # Errors
///
/// [`CryptoError::PkceMismatch`] if they don't match.
pub fn verify_s256(verifier: &CodeVerifier, challenge: &CodeChallenge) -> Result<(), CryptoError> {
    if bool::from(verifier.challenge().0.ct_eq(&challenge.0)) {
        Ok(())
    } else {
        Err(CryptoError::PkceMismatch)
    }
}

/// RFC 3986 unreserved characters, the code verifier alphabet.
const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// RFC 7636 appendix B.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn rfc_7636_appendix_b() {
        let verifier = CodeVerifier::parse(RFC_VERIFIER).unwrap();
        let challenge = CodeChallenge::parse(RFC_CHALLENGE).unwrap();
        assert_eq!(verifier.challenge().to_text(), RFC_CHALLENGE);
        assert_eq!(verify_s256(&verifier, &challenge), Ok(()));
    }

    #[test]
    fn wrong_verifier_fails() {
        let verifier = CodeVerifier::parse(&"a".repeat(43)).unwrap();
        let challenge = CodeChallenge::parse(RFC_CHALLENGE).unwrap();
        assert_eq!(
            verify_s256(&verifier, &challenge),
            Err(CryptoError::PkceMismatch)
        );
    }

    #[test]
    fn verifier_grammar() {
        assert!(CodeVerifier::parse(&"a".repeat(42)).is_err());
        assert!(CodeVerifier::parse(&"a".repeat(43)).is_ok());
        assert!(CodeVerifier::parse(&"a".repeat(128)).is_ok());
        assert!(CodeVerifier::parse(&"a".repeat(129)).is_err());
        assert!(CodeVerifier::parse(&format!("{}-._~", "Z9".repeat(20))).is_ok());
        for bad in ['+', '/', '=', ' ', '\n', 'ñ', '%'] {
            let text = format!("{}{bad}", "a".repeat(43));
            assert!(CodeVerifier::parse(&text).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn challenge_grammar() {
        assert!(CodeChallenge::parse("").is_err());
        assert!(CodeChallenge::parse(RFC_CHALLENGE.get(..42).unwrap()).is_err());
        assert!(CodeChallenge::parse(&format!("{RFC_CHALLENGE}=")).is_err());
        assert!(CodeChallenge::parse(&RFC_CHALLENGE.replace('-', "+")).is_err());
    }

    proptest! {
        #[test]
        fn any_valid_verifier_matches_its_own_challenge(text in "[A-Za-z0-9._~-]{43,128}") {
            let verifier = CodeVerifier::parse(&text).unwrap();
            let challenge = CodeChallenge::parse(&verifier.challenge().to_text()).unwrap();
            prop_assert!(verify_s256(&verifier, &challenge).is_ok());
        }
    }
}
