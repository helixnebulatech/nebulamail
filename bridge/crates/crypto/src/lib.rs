// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Credential encryption and tokens.
//!
//! - **Two key levels.** A [`MasterKey`] (KEK) comes from a file or an
//!   environment variable, never from the database. Each account has its own
//!   random [`DataKey`] (DEK), stored only wrapped by the KEK
//!   ([`WrappedDataKey`]). Rotating the KEK only re-wraps the DEKs; the
//!   secrets sealed with a DEK stay as they are.
//! - **Sealed fields.** Secrets such as the IMAP password are encrypted with
//!   the DEK into a [`SealedField`]. The ciphertext is bound to the account
//!   and to the [`Field`], so it can't be copied to another account or column;
//!   values an account has many of (calendars and contacts) are also bound
//!   to their record ([`Binding`]), so one can't be swapped for another.
//! - **Tokens.** Opaque 256-bit [`Token`]s for OAuth; the database keeps only
//!   their [`TokenHash`]. PKCE S256 checks with [`verify_s256`], and argon2id
//!   hashes for OAuth client secrets ([`ClientSecretHash`]).
//!
//! Encryption is XChaCha20-Poly1305 with a random 192-bit nonce: random
//! nonces of that size never collide in practice.
//!
//! ```
//! use secrecy::{ExposeSecret, SecretString};
//! use nebula_crypto::{AccountId, DataKey, Field, Keyring, KeyVersion, MasterKey};
//!
//! # fn main() -> Result<(), nebula_crypto::CryptoError> {
//! let text = SecretString::from("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
//! let kek = MasterKey::from_base64(KeyVersion::new(1)?, &text)?;
//! let keyring = Keyring::new(kek, [])?;
//! let account = AccountId::new(b"account-42")?;
//!
//! // When the account is created: a new DEK, stored wrapped.
//! let dek = DataKey::generate()?;
//! let wrapped = keyring.wrap(&account, &dek)?;
//!
//! // Later: unwrap the DEK and seal or open a secret.
//! let dek = keyring.unwrap(&account, &wrapped)?;
//! let sealed = dek.seal(&account, Field::ImapPassword, b"hunter2")?;
//! let password = dek.open_string(&account, Field::ImapPassword, &sealed)?;
//! assert_eq!(password.expose_secret(), "hunter2");
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod account;
mod account_key;
mod binding;
mod client_secret;
mod data_key;
mod error;
mod format;
mod master_key;
mod pkce;
mod random;
mod sealed;
mod token;

#[cfg(test)]
mod tests;

pub use account::AccountId;
pub use account_key::AccountKey;
pub use binding::Binding;
pub use client_secret::ClientSecretHash;
pub use data_key::{DataKey, WrappedDataKey};
pub use error::{CryptoError, FormatError};
pub use master_key::{KeyVersion, Keyring, MasterKey};
pub use pkce::{CodeChallenge, CodeVerifier, verify_s256};
pub use sealed::{Field, SealedField};
pub use token::{Token, TokenHash};
