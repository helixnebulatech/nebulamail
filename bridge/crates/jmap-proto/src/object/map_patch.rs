// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Patches of map properties such as `shareWith`: the whole map
//! (`"shareWith": {...}`) or one entry (`"shareWith/p1": {...}` / `null`).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::SetError;
use crate::method::parse::PropertyErrors;

/// A change to a map property.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MapPatch<K: Ord, V> {
    /// Replace the whole map (`None`: set it to `null`).
    Replace(Option<BTreeMap<K, V>>),
    /// Set and remove entries. The two never share a key.
    Change {
        /// Entries to add or replace.
        set: BTreeMap<K, V>,
        /// Keys to remove.
        remove: BTreeSet<K>,
    },
}

impl<K: Ord + Clone, V: Clone> MapPatch<K, V> {
    /// Applies the patch to the current map (`None`: `null`).
    pub fn apply(&self, current: &mut Option<BTreeMap<K, V>>) {
        match self {
            Self::Replace(map) => map.clone_into(current),
            Self::Change { set, remove } => {
                let map = current.get_or_insert_with(BTreeMap::new);
                map.retain(|key, _| !remove.contains(key));
                map.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
    }
}

/// Collects `property/key` patches of one map property.
pub(crate) struct MapMembers<K: Ord, V> {
    set: BTreeMap<K, V>,
    remove: BTreeSet<K>,
}

impl<K: Ord, V> Default for MapMembers<K, V> {
    fn default() -> Self {
        Self {
            set: BTreeMap::new(),
            remove: BTreeSet::new(),
        }
    }
}

impl<K: Ord + FromStr, V: DeserializeOwned> MapMembers<K, V> {
    /// Records `property/member: value` (`null` removes).
    pub(crate) fn record(
        &mut self,
        errors: &mut PropertyErrors,
        key: &str,
        member: &str,
        value: Value,
    ) -> Result<(), SetError> {
        let Ok(item) = member.parse::<K>() else {
            errors.invalid_because(key, "invalid key");
            return Ok(());
        };
        let duplicate = self.set.contains_key(&item) || self.remove.contains(&item);
        if duplicate {
            return Err(SetError::invalid_patch(format!("{key:?} is patched twice")));
        }
        if value.is_null() {
            self.remove.insert(item);
        } else if let Some(parsed) = errors.parse::<V>(key, value) {
            self.set.insert(item, parsed);
        }
        Ok(())
    }

    pub(crate) fn into_patch(self) -> Option<MapPatch<K, V>> {
        if self.set.is_empty() && self.remove.is_empty() {
            None
        } else {
            Some(MapPatch::Change {
                set: self.set,
                remove: self.remove,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply() {
        let mut current = Some(BTreeMap::from([("a", 1), ("b", 2)]));
        MapPatch::Change {
            set: BTreeMap::from([("c", 3)]),
            remove: BTreeSet::from(["a"]),
        }
        .apply(&mut current);
        assert_eq!(current, Some(BTreeMap::from([("b", 2), ("c", 3)])));
        MapPatch::<&str, i32>::Replace(None).apply(&mut current);
        assert_eq!(current, None);
        MapPatch::Change {
            set: BTreeMap::from([("x", 1)]),
            remove: BTreeSet::new(),
        }
        .apply(&mut current);
        assert_eq!(current, Some(BTreeMap::from([("x", 1)])));
    }
}
