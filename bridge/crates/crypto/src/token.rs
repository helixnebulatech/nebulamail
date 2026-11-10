// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use data_encoding::BASE64URL_NOPAD;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::error::CryptoError;
use crate::random;

/// Bytes of randomness in a token (256 bits).
const TOKEN_BYTES: usize = 32;
/// Length of a token as unpadded base64url text.
const TOKEN_TEXT_LEN: usize = 43;

/// An opaque bearer token: OAuth access and refresh tokens, authorization
/// codes, CSRF tokens. 32 random bytes as unpadded base64url (43 characters).
///
/// Only its [`TokenHash`] is ever stored. It has no `Display`, and `Debug`
/// prints nothing of the value, so it can't end up in a log by accident.
pub struct Token(SecretString);

impl Token {
    /// Generates a new random token.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Random`] if the operating system's RNG fails.
    pub fn generate() -> Result<Self, CryptoError> {
        let bytes = Zeroizing::new(random::bytes::<TOKEN_BYTES>()?);
        Ok(Self(SecretString::from(BASE64URL_NOPAD.encode(&*bytes))))
    }

    /// Parses a token received from a client.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidToken`] unless it is exactly 43 characters of
    /// unpadded base64url that decode to 32 bytes (in canonical form).
    pub fn parse(text: &str) -> Result<Self, CryptoError> {
        if text.len() != TOKEN_TEXT_LEN {
            return Err(CryptoError::InvalidToken);
        }
        let decoded = Zeroizing::new(
            BASE64URL_NOPAD
                .decode(text.as_bytes())
                .map_err(|_| CryptoError::InvalidToken)?,
        );
        if decoded.len() != TOKEN_BYTES {
            return Err(CryptoError::InvalidToken);
        }
        Ok(Self(SecretString::from(text)))
    }

    /// The token text, to send to the client once. Never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// The hash to store and look up.
    #[must_use]
    pub fn hash(&self) -> TokenHash {
        TokenHash(Sha256::digest(self.0.expose_secret().as_bytes()).into())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// The SHA-256 of a [`Token`]: the only form stored in the database.
///
/// A database leak gives no usable tokens, and SHA-256 is enough (no slow
/// hash needed) because tokens have 256 bits of entropy. Equality is
/// constant-time.
#[derive(Clone, Copy)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    /// Length of a hash in bytes.
    pub const LEN: usize = 32;

    /// Parses a hash read from the database.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidTokenHash`] unless it is 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        <[u8; Self::LEN]>::try_from(bytes)
            .map(Self)
            .map_err(|_| CryptoError::InvalidTokenHash)
    }

    /// The bytes to store.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    /// Whether `token` hashes to this value, in constant time.
    #[must_use]
    pub fn matches(&self, token: &Token) -> bool {
        *self == token.hash()
    }
}

impl PartialEq for TokenHash {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for TokenHash {}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A hash is not a secret, but there's no reason to print it either.
        f.write_str("TokenHash(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_43_chars_and_unique() {
        let a = Token::generate().unwrap();
        let b = Token::generate().unwrap();
        assert_eq!(a.expose().len(), 43);
        assert_ne!(a.expose(), b.expose());
        assert!(
            a.expose()
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    #[test]
    fn parse_round_trip_and_hash() {
        let token = Token::generate().unwrap();
        let parsed = Token::parse(token.expose()).unwrap();
        assert_eq!(token.hash(), parsed.hash());
        assert!(token.hash().matches(&parsed));
        let other = Token::generate().unwrap();
        assert!(!token.hash().matches(&other));
    }

    #[test]
    fn hash_is_sha256_of_the_text() {
        let token = Token::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let expected: [u8; 32] =
            Sha256::digest(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").into();
        assert_eq!(token.hash().as_bytes(), &expected);
        let stored = TokenHash::from_bytes(&expected).unwrap();
        assert_eq!(stored, token.hash());
    }

    #[test]
    fn parse_rejects_bad_tokens() {
        for bad in [
            "",
            "short",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", // 42
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", // 44
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", // padding
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+", // not url-safe
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB", // non-canonical last char
        ] {
            assert_eq!(
                Token::parse(bad).unwrap_err(),
                CryptoError::InvalidToken,
                "{bad}"
            );
        }
    }

    #[test]
    fn hash_from_bytes_checks_length() {
        assert!(TokenHash::from_bytes(&[0; 31]).is_err());
        assert!(TokenHash::from_bytes(&[0; 33]).is_err());
    }

    #[test]
    fn debug_hides_the_token() {
        let token = Token::generate().unwrap();
        assert_eq!(format!("{token:?}"), "Token(..)");
    }
}
