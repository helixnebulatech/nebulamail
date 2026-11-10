// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded JSON: JSCalendar and JSContact bodies are not modelled here
//! (the conversion lives in `groupware`), but they come from clients, so
//! their size, depth and number of values are checked before anything
//! keeps them.
//!
//! The checks walk the value with an explicit stack (no recursion), so a
//! deep value can't overflow the stack even with limits switched off.

use serde_json::{Map, Value};

use crate::error::SetError;
use crate::references::JsonPointer;

/// Limits for one JSON value from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonLimits {
    /// Approximate size of the value written as JSON, in bytes.
    pub max_size: usize,
    /// Deepest nesting of objects and arrays (the value itself is 1).
    pub max_depth: usize,
    /// Most values (objects, arrays, strings, numbers…) in total.
    pub max_nodes: usize,
    /// Longest string or member name, in bytes.
    pub max_string: usize,
}

impl JsonLimits {
    /// A JSCalendar event: 1 MiB like an iCalendar object,
    /// 32 levels, 50 000 values.
    pub const JSCALENDAR: Self = Self {
        max_size: 1 << 20,
        max_depth: 32,
        max_nodes: 50_000,
        max_string: 1 << 20,
    };

    /// A JSContact card: 4 MiB like a vCard (with `data:` photos), 32
    /// levels, 50 000 values.
    pub const JSCONTACT: Self = Self {
        max_size: 4 << 20,
        max_depth: 32,
        max_nodes: 50_000,
        max_string: 4 << 20,
    };

    /// Small values inside typed objects (a calendar's default alerts).
    pub const SMALL: Self = Self {
        max_size: 64 << 10,
        max_depth: 8,
        max_nodes: 2_000,
        max_string: 16 << 10,
    };
}

/// Why a JSON value is over its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BoundedJsonError {
    /// Bigger than `max_size`.
    #[error("value is too large")]
    TooLarge,
    /// Deeper than `max_depth`.
    #[error("value is nested too deeply")]
    TooDeep,
    /// More than `max_nodes` values.
    #[error("value has too many members")]
    TooManyNodes,
    /// A string or member name longer than `max_string`.
    #[error("string is too long")]
    StringTooLong,
    /// A member name with control characters.
    #[error("member name has control characters")]
    BadMemberName,
}

/// Bytes a string takes in JSON, without escapes (quotes included).
fn string_size(text: &str, limits: &JsonLimits) -> Result<usize, BoundedJsonError> {
    if text.len() > limits.max_string {
        return Err(BoundedJsonError::StringTooLong);
    }
    Ok(text.len().saturating_add(2))
}

/// Checks a value nested `depth` levels down (0 for a whole value).
///
/// # Errors
/// The first limit it goes over.
pub fn check_value_at(
    value: &Value,
    depth: usize,
    limits: &JsonLimits,
) -> Result<(), BoundedJsonError> {
    measure(value, depth, limits).map(|_| ())
}

/// Checks a whole value.
///
/// # Errors
/// The first limit it goes over.
pub fn check_value(value: &Value, limits: &JsonLimits) -> Result<(), BoundedJsonError> {
    check_value_at(value, 0, limits)
}

/// Checks the members of an object as if they were the object.
///
/// # Errors
/// The first limit it goes over.
pub fn check_object(
    members: &Map<String, Value>,
    limits: &JsonLimits,
) -> Result<(), BoundedJsonError> {
    // Each member is measured with what is left of the budget.
    let mut left = *limits;
    left.max_size = left
        .max_size
        .checked_sub(2)
        .ok_or(BoundedJsonError::TooLarge)?;
    left.max_nodes = left
        .max_nodes
        .checked_sub(1)
        .ok_or(BoundedJsonError::TooManyNodes)?;
    for (name, member) in members {
        if name.chars().any(char::is_control) {
            return Err(BoundedJsonError::BadMemberName);
        }
        let name_size = string_size(name, limits)?.saturating_add(2);
        left.max_size = left
            .max_size
            .checked_sub(name_size)
            .ok_or(BoundedJsonError::TooLarge)?;
        let (size, nodes) = measure(member, 1, &left)?;
        left.max_size = left.max_size.saturating_sub(size);
        left.max_nodes = left.max_nodes.saturating_sub(nodes);
    }
    Ok(())
}

