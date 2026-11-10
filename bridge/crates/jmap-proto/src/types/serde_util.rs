// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Serde helpers for JMAP's `null` rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// For `Option<Option<T>>` fields: missing → `None`, `null` →
/// `Some(None)`, a value → `Some(Some(v))`. Use with `#[serde(default)]`.
#[expect(
    clippy::option_option,
    reason = "JMAP distinguishes missing from null, which is exactly this type"
)]
pub(crate) fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// For arguments with a default: `null` means the default too.
pub(crate) fn null_as_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + Default,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// RFC 8620 `/set` responses: an empty map is written as `null`.
pub(crate) fn empty_map_as_null<K, V, S>(
    map: &BTreeMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    K: Serialize,
    V: Serialize,
    S: Serializer,
{
    if map.is_empty() {
        serializer.serialize_none()
    } else {
        map.serialize(serializer)
    }
}

/// RFC 8620 `/set` responses: an empty list is written as `null`.
// `serialize_with` passes `&Vec<T>`, not `&[T]`.
#[allow(clippy::ptr_arg, reason = "serde's serialize_with signature")]
pub(crate) fn empty_vec_as_null<T, S>(list: &Vec<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    if list.is_empty() {
        serializer.serialize_none()
    } else {
        list.serialize(serializer)
    }
}
