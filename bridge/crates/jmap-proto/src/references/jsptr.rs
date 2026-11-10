// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JSON Pointer (RFC 6901) with the JMAP `*` wildcard (RFC 8620 §3.7).

use std::fmt;
use std::str::FromStr;

use serde_json::Value;

use super::budget::{OverBudget, ReferenceBudget};
use crate::macros::serde_via_str;

/// Why a JSON Pointer could not be parsed or evaluated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PointerError {
    /// A non-empty pointer must start with `/`.
    #[error("JSON Pointer must start with \"/\"")]
    MissingSlash,
    /// `~` not followed by `0` or `1`.
    #[error("invalid \"~\" escape in JSON Pointer")]
    InvalidEscape,
    /// More than [`JsonPointer::MAX_TOKENS`] tokens.
    #[error("JSON Pointer is too long")]
    TooLong,
    /// The pointer does not lead to a value in the document.
    #[error("path {0:?} not found")]
    NotFound(String),
}

/// A parsed JSON Pointer: its reference tokens, unescaped (`~1` → `/`,
/// `~0` → `~`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct JsonPointer {
    tokens: Vec<String>,
}

impl JsonPointer {
    /// Most tokens accepted. JMAP paths are short (`/list/*/threadId`); the
    /// limit stops pathological input from a client.
    pub const MAX_TOKENS: usize = 32;

    /// The empty pointer: the whole document.
    #[must_use]
    pub fn root() -> Self {
        Self { tokens: Vec::new() }
    }

    /// Parses a `PatchObject` key, which is a pointer without the leading
    /// `/` (RFC 8620 §5.3).
    ///
    /// # Errors
    /// See [`PointerError`].
    pub fn from_patch_key(key: &str) -> Result<Self, PointerError> {
        parse_tokens(key)
    }

    /// The unescaped reference tokens.
    #[must_use]
    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }

    /// Whether `self` is a (non-strict) prefix of `other`.
    #[must_use]
    pub fn is_prefix_of(&self, other: &Self) -> bool {
        other.tokens.starts_with(&self.tokens)
    }

    /// Evaluates the pointer on `document`, with the RFC 8620 §3.7
    /// extension: on an array, the token `*` applies the rest of the
    /// pointer to every item and collects the results in a new array,
    /// flattening items that are themselves arrays and leaving out `null`.
    ///
    /// # Errors
    /// [`PointerError::NotFound`] if a token does not match (missing key,
    /// index out of range, `*` on a non-array, key on a scalar…).
    pub fn evaluate(&self, document: &Value) -> Result<Value, PointerError> {
        evaluate(document, &self.tokens).ok_or_else(|| PointerError::NotFound(self.to_string()))
    }
}

fn parse_tokens(rest: &str) -> Result<JsonPointer, PointerError> {
    let tokens = rest
        .split('/')
        .map(unescape)
        .collect::<Result<Vec<_>, _>>()?;
    if tokens.len() > JsonPointer::MAX_TOKENS {
        return Err(PointerError::TooLong);
    }
    Ok(JsonPointer { tokens })
}

fn unescape(token: &str) -> Result<String, PointerError> {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some('0') => out.push('~'),
                Some('1') => out.push('/'),
                _ => return Err(PointerError::InvalidEscape),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// RFC 6901 array index: `0` or digits without a leading zero.
fn array_index(token: &str) -> Option<usize> {
    let valid = token == "0"
        || (!token.is_empty()
            && !token.starts_with('0')
            && token.bytes().all(|b| b.is_ascii_digit()));
    if valid { token.parse().ok() } else { None }
}

fn evaluate(value: &Value, tokens: &[String]) -> Option<Value> {
    let Some((token, rest)) = tokens.split_first() else {
        return Some(value.clone());
    };
    match value {
        Value::Array(items) if token == "*" => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match evaluate(item, rest)? {
                    Value::Array(inner) => out.extend(inner),
                    // A `null` is left out, as in Stalwart: `baseEventId` of
                    // an event that doesn't repeat, under `#ids`.
                    Value::Null => {}
                    other => out.push(other),
                }
            }
            Some(Value::Array(out))
        }
        Value::Array(items) => evaluate(items.get(array_index(token)?)?, rest),
        Value::Object(map) => evaluate(map.get(token.as_str())?, rest),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

/// Like [`evaluate`], but every value copied is paid from `budget` before
/// it is copied, inside `*` too: `Ok(None)` if the path doesn't match,
/// `Err` if the copies would pass the budget.
pub(crate) fn evaluate_bounded(
    value: &Value,
    tokens: &[String],
    budget: &mut ReferenceBudget,
) -> Result<Option<Value>, OverBudget> {
    let Some((token, rest)) = tokens.split_first() else {
        budget.charge(value)?;
        return Ok(Some(value.clone()));
    };
    match value {
        Value::Array(items) if token == "*" => {
            budget.charge_node()?;
            let mut out = Vec::new();
            for item in items {
                match evaluate_bounded(item, rest, budget)? {
                    None => return Ok(None),
                    Some(Value::Array(inner)) => out.extend(inner),
                    // A `null` is left out, as in [`evaluate`].
                    Some(Value::Null) => {}
                    Some(other) => out.push(other),
                }
            }
            Ok(Some(Value::Array(out)))
        }
        Value::Array(items) => match array_index(token).and_then(|index| items.get(index)) {
            Some(item) => evaluate_bounded(item, rest, budget),
            None => Ok(None),
        },
        Value::Object(map) => match map.get(token.as_str()) {
            Some(item) => evaluate_bounded(item, rest, budget),
            None => Ok(None),
        },
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(None),
    }
}

impl FromStr for JsonPointer {
    type Err = PointerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::root());
        }
        let rest = s.strip_prefix('/').ok_or(PointerError::MissingSlash)?;
        parse_tokens(rest)
    }
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for token in &self.tokens {
            write!(f, "/{}", escape(token))?;
        }
        Ok(())
    }
}

