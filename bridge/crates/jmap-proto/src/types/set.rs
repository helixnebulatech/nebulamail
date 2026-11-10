// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `A[Boolean]` maps whose values must all be `true` (RFC 8621
//! `mailboxIds` and `keywords`), as sets.

use std::collections::BTreeSet;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A set written in JSON as an object whose values are all `true`:
/// `{"$seen": true, "$flagged": true}`.
///
/// Deserializing rejects `false` and any other value (RFC 8621 §4.1.1:
/// "the value MUST be true").
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TrueSet<T: Ord>(BTreeSet<T>);

impl<T: Ord> TrueSet<T> {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// The items.
    #[must_use]
    pub fn as_set(&self) -> &BTreeSet<T> {
        &self.0
    }

    /// The items, by value.
    #[must_use]
    pub fn into_set(self) -> BTreeSet<T> {
        self.0
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether `item` is in the set.
    #[must_use]
    pub fn contains(&self, item: &T) -> bool {
        self.0.contains(item)
    }

    /// Adds an item; `false` if it was already there.
    pub fn insert(&mut self, item: T) -> bool {
        self.0.insert(item)
    }

    /// The items in order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T: Ord> Default for TrueSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord> From<BTreeSet<T>> for TrueSet<T> {
    fn from(set: BTreeSet<T>) -> Self {
        Self(set)
    }
}

impl<T: Ord> FromIterator<T> for TrueSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<T: Ord + fmt::Debug> fmt::Debug for TrueSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(&self.0).finish()
    }
}

impl<T: Ord + fmt::Display> Serialize for TrueSet<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for item in &self.0 {
            map.serialize_entry(&item.to_string(), &true)?;
        }
        map.end()
    }
}

impl<'de, T> Deserialize<'de> for TrueSet<T>
where
    T: Ord + FromStr,
    T::Err: fmt::Display,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(TrueSetVisitor(PhantomData))
    }
}

struct TrueSetVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for TrueSetVisitor<T>
where
    T: Ord + FromStr,
    T::Err: fmt::Display,
{
    type Value = TrueSet<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object whose values are all true")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut set = BTreeSet::new();
        while let Some(key) = access.next_key::<std::borrow::Cow<'de, str>>()? {
            let item = key.parse::<T>().map_err(de::Error::custom)?;
            if !access.next_value::<bool>()? {
                return Err(de::Error::custom("values must be true"));
            }
            set.insert(item);
        }
        Ok(TrueSet(set))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Keyword, MailboxId};

    #[test]
    fn round_trip() {
        let set: TrueSet<Keyword> = serde_json::from_str(r#"{"$Seen": true, "a": true}"#).unwrap();
        assert!(set.contains(&Keyword::seen()));
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            serde_json::json!({"$seen": true, "a": true})
        );
    }

    #[test]
    fn rejects_false_and_bad_keys() {
        assert!(serde_json::from_str::<TrueSet<Keyword>>(r#"{"$seen": false}"#).is_err());
        assert!(serde_json::from_str::<TrueSet<Keyword>>(r#"{"$seen": 1}"#).is_err());
        assert!(serde_json::from_str::<TrueSet<MailboxId>>(r#"{"a b": true}"#).is_err());
        assert!(serde_json::from_str::<TrueSet<MailboxId>>("[]").is_err());
    }
}
