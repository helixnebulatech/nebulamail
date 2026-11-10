// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `PatchObjects` (RFC 8620 §5.3): `{"keywords/$seen": true, "name": "x"}`.
//!
//! This module checks the generic rules (valid pointers, no pointer that is
//! a prefix of another). Each object then turns the patches into its own
//! typed patch (`EmailPatch`, `MailboxPatch`…), rejecting unknown or
//! immutable properties.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::SetError;
use crate::references::JsonPointer;

/// An update as sent by the client: keys are JSON Pointers without the
/// leading `/`; `null` values reset/remove.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PatchObject(Map<String, Value>);

/// One patch: set the value at `path` (`null` = remove or default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// The key as sent (for error messages and `properties` lists).
    pub key: String,
    /// Where to apply it.
    pub path: JsonPointer,
    /// The new value.
    pub value: Value,
}

/// What a patch path points at, for objects whose properties are at most
/// one level deep (all of RFC 8621's patchable properties).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target<'a> {
    /// The whole property: `name`.
    Property(&'a str),
    /// One member of a map property: `keywords/$seen`.
    Member(&'a str, &'a str),
    /// Anything deeper (`a/b/c`).
    Deeper(&'a str),
}

impl<'a> Target<'a> {
    pub(crate) fn of(path: &'a JsonPointer) -> Self {
        match path.tokens() {
            [property] => Target::Property(property),
            [property, member] => Target::Member(property, member),
            [property, ..] => Target::Deeper(property),
            // A patch key always has at least one token.
            [] => Target::Property(""),
        }
    }
}

impl PatchObject {
    /// Wraps a JSON object.
    #[must_use]
    pub fn new(map: Map<String, Value>) -> Self {
        Self(map)
    }

    /// The raw object.
    #[must_use]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// The patches, checked against the generic RFC 8620 §5.3 rules.
    ///
    /// # Errors
    /// `invalidPatch` if a key is not a valid pointer, or one pointer is
    /// a prefix of another (`keywords` and `keywords/$seen`).
    pub fn patches(&self) -> Result<Vec<Patch>, SetError> {
        let mut patches = self
            .0
            .iter()
            .map(|(key, value)| {
                JsonPointer::from_patch_key(key)
                    .map(|path| Patch {
                        key: key.clone(),
                        path,
                        value: value.clone(),
                    })
                    .map_err(|error| SetError::invalid_patch(format!("{key:?}: {error}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Sorted by tokens, a pointer is a prefix of another only if it is a
        // prefix of the next one, so adjacent pairs are enough.
        patches.sort_by(|a, b| a.path.tokens().cmp(b.path.tokens()));
        for pair in patches.windows(2) {
            if let [first, second] = pair
                && first.path.is_prefix_of(&second.path)
            {
                return Err(SetError::invalid_patch(format!(
                    "{:?} overlaps {:?}",
                    first.key, second.key
                )));
            }
        }
        Ok(patches)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn patch(value: Value) -> PatchObject {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn targets() {
        let patches = patch(json!({"name": "x", "keywords/$seen": true, "a/b/c": 1}))
            .patches()
            .unwrap();
        let targets: Vec<_> = patches.iter().map(|p| Target::of(&p.path)).collect();
        assert_eq!(
            targets,
            [
                Target::Deeper("a"),
                Target::Member("keywords", "$seen"),
                Target::Property("name")
            ]
        );
    }

    #[test]
    fn escapes() {
        let patches = patch(json!({"mailboxIds/a~1b": true, "keywords/x~0y": null}))
            .patches()
            .unwrap();
        assert_eq!(
            Target::of(&patches[0].path),
            Target::Member("keywords", "x~y")
        );
        assert_eq!(
            Target::of(&patches[1].path),
            Target::Member("mailboxIds", "a/b")
        );
    }

    #[test]
    fn rejects_overlaps_and_bad_pointers() {
        for bad in [
            json!({"keywords": {}, "keywords/$seen": true}),
            json!({"a/b": 1, "a/b/c": 2, "z": 0}),
            json!({"keywords/bad~2": true}),
        ] {
            assert_eq!(
                patch(bad).patches().unwrap_err().kind(),
                SetErrorType::InvalidPatch
            );
        }
        // Siblings are fine.
        assert!(
            patch(json!({"keywords/a": true, "keywords/ab": true}))
                .patches()
                .is_ok()
        );
    }
}