impl fmt::Debug for JsonPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsonPointer({self})")
    }
}

serde_via_str!(JsonPointer);

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn eval(pointer: &str, document: &Value) -> Option<Value> {
        pointer
            .parse::<JsonPointer>()
            .unwrap()
            .evaluate(document)
            .ok()
    }

    /// RFC 6901 §5.
    #[test]
    fn rfc_6901_examples() {
        let doc = json!({
            "foo": ["bar", "baz"], "": 0, "a/b": 1, "c%d": 2, "e^f": 3,
            "g|h": 4, "i\\j": 5, "k\"l": 6, " ": 7, "m~n": 8
        });
        let cases = [
            ("", doc.clone()),
            ("/foo", json!(["bar", "baz"])),
            ("/foo/0", json!("bar")),
            ("/", json!(0)),
            ("/a~1b", json!(1)),
            ("/c%d", json!(2)),
            ("/e^f", json!(3)),
            ("/g|h", json!(4)),
            ("/i\\j", json!(5)),
            ("/k\"l", json!(6)),
            ("/ ", json!(7)),
            ("/m~0n", json!(8)),
        ];
        for (pointer, expected) in cases {
            assert_eq!(eval(pointer, &doc), Some(expected), "{pointer}");
        }
    }

    /// RFC 8620 §3.7: the responses of the Email/query → Email/get →
    /// Thread/get → Email/get example.
    #[test]
    fn rfc_8620_wildcard_examples() {
        let query = json!({
            "accountId": "A1", "queryState": "abcdefg", "canCalculateChanges": true,
            "position": 0, "total": 101,
            "ids": ["msg1023", "msg223", "msg110", "msg93", "msg91",
                    "msg38", "msg36", "msg33", "msg11", "msg1"]
        });
        assert_eq!(eval("/ids", &query), Some(query["ids"].clone()));
        let get = json!({
            "accountId": "A1", "state": "123456",
            "list": [
                {"id": "msg1023", "threadId": "trd194"},
                {"id": "msg223", "threadId": "trd114"}
            ],
            "notFound": []
        });
        assert_eq!(
            eval("/list/*/threadId", &get),
            Some(json!(["trd194", "trd114"]))
        );
        let threads = json!({
            "accountId": "A1", "state": "123456",
            "list": [
                {"id": "trd194", "emailIds": ["msg1020", "msg1021", "msg1023"]},
                {"id": "trd114", "emailIds": ["msg201", "msg223"]}
            ],
            "notFound": []
        });
        // Arrays of arrays are flattened.
        assert_eq!(
            eval("/list/*/emailIds", &threads),
            Some(json!(["msg1020", "msg1021", "msg1023", "msg201", "msg223"]))
        );
        // A `null` is not an id: the event that is its own series.
        let events = json!({"list": [
            {"id": "v1_20261019T100000", "baseEventId": "v1"},
            {"id": "v2", "baseEventId": null}
        ]});
        assert_eq!(eval("/list/*/baseEventId", &events), Some(json!(["v1"])));
        let singles = json!({"list": [{"id": "v2", "baseEventId": null}]});
        assert_eq!(eval("/list/*/baseEventId", &singles), Some(json!([])));
    }

    #[test]
    fn failures() {
        let doc = json!({"list": [{"a": 1}, {"b": 2}], "n": 1});
        assert_eq!(eval("/missing", &doc), None);
        assert_eq!(eval("/list/*/a", &doc), None, "one item lacks the key");
        assert_eq!(eval("/n/*", &doc), None, "wildcard on a number");
        assert_eq!(eval("/list/01", &doc), None, "leading zero");
        assert_eq!(eval("/list/-", &doc), None);
        assert_eq!(eval("/list/2", &doc), None);
        assert_eq!(eval("/n/x", &doc), None);
        assert_eq!(
            "ids".parse::<JsonPointer>(),
            Err(PointerError::MissingSlash)
        );
        assert_eq!(
            "/a~2".parse::<JsonPointer>(),
            Err(PointerError::InvalidEscape)
        );
        assert_eq!(
            "/a~".parse::<JsonPointer>(),
            Err(PointerError::InvalidEscape)
        );
        assert_eq!(
            "/a".repeat(33).parse::<JsonPointer>(),
            Err(PointerError::TooLong)
        );
    }

    #[test]
    fn patch_keys() {
        let key = JsonPointer::from_patch_key("keywords/$seen").unwrap();
        assert_eq!(key.tokens(), ["keywords", "$seen"]);
        let escaped = JsonPointer::from_patch_key("keywords/a~1b~0c").unwrap();
        assert_eq!(escaped.tokens(), ["keywords", "a/b~c"]);
        assert!(
            JsonPointer::from_patch_key("keywords")
                .unwrap()
                .is_prefix_of(&escaped)
        );
    }

    proptest! {
        #[test]
        fn display_round_trips(tokens in proptest::collection::vec("\\PC{0,8}", 0..6)) {
            let pointer = JsonPointer { tokens };
            let reparsed: JsonPointer = pointer.to_string().parse().unwrap();
            prop_assert_eq!(reparsed, pointer);
        }

        #[test]
        fn parse_never_panics(s in "\\PC{0,64}") {
            let _ = s.parse::<JsonPointer>();
        }
    }
}
