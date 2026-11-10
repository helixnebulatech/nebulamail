// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Strict JSON: like `serde_json::Value`, but an object with the same
//! member name twice is an error.
//!
//! I-JSON (RFC 7493 §2.3), which JMAP requires (RFC 8620 §3.3), forbids
//! duplicate names. `serde_json` keeps the last one silently, so
//! `{"accountId": "a1", "accountId": "a2"}` would mean whatever the last
//! parser in a chain decides: a proxy or a log could see `a1` while we act
//! on `a2`. Parsing goes through [`StrictValue`] instead, which keeps
//! `serde_json`'s own limits (nesting depth 128) and rejects duplicates at
//! any depth.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// A JSON value whose objects have no duplicate member names.
#[derive(Debug, Clone, PartialEq)]
pub struct StrictValue(Value);

impl StrictValue {
    /// The value.
    #[must_use]
    pub fn into_inner(self) -> Value {
        self.0
    }
}

/// Parses JSON text, rejecting duplicate member names in any object.
///
/// # Errors
/// A `serde_json` error for invalid JSON, trailing data or a duplicate
/// name.
pub fn from_slice_strict(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<StrictValue>(bytes).map(StrictValue::into_inner)
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<StrictValue, E> {
        Number::from_f64(value)
            .map(|n| StrictValue(Value::Number(n)))
            .ok_or_else(|| E::custom("number out of range"))
    }

    fn visit_str<E>(self, value: &str) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_unit<E>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_none<E>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<StrictValue, D::Error> {
        StrictValue::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StrictValue, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(StrictValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StrictValue, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format_args!(
                    "duplicate member name {:?}",
                    Echo(&key)
                )));
            }
            let StrictValue(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(StrictValue(Value::Object(object)))
    }
}

/// At most 64 characters of a client string in an error message.
struct Echo<'a>(&'a str);

impl fmt::Debug for Echo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short: String = self.0.chars().take(64).collect();
        fmt::Debug::fmt(&short, f)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    #[test]
    fn same_as_serde_json_without_duplicates() {
        let text =
            br#"{"a": [1, -2, 3.5, true, null, "x"], "b": {"c": {}}, "d": 18446744073709551615}"#;
        assert_eq!(
            from_slice_strict(text).unwrap(),
            serde_json::from_slice::<Value>(text).unwrap()
        );
    }

    #[test]
    fn duplicates_are_rejected_at_any_depth() {
        for text in [
            r#"{"a": 1, "a": 2}"#,
            r#"{"x": [{"b": 1, "c": 2, "b": 3}]}"#,
            r#"{"using": [], "methodCalls": [], "using": []}"#,
            // Escapes don't hide a duplicate: both are "a".
            r#"{"a": 1, "a": 2}"#,
        ] {
            let error = from_slice_strict(text.as_bytes()).unwrap_err();
            assert!(error.to_string().contains("duplicate"), "{text}: {error}");
        }
        // The same name in different objects is fine.
        assert_eq!(
            from_slice_strict(br#"{"a": {"a": 1}, "b": [{"a": 2}, {"a": 3}]}"#).unwrap(),
            json!({"a": {"a": 1}, "b": [{"a": 2}, {"a": 3}]})
        );
    }

    #[test]
    fn keeps_the_depth_limit_and_rejects_trailing_data() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(from_slice_strict(deep.as_bytes()).is_err());
        assert!(from_slice_strict(b"{} {}").is_err());
        assert!(from_slice_strict(b"\xff").is_err());
    }

    #[test]
    fn long_names_are_cut_in_the_error() {
        let name = "n".repeat(1000);
        let text = format!(r#"{{"{name}": 1, "{name}": 2}}"#);
        let error = from_slice_strict(text.as_bytes()).unwrap_err().to_string();
        assert!(error.len() < 200, "{error}");
    }

    proptest! {
        #[test]
        fn agrees_with_serde_json(text in "[\\[\\]{}\":,0-9a-c \\-.e]{0,64}") {
            let strict = from_slice_strict(text.as_bytes());
            let loose = serde_json::from_slice::<Value>(text.as_bytes());
            match (strict, loose) {
                (Ok(a), Ok(b)) => prop_assert_eq!(a, b),
                (Ok(_), Err(_)) => prop_assert!(false, "strict accepted what serde_json rejects"),
                (Err(_), _) => {}
            }
        }
    }
}
