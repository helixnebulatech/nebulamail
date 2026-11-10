// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sequence sets (`1,3:5,7:*`) of UIDs or sequence numbers.

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use super::number::{SeqNum, Uid, parse_digits_u32};
use crate::ValueError;

/// A number that can appear in a sequence set: [`Uid`] or [`SeqNum`].
pub trait SequenceValue: Copy + Ord + fmt::Debug + sealed::Sealed {
    /// The number on the wire.
    fn to_u32(self) -> u32;
    /// Builds the value from a non-zero number.
    fn from_non_zero(value: NonZeroU32) -> Self;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Uid {}
    impl Sealed for super::SeqNum {}
}

impl SequenceValue for Uid {
    fn to_u32(self) -> u32 {
        self.get()
    }
    fn from_non_zero(value: NonZeroU32) -> Self {
        Self::from(value)
    }
}

impl SequenceValue for SeqNum {
    fn to_u32(self) -> u32 {
        self.get()
    }
    fn from_non_zero(value: NonZeroU32) -> Self {
        Self::from(value)
    }
}

/// One end of a range: a number or `*` (the largest number in use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeqBound<T> {
    /// A concrete number.
    Value(T),
    /// `*`: the largest UID or sequence number in the mailbox.
    Star,
}

/// One element of a sequence set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeqItem<T> {
    /// A single number (or `*`).
    Single(SeqBound<T>),
    /// An inclusive range, in either order (`5:2` is the same as `2:5`).
    Range(SeqBound<T>, SeqBound<T>),
}

/// A non-empty set of UIDs or sequence numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SequenceSet<T> {
    items: Vec<SeqItem<T>>,
}

/// A set of UIDs.
pub type UidSet = SequenceSet<Uid>;
/// A set of message sequence numbers.
pub type SeqSet = SequenceSet<SeqNum>;

impl<T: SequenceValue> SequenceSet<T> {
    /// A set with one number.
    #[must_use]
    pub fn single(value: T) -> Self {
        Self {
            items: vec![SeqItem::Single(SeqBound::Value(value))],
        }
    }

    /// An inclusive range.
    #[must_use]
    pub fn range(first: T, last: T) -> Self {
        Self {
            items: vec![SeqItem::Range(
                SeqBound::Value(first),
                SeqBound::Value(last),
            )],
        }
    }

    /// `first:*`: from `first` to the end of the mailbox.
    #[must_use]
    pub fn from_value(first: T) -> Self {
        Self {
            items: vec![SeqItem::Range(SeqBound::Value(first), SeqBound::Star)],
        }
    }

    /// `1:*`: every message.
    #[must_use]
    pub fn all() -> Self {
        Self {
            items: vec![SeqItem::Range(
                SeqBound::Value(T::from_non_zero(NonZeroU32::MIN)),
                SeqBound::Star,
            )],
        }
    }

