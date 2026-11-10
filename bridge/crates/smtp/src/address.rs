// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The SMTP envelope: who it is from (`MAIL FROM`) and who gets it
//! (`RCPT TO`). These are not the `From:`/`To:` headers.

use std::fmt;
use std::str::FromStr;

use lettre::Address;

use crate::dsn::{MailDsn, RcptDsn};
use crate::error::SmtpError;

/// Why an envelope address was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AddressError {
    /// Longer than the 254 characters an SMTP path can hold.
    #[error("address is too long")]
    TooLong,
    /// Has a control character (CR, LF, NUL...), a space, `<` or `>`: these
    /// could inject SMTP commands or parameters.
    #[error("address contains a forbidden character")]
    ForbiddenCharacter,
    /// Not a valid `local@domain` address.
    #[error("invalid address")]
    Invalid,
}

/// Parses a bare `local@domain` address, rejecting anything that could
/// break out of `<...>` in `MAIL FROM:<...>` or `RCPT TO:<...>`.
fn parse(text: &str) -> Result<Address, AddressError> {
    // RFC 5321 §4.5.3.1.3: a path is at most 256 octets with the brackets.
    const MAX_LEN: usize = 254;
    if text.len() > MAX_LEN {
        return Err(AddressError::TooLong);
    }
    let forbidden = |c: char| c.is_control() || c.is_whitespace() || c == '<' || c == '>';
    if text.chars().any(forbidden) {
        return Err(AddressError::ForbiddenCharacter);
    }
    Address::from_str(text).map_err(|_| AddressError::Invalid)
}

/// The envelope sender (`MAIL FROM`). The null sender `<>` is for bounces
/// only, so it is not accepted.
#[derive(Clone, PartialEq, Eq)]
pub struct MailFrom(Address);

/// An envelope recipient (`RCPT TO`).
#[derive(Clone, PartialEq, Eq)]
pub struct RcptTo(Address);

