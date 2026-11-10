// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMAP integers (RFC 8620 §1.3): numbers that fit exactly in an IEEE 754
//! double, so JavaScript clients read them without losing precision.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 2^53 − 1, the largest integer JavaScript represents exactly.
const MAX_SAFE: u64 = 9_007_199_254_740_991;
/// −(2^53 − 1).
const MIN_SAFE: i64 = -9_007_199_254_740_991;

/// Why a number is not a valid JMAP integer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IntError {
    /// Outside −(2^53−1) ..= 2^53−1 (or negative for `UnsignedInt`).
    #[error("number out of range")]
    OutOfRange,
    /// Zero where a positive number is required.
    #[error("number must be greater than 0")]
    Zero,
}

/// `UnsignedInt` (RFC 8620 §1.3): an integer in `0 ..= 2^53 − 1`.
///
/// Deserializing rejects negative, fractional (`1.5`, `1.0`) and too big
/// numbers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UnsignedInt(u64);

impl UnsignedInt {
    /// Zero.
    pub const ZERO: Self = Self(0);
    /// 2^53 − 1.
    pub const MAX: Self = Self(MAX_SAFE);

    /// `value` if it is in range.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value <= MAX_SAFE {
            Some(Self(value))
        } else {
            None
        }
    }

    /// `value`, or [`Self::MAX`] if it is bigger (for sizes and counts
    /// that can't really reach 2^53).
    #[must_use]
    pub const fn saturating(value: u64) -> Self {
        if value <= MAX_SAFE {
            Self(value)
        } else {
            Self::MAX
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The sum, if it stays in range.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).and_then(Self::new)
    }

    /// The value as `usize`, saturating on (theoretical) 32-bit targets.
    #[must_use]
    pub fn to_usize_saturating(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

impl From<u32> for UnsignedInt {
    fn from(value: u32) -> Self {
        Self(u64::from(value))
    }
}

impl TryFrom<u64> for UnsignedInt {
    type Error = IntError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(IntError::OutOfRange)
    }
}

impl TryFrom<usize> for UnsignedInt {
    type Error = IntError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map_err(|_| IntError::OutOfRange)
            .and_then(Self::try_from)
    }
}

impl TryFrom<i64> for UnsignedInt {
    type Error = IntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map_err(|_| IntError::OutOfRange)
            .and_then(Self::try_from)
    }
}

impl From<UnsignedInt> for u64 {
    fn from(value: UnsignedInt) -> Self {
        value.0
    }
}

impl fmt::Display for UnsignedInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for UnsignedInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for UnsignedInt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for UnsignedInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = deserializer.deserialize_any(IntVisitor)?;
        Self::try_from(value).map_err(de::Error::custom)
    }
}

/// `Int` (RFC 8620 §1.3): an integer in `−(2^53 − 1) ..= 2^53 − 1`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Int(i64);

impl Int {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// `value` if it is in range.
    #[must_use]
    pub const fn new(value: i64) -> Option<Self> {
        if value >= MIN_SAFE && value <= MIN_SAFE.saturating_neg() {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for Int {
    type Error = IntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(IntError::OutOfRange)
    }
}

impl From<i32> for Int {
    fn from(value: i32) -> Self {
        Self(i64::from(value))
    }
}

impl From<Int> for i64 {
    fn from(value: Int) -> Self {
        value.0
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Int {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Int {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = deserializer.deserialize_any(IntVisitor)?;
        Self::try_from(value).map_err(de::Error::custom)
    }
}

/// An `UnsignedInt` greater than 0, e.g. `maxChanges` (RFC 8620 §5.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PositiveInt(UnsignedInt);

impl PositiveInt {
    /// `value` if it is greater than 0 and in range.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match UnsignedInt::new(value) {
            Some(n) if value > 0 => Some(Self(n)),
            _ => None,
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> UnsignedInt {
        self.0
    }
}

impl TryFrom<UnsignedInt> for PositiveInt {
    type Error = IntError;

    fn try_from(value: UnsignedInt) -> Result<Self, Self::Error> {
        if value.get() == 0 {
            Err(IntError::Zero)
        } else {
            Ok(Self(value))
        }
    }
}

impl fmt::Debug for PositiveInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for PositiveInt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PositiveInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = UnsignedInt::deserialize(deserializer)?;
        Self::try_from(value).map_err(de::Error::custom)
    }
}

/// Accepts JSON integers only; fractions (even `1.0`) are a type error.
struct IntVisitor;

impl Visitor<'_> for IntVisitor {
    type Value = i64;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an integer between -(2^53-1) and 2^53-1")
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<i64, E> {
        Ok(value)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<i64, E> {
        i64::try_from(value).map_err(|_| E::custom(IntError::OutOfRange))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn unsigned_int_bounds() {
        assert_eq!(UnsignedInt::new(MAX_SAFE).unwrap().get(), MAX_SAFE);
        assert!(UnsignedInt::new(MAX_SAFE + 1).is_none());
        let ok: UnsignedInt = serde_json::from_str("9007199254740991").unwrap();
        assert_eq!(ok, UnsignedInt::MAX);
        for bad in [
            "9007199254740992",
            "-1",
            "1.5",
            "1.0",
            "1e3",
            "\"1\"",
            "null",
        ] {
            assert!(serde_json::from_str::<UnsignedInt>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn int_bounds() {
        let min: Int = serde_json::from_str("-9007199254740991").unwrap();
        assert_eq!(min.get(), MIN_SAFE);
        assert!(serde_json::from_str::<Int>("-9007199254740992").is_err());
        assert!(serde_json::from_str::<Int>("9007199254740992").is_err());
        assert!(serde_json::from_str::<Int>("-0.5").is_err());
    }

    #[test]
    fn positive_int_rejects_zero() {
        assert!(serde_json::from_str::<PositiveInt>("0").is_err());
        assert_eq!(
            serde_json::from_str::<PositiveInt>("5")
                .unwrap()
                .get()
                .get(),
            5
        );
    }

    proptest! {
        #[test]
        fn unsigned_int_accepts_exactly_the_safe_range(n in any::<u64>()) {
            let parsed = serde_json::from_str::<UnsignedInt>(&n.to_string());
            prop_assert_eq!(parsed.is_ok(), n <= MAX_SAFE);
            if let Ok(value) = parsed {
                prop_assert_eq!(serde_json::to_string(&value).unwrap(), n.to_string());
            }
        }

        #[test]
        fn int_accepts_exactly_the_safe_range(n in any::<i64>()) {
            let parsed = serde_json::from_str::<Int>(&n.to_string());
            prop_assert_eq!(parsed.is_ok(), (MIN_SAFE..=-MIN_SAFE).contains(&n));
        }

        #[test]
        fn negative_numbers_are_not_unsigned(n in i64::MIN..0) {
            prop_assert!(serde_json::from_str::<UnsignedInt>(&n.to_string()).is_err());
        }
    }
}
