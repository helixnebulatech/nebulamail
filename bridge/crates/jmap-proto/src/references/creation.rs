// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! References to objects created earlier in the same request: `"#k1"` in
//! place of an id (RFC 8620 §5.3).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::types::{CreationId, Id, IdError, IdKind};

/// An id, or `#creationId` for an object created earlier in this request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum IdOrRef<T> {
    /// An existing object's id.
    Id(T),
    /// `#creationId`: the object created with this creation id.
    Ref(CreationId),
}

impl<T: IdKind> IdOrRef<T> {
    /// The id, looking up a creation reference in `created`.
    ///
    /// # Errors
    /// [`UnknownCreationId`] if nothing was created with that creation id.
    pub fn resolve(&self, created: &CreatedIds) -> Result<T, UnknownCreationId> {
        match self {
            Self::Id(id) => Ok(id.clone()),
            Self::Ref(creation_id) => created
                .get(creation_id)
                .cloned()
                .map(T::from)
                .ok_or_else(|| UnknownCreationId(creation_id.clone())),
        }
    }
}

impl<T: FromStr<Err = IdError>> FromStr for IdOrRef<T> {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.strip_prefix('#') {
            Some(creation_id) => creation_id.parse().map(Self::Ref),
            None => s.parse().map(Self::Id),
        }
    }
}

impl<T: fmt::Display> fmt::Display for IdOrRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => id.fmt(f),
            Self::Ref(creation_id) => write!(f, "#{creation_id}"),
        }
    }
}

impl<T: fmt::Display> Serialize for IdOrRef<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de, T: FromStr<Err = IdError>> Deserialize<'de> for IdOrRef<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A `#creationId` that does not match anything created in the request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown creation id #{0}")]
pub struct UnknownCreationId(pub CreationId);

/// The map of creation id → id of everything created so far in a request
/// (RFC 8620 §3.3 `createdIds`, §5.3). It may start with the request's
/// `createdIds` and is returned in the response.
///
/// Creation ids are not scoped by type; reusing one maps it to the most
/// recent object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CreatedIds(BTreeMap<CreationId, Id>);

impl CreatedIds {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `creation_id` created the object `id`.
    pub fn insert(&mut self, creation_id: CreationId, id: impl Into<Id>) {
        self.0.insert(creation_id, id.into());
    }

    /// The id created with `creation_id`, if any.
    #[must_use]
    pub fn get(&self, creation_id: &CreationId) -> Option<&Id> {
        self.0.get(creation_id)
    }

    /// Whether nothing was created.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// All entries, ordered by creation id.
    pub fn iter(&self) -> impl Iterator<Item = (&CreationId, &Id)> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EmailId, MailboxId};

    #[test]
    fn parses_ids_and_references() {
        let id: IdOrRef<MailboxId> = "m1".parse().unwrap();
        assert_eq!(id, IdOrRef::Id("m1".parse().unwrap()));
        let reference: IdOrRef<MailboxId> = "#k1".parse().unwrap();
        assert_eq!(reference, IdOrRef::Ref("k1".parse().unwrap()));
        assert_eq!(reference.to_string(), "#k1");
        assert!("#".parse::<IdOrRef<MailboxId>>().is_err());
        assert!("##k".parse::<IdOrRef<MailboxId>>().is_err());
    }

    #[test]
    fn resolves_through_created_ids() {
        let mut created = CreatedIds::new();
        created.insert("draft".parse().unwrap(), "e42".parse::<Id>().unwrap());
        let reference: IdOrRef<EmailId> = "#draft".parse().unwrap();
        assert_eq!(reference.resolve(&created).unwrap().as_str(), "e42");
        let missing: IdOrRef<EmailId> = "#other".parse().unwrap();
        assert!(missing.resolve(&created).is_err());
        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            serde_json::json!({"draft": "e42"})
        );
    }
}