macro_rules! address_type {
    ($name:ident) => {
        impl $name {
            /// The address as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_ref()
            }

            /// Whether it has non-ASCII characters, which need SMTPUTF8.
            #[must_use]
            pub fn is_ascii(&self) -> bool {
                self.as_str().is_ascii()
            }

            pub(crate) fn address(&self) -> &Address {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = AddressError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                parse(text).map(Self)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&self.as_str())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

address_type!(MailFrom);
address_type!(RcptTo);

/// A complete envelope: one sender and 1 to [`Envelope::MAX_RECIPIENTS`]
/// recipients (Bcc recipients included: they are only in the envelope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    from: MailFrom,
    to: Vec<RcptTo>,
    /// DSN parameters of `MAIL FROM`.
    mail_dsn: MailDsn,
    /// DSN parameters of each recipient, same order as `to`.
    rcpt_dsn: Vec<RcptDsn>,
}

impl Envelope {
    /// The most recipients in one envelope (the minimum servers must accept,
    /// RFC 5321 §4.5.3.1.8).
    pub const MAX_RECIPIENTS: usize = 100;

    /// Creates an envelope. Repeated recipients are sent only once.
    ///
    /// # Errors
    ///
    /// [`SmtpError::RecipientCount`] if `to` is empty or too long.
    pub fn new(from: MailFrom, to: Vec<RcptTo>) -> Result<Self, SmtpError> {
        let mut unique: Vec<RcptTo> = Vec::with_capacity(to.len());
        for recipient in to {
            if !unique.contains(&recipient) {
                unique.push(recipient);
            }
        }
        if unique.is_empty() || unique.len() > Self::MAX_RECIPIENTS {
            return Err(SmtpError::RecipientCount {
                max: Self::MAX_RECIPIENTS,
            });
        }
        let rcpt_dsn = vec![RcptDsn::default(); unique.len()];
        Ok(Self {
            from,
            to: unique,
            mail_dsn: MailDsn::default(),
            rcpt_dsn,
        })
    }

    /// Sets the DSN parameters of `MAIL FROM` (RFC 3461).
    #[must_use]
    pub fn with_mail_dsn(mut self, dsn: MailDsn) -> Self {
        self.mail_dsn = dsn;
        self
    }

    /// Sets the DSN parameters of one recipient (RFC 3461).
    ///
    /// # Errors
    ///
    /// [`SmtpError::RecipientCount`] if `recipient` is not in the envelope.
    pub fn with_recipient_dsn(
        mut self,
        recipient: &RcptTo,
        dsn: RcptDsn,
    ) -> Result<Self, SmtpError> {
        let index =
            self.to
                .iter()
                .position(|r| r == recipient)
                .ok_or(SmtpError::RecipientCount {
                    max: Self::MAX_RECIPIENTS,
                })?;
        if let Some(slot) = self.rcpt_dsn.get_mut(index) {
            *slot = dsn;
        }
        Ok(self)
    }

    /// The DSN parameters of `MAIL FROM`.
    #[must_use]
    pub const fn mail_dsn(&self) -> &MailDsn {
        &self.mail_dsn
    }

    /// Each recipient with its DSN parameters.
    pub fn recipients(&self) -> impl Iterator<Item = (&RcptTo, &RcptDsn)> {
        self.to.iter().zip(&self.rcpt_dsn)
    }

    /// Whether any DSN parameter is set (the server must offer DSN).
    #[must_use]
    pub fn uses_dsn(&self) -> bool {
        !self.mail_dsn.is_empty() || self.rcpt_dsn.iter().any(|d| !d.is_empty())
    }

    /// The sender.
    #[must_use]
    pub const fn from(&self) -> &MailFrom {
        &self.from
    }

    /// The recipients.
    #[must_use]
    pub fn to(&self) -> &[RcptTo] {
        &self.to
    }

    /// Whether any address has non-ASCII characters (needs SMTPUTF8).
    ///
    /// DSN `ORCPT` values don't count: a `utf-8` one is written as
    /// `utf-8-addr-xtext`, which is ASCII (RFC 6533 §3).
    #[must_use]
    pub fn needs_smtputf8(&self) -> bool {
        !self.from.is_ascii() || self.to.iter().any(|rcpt| !rcpt.is_ascii())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn accepts_normal_addresses() {
        for ok in [
            "alice@example.com",
            "a.b+tag@sub.example.org",
            "bob@localhost",
            "josé@example.es",
        ] {
            assert_eq!(ok.parse::<RcptTo>().unwrap().as_str(), ok);
        }
    }

    #[test]
    fn rejects_injection_tricks() {
        for bad in [
            "alice@example.com>",
            "<alice@example.com>",
            "alice@example.com> SIZE=1",
            "alice@example.com>\r\nRCPT TO:<eve@evil.com",
            "alice@example.com\nDATA",
            "alice@exa\0mple.com",
            "alice @example.com",
            "alice@example.com\t",
            "\"a b\"@example.com",
        ] {
            assert_eq!(
                bad.parse::<MailFrom>(),
                Err(AddressError::ForbiddenCharacter),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn rejects_invalid_and_long() {
        for bad in ["", "alice", "@example.com", "alice@", "a@b@c"] {
            assert_eq!(bad.parse::<RcptTo>(), Err(AddressError::Invalid), "{bad:?}");
        }
        let long = format!("{}@example.com", "a".repeat(250));
        assert_eq!(long.parse::<RcptTo>(), Err(AddressError::TooLong));
    }

    #[test]
    fn envelope_rules() {
        let from: MailFrom = "alice@example.com".parse().unwrap();
        let bob: RcptTo = "bob@example.com".parse().unwrap();
        assert!(matches!(
            Envelope::new(from.clone(), vec![]),
            Err(SmtpError::RecipientCount { .. })
        ));
        let envelope = Envelope::new(from.clone(), vec![bob.clone(), bob.clone()]).unwrap();
        assert_eq!(envelope.to(), &[bob]);
        assert!(!envelope.needs_smtputf8());

        let many = (0..=Envelope::MAX_RECIPIENTS)
            .map(|n| format!("u{n}@example.com").parse().unwrap())
            .collect();
        assert!(Envelope::new(from.clone(), many).is_err());

        let utf8 = Envelope::new(from, vec!["josé@example.es".parse().unwrap()]).unwrap();
        assert!(utf8.needs_smtputf8());
    }

    proptest! {
        #[test]
        fn parsed_addresses_never_contain_line_breaks_or_brackets(text in "(?s).{0,80}") {
            if let Ok(address) = text.parse::<RcptTo>() {
                prop_assert!(!address.as_str().contains(['\r', '\n', '<', '>', ' ', '\0']));
            }
        }
    }
}
