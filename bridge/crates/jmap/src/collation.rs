// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Collations for `/query` sorts (RFC 8620 §5.5, RFC 4790): the ones the
//! session offers in `collationAlgorithms` (`i;ascii-numeric`,
//! `i;ascii-casemap`, `i;unicode-casemap`). Any other (`i;octet`) is
//! `unsupportedSort` ("a collation method it does not recognise"), never
//! accepted and ignored. Without one, text sorts are case-insensitive
//! (`i;unicode-casemap`).

use std::cmp::Ordering;

use nebula_jmap_proto::MethodError;
use nebula_jmap_proto::method::{Collation, Comparator};

/// A collation the bridge applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Collate {
    /// `i;unicode-casemap` (RFC 5051), the default: approximated as each
    /// string's Unicode lowercase compared by code point (RFC 5051 maps
    /// to titlecase and NFKD; the order differs only between letters and
    /// the six ASCII signs between `Z` and `a`, and for compatibility
    /// forms).
    UnicodeCasemap,
    /// `i;ascii-casemap` (RFC 4790 §9.2): `a`–`z` as `A`–`Z`, then octets.
    AsciiCasemap,
    /// `i;ascii-numeric` (RFC 4790 §9.1): the number the string starts
    /// with; strings without one are equal and after every number.
    AsciiNumeric,
}

impl Collate {
    /// The collation of a comparator (`None`: the default).
    ///
    /// # Errors
    /// `unsupportedSort` for one the session doesn't offer.
    pub(crate) fn of(collation: Option<Collation>) -> Result<Self, MethodError> {
        match collation {
            None | Some(Collation::UnicodeCasemap) => Ok(Self::UnicodeCasemap),
            Some(Collation::AsciiCasemap) => Ok(Self::AsciiCasemap),
            Some(Collation::AsciiNumeric) => Ok(Self::AsciiNumeric),
            Some(other) => Err(MethodError::unsupported_sort(format!(
                "collation {} is not supported",
                other.as_str()
            ))),
        }
    }

    /// Compares two strings.
    pub(crate) fn compare(self, a: &str, b: &str) -> Ordering {
        match self {
            Self::UnicodeCasemap => a
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(b.chars().flat_map(char::to_lowercase)),
            Self::AsciiCasemap => a
                .bytes()
                .map(|byte| byte.to_ascii_uppercase())
                .cmp(b.bytes().map(|byte| byte.to_ascii_uppercase())),
            Self::AsciiNumeric => match (leading_number(a), leading_number(b)) {
                (Some(x), Some(y)) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        }
    }
}

/// A `sort` with every comparator's collation checked (RFC 8620 §5.5):
/// built before the query does any work, so a collation the session
/// doesn't offer is `unsupportedSort` up front, and the same for `/query`
/// and `/queryChanges` (one order for both). Properties that aren't text
/// (dates, numbers, ids, booleans) take the collation and ignore it, as
/// `Email/query` does for dates and sizes.
pub(crate) struct CheckedSort<'a, S> {
    comparators: Vec<(&'a Comparator<S>, Collate)>,
}

impl<'a, S> CheckedSort<'a, S> {
    /// # Errors
    /// `unsupportedSort` for a collation the session doesn't offer.
    pub(crate) fn new(sort: &'a [Comparator<S>]) -> Result<Self, MethodError> {
        Ok(Self {
            comparators: sort
                .iter()
                .map(|c| Ok((c, Collate::of(c.collation)?)))
                .collect::<Result<_, MethodError>>()?,
        })
    }

    /// Whether there is no comparator (the method's default order).
    pub(crate) fn is_empty(&self) -> bool {
        self.comparators.is_empty()
    }

    /// Compares by each comparator in turn until one differs (`Equal` if
    /// none does). `by` gives a property's ascending order, comparing
    /// text with the collation it is passed.
    pub(crate) fn compare<T: ?Sized>(
        &self,
        a: &T,
        b: &T,
        mut by: impl FnMut(&S, Collate, &T, &T) -> Ordering,
    ) -> Ordering {
        for (comparator, collate) in &self.comparators {
            let order = by(&comparator.property, *collate, a, b);
            let order = if comparator.is_ascending {
                order
            } else {
                order.reverse()
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    }
}

/// The decimal digits a string starts with, without leading zeros (`"0"`
/// stays one digit); `None` if it doesn't start with a digit.
fn leading_number(text: &str) -> Option<&str> {
    let end = text
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(text.len());
    let digits = text.get(..end).filter(|digits| !digits.is_empty())?;
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() {
        digits.get(end.saturating_sub(1)..).unwrap_or(digits)
    } else {
        trimmed
    })
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::MethodErrorType;

    use super::*;

    #[test]
    fn offered_collations_only() {
        assert_eq!(Collate::of(None).unwrap(), Collate::UnicodeCasemap);
        assert_eq!(
            Collate::of(Some(Collation::AsciiNumeric)).unwrap(),
            Collate::AsciiNumeric
        );
        // Not in `collationAlgorithms`: never accepted and ignored.
        assert_eq!(
            Collate::of(Some(Collation::Octet)).unwrap_err().kind(),
            MethodErrorType::UnsupportedSort
        );
    }

    #[test]
    fn unicode_casemap_ignores_case() {
        let c = Collate::UnicodeCasemap;
        assert_eq!(c.compare("Ábaco", "ábaco"), Ordering::Equal);
        assert_eq!(c.compare("alpha", "Beta"), Ordering::Less);
        assert_eq!(c.compare("Zeta", "alpha"), Ordering::Greater);
    }

    #[test]
    fn ascii_casemap_folds_only_ascii() {
        let c = Collate::AsciiCasemap;
        assert_eq!(c.compare("ABC", "abc"), Ordering::Equal);
        assert_ne!(c.compare("É", "é"), Ordering::Equal);
        // RFC 4790 §9.2: "_" (0x5F) sorts after the letters' uppercase.
        assert_eq!(c.compare("_", "a"), Ordering::Greater);
    }

    #[test]
    fn ascii_numeric_compares_numbers() {
        let c = Collate::AsciiNumeric;
        assert_eq!(c.compare("9", "10"), Ordering::Less);
        assert_eq!(c.compare("007 x", "7"), Ordering::Equal);
        assert_eq!(c.compare("0", "00"), Ordering::Equal);
        assert_eq!(c.compare("12", "abc"), Ordering::Less);
        assert_eq!(c.compare("abc", "xyz"), Ordering::Equal);
    }
}
