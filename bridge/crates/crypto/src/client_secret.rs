// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! argon2id hashes of OAuth client secrets, kept in the configuration file.

use std::fmt;
use std::str::FromStr;

use argon2::{ARGON2ID_IDENT, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier};
use secrecy::{ExposeSecret, SecretString};

use crate::error::CryptoError;
use crate::random;

/// Upper bounds for the parameters of a hash we agree to verify, so a typo
/// in the configuration (`m=19456000`) can't make every token request use
/// gigabytes of memory.
const MAX_MEMORY_KIB: u32 = 256 * 1024;
const MAX_ITERATIONS: u32 = 16;
const MAX_PARALLELISM: u32 = 16;

/// An argon2id hash of an OAuth client secret, as a PHC string
/// (`$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`).
///
/// Parsing accepts only argon2id with sane parameters. New hashes use the
/// argon2 crate's defaults (the OWASP recommendation: 19 MiB, 2 passes, one
/// lane). A new secret can be made with [`crate::Token::generate`].
#[derive(Clone, PartialEq, Eq)]
pub struct ClientSecretHash(String);

impl ClientSecretHash {
    /// Hashes a new client secret with a random salt.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Random`] or [`CryptoError::PasswordHash`].
    pub fn hash(secret: &SecretString) -> Result<Self, CryptoError> {
        let salt = random::bytes::<16>()?;
        let hash = Argon2::default()
            .hash_password_with_salt(secret.expose_secret().as_bytes(), &salt)
            .map_err(|_| CryptoError::PasswordHash)?;
        Ok(Self(hash.to_string()))
    }

    /// Checks `secret` against this hash. The comparison of the outputs is
    /// constant-time.
    ///
    /// # Errors
    ///
    /// [`CryptoError::ClientSecretMismatch`] if it doesn't match.
    pub fn verify(&self, secret: &SecretString) -> Result<(), CryptoError> {
        let hash = PasswordHash::new(&self.0).map_err(|_| CryptoError::InvalidClientSecretHash)?;
        Argon2::default()
            .verify_password(secret.expose_secret().as_bytes(), &hash)
            .map_err(|_| CryptoError::ClientSecretMismatch)
    }

    /// The PHC string, to write in the configuration.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ClientSecretHash {
    type Err = CryptoError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let hash = PasswordHash::new(text).map_err(|_| CryptoError::InvalidClientSecretHash)?;
        if hash.algorithm != ARGON2ID_IDENT || hash.salt.is_none() || hash.hash.is_none() {
            return Err(CryptoError::InvalidClientSecretHash);
        }
        let params = Params::try_from(&hash).map_err(|_| CryptoError::InvalidClientSecretHash)?;
        if params.m_cost() > MAX_MEMORY_KIB
            || params.t_cost() > MAX_ITERATIONS
            || params.p_cost() > MAX_PARALLELISM
        {
            return Err(CryptoError::InvalidClientSecretHash);
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Debug for ClientSecretHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The hash isn't the secret, but offline guessing needs only the hash.
        f.write_str("ClientSecretHash(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let secret = SecretString::from("correct horse battery staple");
        let hash = ClientSecretHash::hash(&secret).unwrap();
        assert!(hash.as_str().starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert_eq!(hash.verify(&secret), Ok(()));
        assert_eq!(
            hash.verify(&SecretString::from("wrong")),
            Err(CryptoError::ClientSecretMismatch)
        );
    }

    #[test]
    fn parse_round_trip() {
        let secret = SecretString::from("s3cret");
        let hash = ClientSecretHash::hash(&secret).unwrap();
        let parsed: ClientSecretHash = format!("{}\n", hash.as_str()).parse().unwrap();
        assert_eq!(parsed, hash);
        assert_eq!(parsed.verify(&secret), Ok(()));
    }

    #[test]
    fn parse_rejects_other_algorithms_and_huge_params() {
        for bad in [
            "",
            "plain",
            "$argon2i$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g",
            "$argon2id$v=19$m=19456000,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g",
            "$argon2id$v=19$m=19456,t=200,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g",
            "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0",
            "$pbkdf2-sha256$i=1000$c2FsdA$aGFzaA",
        ] {
            assert!(bad.parse::<ClientSecretHash>().is_err(), "{bad}");
        }
    }
}
