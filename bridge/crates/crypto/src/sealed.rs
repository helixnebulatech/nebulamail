// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::fmt;

use secrecy::SecretSlice;

use crate::account::AccountId;
use crate::binding::Binding;
use crate::error::{CryptoError, FormatError};
use crate::format::{self, KEY_LEN, Kind, TAG_LEN};

/// Which secret a [`SealedField`] holds. Part of the AAD, so a ciphertext
/// can't be moved to another column (an IMAP password opened as an OAuth
/// token, say).
///
/// The tags are stored in the database: never change or reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Field {
    /// The password for the IMAP server.
    ImapPassword,
    /// The password for the SMTP server, when it differs from IMAP's.
    SmtpPassword,
    /// An OAuth refresh token for the mail provider (Gmail, Outlook).
    OAuthRefreshToken,
    /// An OAuth access token for the mail provider.
    OAuthAccessToken,
    /// The iCalendar or vCard text of a CalDAV/CardDAV resource, kept in
    /// the bridge's local copy. Up to
    /// [`SealedField::MAX_DATA_LEN`], not [`SealedField::MAX_SECRET_LEN`].
    /// Bound to its resource ([`Field::needs_binding`]).
    DavResource,
    /// The push URL and keys of a Web Push subscription (RFC 8620 §7.2):
    /// a capability to notify the user's browser. Bound to the
    /// subscription.
    PushSubscription,
    /// The bridge's VAPID private key (RFC 8292), sealed with a data key
    /// of its own (not an account's).
    VapidKey,
    /// A link of a calendar (Nextcloud's public link or a webcal
    /// subscription's source): a capability to read it. Bound to the
    /// collection and to which link it is.
    DavCollectionLink,
    /// The message of a held submission (undo send and scheduled send,
    /// RFC 4865 FUTURERELEASE), kept until it is released. Up to
    /// [`SealedField::MAX_MESSAGE_LEN`]. Bound to its submission.
    HeldMessage,
}

impl Field {
    /// Tag 5 was [`Field::DavResource`] without a [`Binding`] (
    /// development only): retired, never reused. Such values no longer
    /// open; the store dropped them and syncs again.
    const fn tag(self) -> u8 {
        match self {
            Self::ImapPassword => 1,
            Self::SmtpPassword => 2,
            Self::OAuthRefreshToken => 3,
            Self::OAuthAccessToken => 4,
            Self::DavResource => 6,
            Self::PushSubscription => 7,
            Self::VapidKey => 8,
            Self::DavCollectionLink => 9,
            Self::HeldMessage => 10,
        }
    }

    /// Whether values of this field are bound to a record of the account
    /// (sealed and opened with a [`Binding`]): those an account has many
    /// of, so one can't be swapped for another. Secrets (one per account
    /// and field) are not.
    #[must_use]
    pub const fn needs_binding(self) -> bool {
        match self {
            Self::DavResource
            | Self::PushSubscription
            | Self::DavCollectionLink
            | Self::HeldMessage => true,
            Self::VapidKey
            | Self::ImapPassword
            | Self::SmtpPassword
            | Self::OAuthRefreshToken
            | Self::OAuthAccessToken => false,
        }
    }

    /// The longest value this field may hold.
    #[must_use]
    pub const fn max_len(self) -> usize {
        match self {
            Self::DavResource => SealedField::MAX_DATA_LEN,
            Self::HeldMessage => SealedField::MAX_MESSAGE_LEN,
            Self::PushSubscription
            | Self::DavCollectionLink
            | Self::VapidKey
            | Self::ImapPassword
            | Self::SmtpPassword
            | Self::OAuthRefreshToken
            | Self::OAuthAccessToken => SealedField::MAX_SECRET_LEN,
        }
    }

    const fn from_tag(tag: u8) -> Result<Self, FormatError> {
        match tag {
            1 => Ok(Self::ImapPassword),
            2 => Ok(Self::SmtpPassword),
            3 => Ok(Self::OAuthRefreshToken),
            4 => Ok(Self::OAuthAccessToken),
            // 5 is retired (see `tag`).
            6 => Ok(Self::DavResource),
            7 => Ok(Self::PushSubscription),
            8 => Ok(Self::VapidKey),
            9 => Ok(Self::DavCollectionLink),
            10 => Ok(Self::HeldMessage),
            other => Err(FormatError::UnknownField(other)),
        }
    }
}