/// Size and number of values of `value`, within `limits`.
fn measure(
    value: &Value,
    depth: usize,
    limits: &JsonLimits,
) -> Result<(usize, usize), BoundedJsonError> {
    let mut size: usize = 0;
    let mut nodes: usize = 0;
    let mut stack: Vec<(&Value, usize)> = vec![(value, depth.saturating_add(1))];
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > limits.max_nodes {
            return Err(BoundedJsonError::TooManyNodes);
        }
        if depth > limits.max_depth {
            return Err(BoundedJsonError::TooDeep);
        }
        let here = match value {
            Value::Null | Value::Bool(_) => 5,
            Value::Number(_) => 24,
            Value::String(text) => string_size(text, limits)?,
            Value::Array(items) => {
                let next = depth.saturating_add(1);
                stack.extend(items.iter().map(|item| (item, next)));
                items.len().saturating_add(2)
            }
            Value::Object(members) => {
                let next = depth.saturating_add(1);
                let mut here = members.len().saturating_mul(2).saturating_add(2);
                for (name, member) in members {
                    if name.chars().any(char::is_control) {
                        return Err(BoundedJsonError::BadMemberName);
                    }
                    here = here.saturating_add(string_size(name, limits)?);
                    stack.push((member, next));
                }
                here
            }
        };
        size = size.saturating_add(here);
        if size > limits.max_size {
            return Err(BoundedJsonError::TooLarge);
        }
    }
    Ok((size, nodes))
}