    /// Builds a set from items, if there is at least one.
    #[must_use]
    pub fn from_items(items: Vec<SeqItem<T>>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self { items })
        }
    }

    /// The shortest set with exactly these numbers: they are sorted, duplicates
    /// removed and consecutive runs written as ranges (`1,2,3,7` → `1:3,7`).
    ///
    /// Returns `None` if there are no numbers.
    pub fn from_values(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut values: Vec<T> = values.into_iter().collect();
        values.sort_unstable();
        values.dedup();
        let mut items = Vec::new();
        let mut run: Option<(T, T)> = None;
        for value in values {
            run = match run {
                Some((first, last)) if last.to_u32().checked_add(1) == Some(value.to_u32()) => {
                    Some((first, value))
                }
                Some((first, last)) => {
                    items.push(Self::run_item(first, last));
                    Some((value, value))
                }
                None => Some((value, value)),
            };
        }
        if let Some((first, last)) = run {
            items.push(Self::run_item(first, last));
        }
        Self::from_items(items)
    }

    fn run_item(first: T, last: T) -> SeqItem<T> {
        if first == last {
            SeqItem::Single(SeqBound::Value(first))
        } else {
            SeqItem::Range(SeqBound::Value(first), SeqBound::Value(last))
        }
    }

    /// The items, in the order they were given.
    #[must_use]
    pub fn items(&self) -> &[SeqItem<T>] {
        &self.items
    }

    /// Whether the set uses `*` (its size depends on the mailbox).
    #[must_use]
    pub fn has_star(&self) -> bool {
        self.items.iter().any(|item| match item {
            SeqItem::Single(bound) => *bound == SeqBound::Star,
            SeqItem::Range(first, last) => *first == SeqBound::Star || *last == SeqBound::Star,
        })
    }

    /// How many numbers the set lists (duplicates counted again), or `None`
    /// if it uses `*`.
    #[must_use]
    pub fn count(&self) -> Option<u64> {
        let mut total: u64 = 0;
        for item in &self.items {
            let (low, high) = Self::bounds(item)?;
            let size = u64::from(high.to_u32())
                .checked_sub(u64::from(low.to_u32()))?
                .checked_add(1)?;
            total = total.checked_add(size)?;
        }
        Some(total)
    }

    /// The numbers in order of appearance (ranges ascending), or `None` if the
    /// set uses `*`. Lazy: a huge range does not allocate.
    #[must_use]
    pub fn iter_values(&self) -> Option<impl Iterator<Item = T> + '_> {
        if self.has_star() {
            return None;
        }
        Some(self.items.iter().flat_map(|item| {
            let (low, high) =
                Self::bounds(item).map_or((1, 0), |(low, high)| (low.to_u32(), high.to_u32()));
            (low..=high).filter_map(|value| NonZeroU32::new(value).map(T::from_non_zero))
        }))
    }

    /// Whether `value` is listed (an item with `*` never matches, because
    /// its meaning depends on the mailbox).
    #[must_use]
    pub fn contains(&self, value: T) -> bool {
        self.items
            .iter()
            .any(|item| Self::bounds(item).is_some_and(|(low, high)| low <= value && value <= high))
    }

    /// Splits the set into sets whose text is at most `max_len` bytes, in
    /// order, so each command line stays short (RFC 7162 §4 asks clients to
    /// keep lines under 8192 bytes). An item longer than `max_len` on its
    /// own (never more than 21 bytes) gets a set of its own.
    #[must_use]
    pub fn split(&self, max_len: usize) -> Vec<Self> {
        let mut sets = Vec::new();
        let mut current: Vec<SeqItem<T>> = Vec::new();
        let mut len: usize = 0;
        for item in &self.items {
            let item_len = item_len(item);
            let extra = if current.is_empty() {
                item_len
            } else {
                item_len.saturating_add(1)
            };
            if !current.is_empty() && len.saturating_add(extra) > max_len {
                sets.push(Self {
                    items: std::mem::take(&mut current),
                });
                len = item_len;
            } else {
                len = len.saturating_add(extra);
            }
            current.push(*item);
        }
        if !current.is_empty() {
            sets.push(Self { items: current });
        }
        sets
    }

    fn bounds(item: &SeqItem<T>) -> Option<(T, T)> {
        match *item {
            SeqItem::Single(SeqBound::Value(value)) => Some((value, value)),
            SeqItem::Range(SeqBound::Value(first), SeqBound::Value(last)) => {
                Some((first.min(last), first.max(last)))
            }
            SeqItem::Single(SeqBound::Star) | SeqItem::Range(_, _) => None,
        }
    }
}

/// Length of an item's text.
fn item_len<T: SequenceValue>(item: &SeqItem<T>) -> usize {
    let bound_len = |bound: &SeqBound<T>| match bound {
        SeqBound::Value(value) => digits(value.to_u32()),
        SeqBound::Star => 1,
    };
    match item {
        SeqItem::Single(bound) => bound_len(bound),
        SeqItem::Range(first, last) => bound_len(first)
            .saturating_add(1)
            .saturating_add(bound_len(last)),
    }
}

fn digits(value: u32) -> usize {
    value.to_string().len()
}

impl<T: SequenceValue> fmt::Display for SeqBound<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => write!(f, "{}", value.to_u32()),
            Self::Star => f.write_str("*"),
        }
    }
}