/// A secret encrypted with an account's [`crate::DataKey`], ready to store.
///
/// Its bytes are a self-describing value (see the crate's format notes):
/// format version, field tag, nonce, ciphertext and tag. The AAD binds it to
/// the account and the field, and for some fields to a record of the
/// account ([`Binding`]).
///
/// It records no key version: the data key belongs to one account and is
/// stored next to it, and rotating the master key re-wraps only the data
/// key, so sealed fields never need to change.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedField {
    /// Parsed from `bytes` once, so it is always valid.
    field: Field,
    bytes: Box<[u8]>,
}

impl SealedField {
    /// The longest secret that can be sealed (16 KiB: OAuth tokens can be a
    /// few kilobytes, passwords are far shorter).
    pub const MAX_SECRET_LEN: usize = 16 * 1024;

    /// The longest calendar or contact that can be sealed
    /// ([`Field::DavResource`]): 8 MiB, above the 4 MiB the DAV client
    /// accepts for a vCard.
    pub const MAX_DATA_LEN: usize = 8 * 1024 * 1024;

    /// The longest held message that can be sealed ([`Field::HeldMessage`]):
    /// 128 MiB, above a message with the 50 MB of attachments the bridge
    /// takes by default once they are in base64.
    pub const MAX_MESSAGE_LEN: usize = 128 * 1024 * 1024;

    /// The longest value of any field.
    const MAX_ANY_LEN: usize = Self::MAX_MESSAGE_LEN;

    /// The longest valid sealed secret (every field but
    /// [`Field::DavResource`]).
    pub const MAX_LEN: usize = Self::sealed_len(Self::MAX_SECRET_LEN);

    /// The length of a sealed value holding `secret_len` bytes.
    #[must_use]
    pub const fn sealed_len(secret_len: usize) -> usize {
        2 + FIELD_TAG_LEN + format::NONCE_LEN + secret_len + TAG_LEN
    }

    /// Parses a stored sealed field.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Format`] if the bytes aren't a sealed field.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let parts = split(bytes)?;
        let field = parse_field(parts.kind_header)?;
        if bytes.len() > Self::sealed_len(field.max_len()) {
            return Err(FormatError::TooLong.into());
        }
        Ok(Self {
            field,
            bytes: bytes.into(),
        })
    }

    /// The bytes to store.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The field this value holds.
    #[must_use]
    pub const fn field(&self) -> Field {
        self.field
    }

    pub(crate) fn seal(
        key: &[u8; KEY_LEN],
        account: &AccountId,
        field: Field,
        binding: Option<&Binding<'_>>,
        secret: &[u8],
    ) -> Result<Self, CryptoError> {
        check_binding(field, binding)?;
        if secret.len() > field.max_len() {
            return Err(CryptoError::SecretTooLong);
        }
        let header = format::header(Kind::SealedField, &[field.tag()]);
        let bytes = format::seal(key, header, account, binding, secret)?;
        Ok(Self {
            field,
            bytes: bytes.into(),
        })
    }

    pub(crate) fn open(
        &self,
        key: &[u8; KEY_LEN],
        account: &AccountId,
        expected: Field,
        binding: Option<&Binding<'_>>,
    ) -> Result<SecretSlice<u8>, CryptoError> {
        if self.field != expected {
            return Err(CryptoError::FieldMismatch {
                expected,
                found: self.field,
            });
        }
        check_binding(expected, binding)?;
        let parts = split(&self.bytes)?;
        let plaintext = format::open(key, &parts, account, binding)?;
        Ok(SecretSlice::from(plaintext.to_vec()))
    }
}

impl fmt::Debug for SealedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SealedField")
            .field("field", &self.field)
            .finish_non_exhaustive()
    }
}

const FIELD_TAG_LEN: usize = 1;

/// A bound field needs a binding, and the others take none.
const fn check_binding(field: Field, binding: Option<&Binding<'_>>) -> Result<(), CryptoError> {
    if field.needs_binding() == binding.is_some() {
        Ok(())
    } else {
        Err(CryptoError::WrongBinding(field))
    }
}

/// Splits a sealed value of any field; [`SealedField::from_bytes`] then
/// checks the length against its field's limit.
fn split(bytes: &[u8]) -> Result<format::Parts<'_>, FormatError> {
    if bytes.len() > SealedField::sealed_len(SealedField::MAX_ANY_LEN) {
        return Err(FormatError::TooLong);
    }
    format::split(
        bytes,
        Kind::SealedField,
        FIELD_TAG_LEN,
        TAG_LEN..=SealedField::MAX_ANY_LEN + TAG_LEN,
    )
}

fn parse_field(kind_header: &[u8]) -> Result<Field, FormatError> {
    match kind_header {
        [tag] => Field::from_tag(*tag),
        _ => Err(FormatError::Truncated),
    }
}
