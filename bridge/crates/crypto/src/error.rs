// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use crate::master_key::KeyVersion;
use crate::sealed::Field;

/// Everything that can go wrong in this crate.
///
/// No variant carries key material, plaintext or tokens, so errors are safe
/// to log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CryptoError {
    /// The master key text is not valid base64.
    #[error("master key is not valid base64")]
    MasterKeyEncoding,
    /// The master key does not decode to exactly 32 bytes.
    #[error("master key must be 32 bytes, got {actual}")]
    MasterKeyLength {
        /// How many bytes the text decoded to.
        actual: usize,
    },
    /// Key version 0 is reserved and never valid.
    #[error("key version 0 is not allowed")]
    KeyVersionZero,
    /// Two keys in a keyring share the same version.
    #[error("duplicate master key version {0}")]
    DuplicateKeyVersion(KeyVersion),
    /// No master key with this version is configured.
    #[error("no master key with version {0}")]
    UnknownKeyVersion(KeyVersion),
    /// The account id is empty or too long.
    #[error("account id must be 1 to {max} bytes")]
    AccountIdLength {
        /// The maximum length.
        max: usize,
    },
    /// A sealed value was opened as a different field than it was sealed as.
    #[error("sealed value is {found:?}, expected {expected:?}")]
    FieldMismatch {
        /// The field the caller asked for.
        expected: Field,
        /// The field stored in the sealed value.
        found: Field,
    },
    /// A field that needs a [`crate::Binding`] was sealed or opened without
    /// one, or one that takes none with one.
    #[error("wrong binding for {0:?}")]
    WrongBinding(Field),
    /// A [`crate::Binding`] longer than its limit.
    #[error("binding must be at most {max} bytes")]
    BindingLength {
        /// The maximum length.
        max: usize,
    },
    /// The secret to seal is longer than its field allows
    /// ([`crate::Field::max_len`]).
    #[error("secret is too long to seal")]
    SecretTooLong,
    /// Authenticated decryption failed: wrong key, account, field or version,
    /// or the data was changed. On purpose it doesn't say which.
    #[error("decryption failed")]
    Decrypt,
    /// The cipher refused to encrypt (only possible with absurd input sizes).
    #[error("encryption failed")]
    Encrypt,
    /// A decrypted secret was expected to be UTF-8 text and isn't.
    #[error("decrypted secret is not UTF-8")]
    NotUtf8,
    /// The operating system's random number generator failed.
    #[error("random number generator failed")]
    Random,
    /// A stored value does not follow the expected binary format.
    #[error("invalid encrypted value: {0}")]
    Format(#[from] FormatError),
    /// A token is not 43 characters of unpadded base64url (32 bytes).
    #[error("invalid token")]
    InvalidToken,
    /// A token hash is not exactly 32 bytes.
    #[error("invalid token hash")]
    InvalidTokenHash,
    /// A PKCE code verifier does not follow RFC 7636 §4.1.
    #[error("invalid PKCE code verifier")]
    InvalidCodeVerifier,
    /// A PKCE S256 code challenge is not 43 characters of base64url.
    #[error("invalid PKCE code challenge")]
    InvalidCodeChallenge,
    /// The PKCE code verifier does not match the challenge.
    #[error("PKCE verification failed")]
    PkceMismatch,
    /// A client secret hash is not an argon2id PHC string we accept.
    #[error("invalid client secret hash")]
    InvalidClientSecretHash,
    /// Computing an argon2 hash failed.
    #[error("password hashing failed")]
    PasswordHash,
    /// The client secret does not match its hash.
    #[error("client secret does not match")]
    ClientSecretMismatch,
}

/// Why a stored encrypted value could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The value is shorter than its fixed parts.
    #[error("truncated")]
    Truncated,
    /// The value is longer than allowed.
    #[error("too long")]
    TooLong,
    /// The format version byte is not one we know.
    #[error("unsupported format version {0}")]
    UnsupportedVersion(u8),
    /// The value is of another kind (a sealed field where a wrapped key was
    /// expected, or the other way round).
    #[error("wrong kind of value")]
    WrongKind,
    /// The field tag is not one we know.
    #[error("unknown field tag {0}")]
    UnknownField(u8),
}