impl<T: SequenceValue> fmt::Display for SequenceSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                f.write_str(",")?;
            }
            match item {
                SeqItem::Single(bound) => write!(f, "{bound}")?,
                SeqItem::Range(first, last) => write!(f, "{first}:{last}")?,
            }
        }
        Ok(())
    }
}

impl<T: SequenceValue> FromStr for SequenceSet<T> {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let items = text
            .split(',')
            .map(parse_item)
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_items(items).ok_or(ValueError::Empty("sequence set"))
    }
}

fn parse_item<T: SequenceValue>(text: &str) -> Result<SeqItem<T>, ValueError> {
    match text.split_once(':') {
        Some((first, last)) => Ok(SeqItem::Range(parse_bound(first)?, parse_bound(last)?)),
        None => Ok(SeqItem::Single(parse_bound(text)?)),
    }
}

fn parse_bound<T: SequenceValue>(text: &str) -> Result<SeqBound<T>, ValueError> {
    if text == "*" {
        return Ok(SeqBound::Star);
    }
    let value = parse_digits_u32(text).ok_or(ValueError::Invalid("sequence set"))?;
    NonZeroU32::new(value)
        .map(|value| SeqBound::Value(T::from_non_zero(value)))
        .ok_or(ValueError::Zero("sequence number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn uid(value: u32) -> Uid {
        Uid::new(value).unwrap()
    }

    #[test]
    fn compact_encoding() {
        let set = UidSet::from_values([7, 1, 2, 3, 3, 9, 10].map(uid)).unwrap();
        assert_eq!(set.to_string(), "1:3,7,9:10");
        assert_eq!(set.count(), Some(6));
        assert!(UidSet::from_values([]).is_none());
    }

    #[test]
    fn star_and_parse() {
        assert_eq!(UidSet::all().to_string(), "1:*");
        assert_eq!(UidSet::from_value(uid(5)).to_string(), "5:*");
        let set: UidSet = "4:2,*,9".parse().unwrap();
        assert!(set.has_star());
        assert!(set.contains(uid(3)));
        assert!(set.iter_values().is_none());
        assert!("".parse::<UidSet>().is_err());
        assert!("0".parse::<UidSet>().is_err());
        assert!("1,,2".parse::<UidSet>().is_err());
        assert!("1:2:3".parse::<UidSet>().is_err());
    }

    #[test]
    fn split_keeps_lines_short() {
        let set = UidSet::from_values((1..=200).step_by(2).map(uid)).unwrap();
        let parts = set.split(20);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|part| part.to_string().len() <= 20));
        let joined: Vec<u32> = parts
            .iter()
            .flat_map(|part| {
                part.iter_values()
                    .unwrap()
                    .map(Uid::get)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(joined, (1..=200).step_by(2).collect::<Vec<_>>());
        assert_eq!(UidSet::all().split(1), vec![UidSet::all()]);
        let short = UidSet::range(uid(1), uid(9));
        assert_eq!(short.split(8192), vec![short]);
    }

    #[test]
    fn huge_range_is_lazy() {
        let set = UidSet::range(uid(1), Uid::MAX);
        assert_eq!(set.count(), Some(u64::from(u32::MAX)));
        assert_eq!(set.iter_values().unwrap().take(2).count(), 2);
    }

    proptest! {
        #[test]
        fn from_values_round_trips(values in proptest::collection::vec(1u32..500, 1..60)) {
            let set = UidSet::from_values(values.iter().copied().map(uid)).unwrap();
            let mut expected: Vec<u32> = values.clone();
            expected.sort_unstable();
            expected.dedup();
            let listed: Vec<u32> = set.iter_values().unwrap().map(Uid::get).collect();
            prop_assert_eq!(&listed, &expected);
            let reparsed: UidSet = set.to_string().parse().unwrap();
            prop_assert_eq!(reparsed, set);
        }

        #[test]
        fn parse_never_panics(text in "[0-9:,*]{0,40}") {
            if let Ok(set) = text.parse::<UidSet>() {
                let again: UidSet = set.to_string().parse().unwrap();
                prop_assert_eq!(again, set);
            }
        }
    }
}