/// Applies one patch (RFC 8620 §5.3) to a JSON object: sets the value at
/// `path`, or removes it if `value` is `null`. Every object on the way
/// must exist; paths never go into arrays (RFC 8620: "MUST NOT reference
/// inside an array").
///
/// # Errors
/// `invalidPatch` if the path goes through something that is not an
/// object, or a parent is missing.
pub fn apply_patch(
    document: &mut Map<String, Value>,
    path: &JsonPointer,
    value: Value,
) -> Result<(), SetError> {
    let Some((last, parents)) = path.tokens().split_last() else {
        return Err(SetError::invalid_patch("empty path"));
    };
    let mut target = document;
    for token in parents {
        target = match target.get_mut(token) {
            Some(Value::Object(next)) => next,
            Some(_) => {
                return Err(SetError::invalid_patch(format!(
                    "{path}: goes through a value that is not an object"
                )));
            }
            None => {
                return Err(SetError::invalid_patch(format!("{path}: parent missing")));
            }
        };
    }
    if value.is_null() {
        target.remove(last);
    } else {
        target.insert(last.clone(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    const TINY: JsonLimits = JsonLimits {
        max_size: 100,
        max_depth: 3,
        max_nodes: 10,
        max_string: 20,
    };

    #[test]
    fn limits() {
        assert_eq!(check_value(&json!({"a": [1, "x"]}), &TINY), Ok(()));
        assert_eq!(
            check_value(&json!({"a": {"b": {"c": 1}}}), &TINY),
            Err(BoundedJsonError::TooDeep)
        );
        assert_eq!(
            check_value(
                &json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
                &JsonLimits {
                    max_size: 10_000,
                    ..TINY
                }
            ),
            Err(BoundedJsonError::TooManyNodes)
        );
        assert_eq!(
            check_value(&json!("x".repeat(21)), &TINY),
            Err(BoundedJsonError::StringTooLong)
        );
        assert_eq!(
            check_value(&json!({"x".repeat(21): 1}), &TINY),
            Err(BoundedJsonError::StringTooLong)
        );
        assert_eq!(
            check_value(
                &json!([
                    "x".repeat(20),
                    "y".repeat(20),
                    "z".repeat(20),
                    "w".repeat(20),
                    "v".repeat(20)
                ]),
                &TINY
            ),
            Err(BoundedJsonError::TooLarge)
        );
        assert_eq!(
            check_value(&json!({"a\u{0}": 1}), &TINY),
            Err(BoundedJsonError::BadMemberName)
        );
        // Nested one level down, the same value is one level too deep.
        assert_eq!(check_value_at(&json!({"b": {"c": 1}}), 0, &TINY), Ok(()));
        assert_eq!(
            check_value_at(&json!({"b": {"c": 1}}), 1, &TINY),
            Err(BoundedJsonError::TooDeep)
        );
    }

    #[test]
    fn objects_add_up() {
        let object = json!({"a": "x".repeat(20), "b": "y".repeat(20), "c": "z".repeat(20)});
        let Value::Object(members) = object else {
            unreachable!()
        };
        assert_eq!(check_object(&members, &TINY), Ok(()));
        let mut more = members.clone();
        more.insert("d".to_owned(), json!("w".repeat(20)));
        more.insert("e".to_owned(), json!("v".repeat(20)));
        assert_eq!(check_object(&more, &TINY), Err(BoundedJsonError::TooLarge));
        let Value::Object(deep) = json!({"a": {"b": {"c": 1}}}) else {
            unreachable!()
        };
        assert_eq!(check_object(&deep, &TINY), Err(BoundedJsonError::TooDeep));
        let Value::Object(many) = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5, "f": 6, "g": 7, "h": 8, "i": 9, "j": 10, "k": 11})
        else {
            unreachable!()
        };
        let roomy = JsonLimits {
            max_size: 10_000,
            ..TINY
        };
        assert_eq!(
            check_object(&many, &roomy),
            Err(BoundedJsonError::TooManyNodes)
        );
    }

    #[test]
    fn deep_values_do_not_overflow_the_stack() {
        let mut deep = json!(1);
        for _ in 0..10_000 {
            deep = Value::Array(vec![deep]);
        }
        let unlimited = JsonLimits {
            max_size: usize::MAX,
            max_depth: usize::MAX,
            max_nodes: usize::MAX,
            max_string: usize::MAX,
        };
        assert_eq!(check_value(&deep, &unlimited), Ok(()));
        assert_eq!(
            check_value(&deep, &JsonLimits::JSCALENDAR),
            Err(BoundedJsonError::TooDeep)
        );
        // serde_json's own drop is recursive: take it apart by hand.
        let mut current = deep;
        while let Value::Array(mut items) = current {
            current = items.pop().unwrap_or(Value::Null);
        }
    }

    #[test]
    fn patches() {
        let Value::Object(mut event) = json!({
            "title": "x",
            "alerts": {"a1": {"trigger": {"offset": "-PT10M"}}},
            "keywords": ["a"]
        }) else {
            unreachable!()
        };
        let path = |text: &str| JsonPointer::from_patch_key(text).unwrap();
        apply_patch(
            &mut event,
            &path("alerts/a1/acknowledged"),
            json!("2026-10-26T20:49:00Z"),
        )
        .unwrap();
        apply_patch(&mut event, &path("alerts/a2"), json!({"action": "display"})).unwrap();
        apply_patch(&mut event, &path("title"), Value::Null).unwrap();
        assert_eq!(
            Value::Object(event.clone()),
            json!({
                "alerts": {
                    "a1": {"trigger": {"offset": "-PT10M"}, "acknowledged": "2026-10-26T20:49:00Z"},
                    "a2": {"action": "display"}
                },
                "keywords": ["a"]
            })
        );
        for bad in ["missing/x", "keywords/0", "alerts/a1/trigger/offset/x"] {
            assert_eq!(
                apply_patch(&mut event, &path(bad), json!(1))
                    .unwrap_err()
                    .kind(),
                SetErrorType::InvalidPatch,
                "{bad}"
            );
        }
        // Removing something that isn't there is fine.
        apply_patch(&mut event, &path("alerts/zz"), Value::Null).unwrap();
    }
}
