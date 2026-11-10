// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Numeric newtypes: `Uid`, `UidValidity`, `SeqNum`, `ModSeq`.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use crate::ValueError;

macro_rules! non_zero_u32 {
    ($(#[$doc:meta])* $name:ident, $label:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU32);

        impl $name {
            /// The smallest value (1).
            pub const MIN: Self = Self(NonZeroU32::MIN);
            /// The largest value (2^32 - 1).
            pub const MAX: Self = Self(NonZeroU32::MAX);

            /// Returns `None` for zero.
            #[must_use]
            pub const fn new(value: u32) -> Option<Self> {
                match NonZeroU32::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// The number as sent on the wire.
            #[must_use]
            pub const fn get(self) -> u32 {
                self.0.get()
            }

            /// The next value, or `None` at the maximum.
            #[must_use]
            pub fn checked_next(self) -> Option<Self> {
                self.0.checked_add(1).map(Self)
            }
        }

        impl TryFrom<u32> for $name {
            type Error = ValueError;

            fn try_from(value: u32) -> Result<Self, Self::Error> {
                Self::new(value).ok_or(ValueError::Zero($label))
            }
        }

        impl From<NonZeroU32> for $name {
            fn from(value: NonZeroU32) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u32 {
            fn from(value: $name) -> Self {
                value.get()
            }
        }

        impl FromStr for $name {
            type Err = ValueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let value = parse_digits_u32(text).ok_or(ValueError::Invalid($label))?;
                Self::try_from(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

non_zero_u32!(
    /// Unique identifier of a message in a mailbox (RFC 3501 §2.3.1.1).
    ///
    /// Only meaningful together with the mailbox's [`UidValidity`].
    Uid,
    "UID"
);
non_zero_u32!(
    /// `UIDVALIDITY` of a mailbox: if it changes, all its UIDs are invalid.
    UidValidity,
    "UIDVALIDITY"
);
non_zero_u32!(
    /// Message sequence number (position in the mailbox, changes on expunge).
    SeqNum,
    "sequence number"
);

/// Digits only (no sign, no spaces), as in the IMAP `number` rule.
pub(crate) fn parse_digits_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Digits only, 64-bit.
pub(crate) fn parse_digits_u64(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Modification sequence of CONDSTORE (RFC 7162): 1 ..= 2^63 - 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModSeq(NonZeroU64);

impl ModSeq {
    /// The largest value allowed by RFC 7162 (`mod-sequence-value`).
    pub const MAX_VALUE: u64 = 9_223_372_036_854_775_807;

    /// Returns `None` for zero or values above 2^63 - 1.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        if value > Self::MAX_VALUE {
            return None;
        }
        NonZeroU64::new(value).map(Self)
    }

    /// The number as sent on the wire.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for ModSeq {
    type Error = ValueError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(ValueError::Zero("MODSEQ"));
        }
        Self::new(value).ok_or(ValueError::OutOfRange("MODSEQ"))
    }
}

impl From<ModSeq> for u64 {
    fn from(value: ModSeq) -> Self {
        value.get()
    }
}

impl FromStr for ModSeq {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = parse_digits_u64(text).ok_or(ValueError::Invalid("MODSEQ"))?;
        Self::try_from(value)
    }
}

impl fmt::Display for ModSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_rejected() {
        assert_eq!(Uid::try_from(0), Err(ValueError::Zero("UID")));
        assert!(UidValidity::new(0).is_none());
        assert!(ModSeq::new(0).is_none());
    }

    #[test]
    fn modseq_range() {
        assert!(ModSeq::new(ModSeq::MAX_VALUE).is_some());
        assert_eq!(
            ModSeq::try_from(9_223_372_036_854_775_808),
            Err(ValueError::OutOfRange("MODSEQ"))
        );
    }

    #[test]
    fn parse_only_digits() {
        assert_eq!("42".parse::<Uid>().map(Uid::get), Ok(42));
        assert!("+42".parse::<Uid>().is_err());
        assert!(" 42".parse::<Uid>().is_err());
        assert!("4294967296".parse::<Uid>().is_err());
    }
}
