// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! How much result references may copy in one request. A reference copies
//! (part of) an earlier response, and the copy can be referenced again:
//! with `Core/echo`, a few calls of 100 references each grow a 1 KB
//! request into gigabytes. The budget is per request, shared by all its
//! calls, and counts memory rather than JSON text: `[[],[],…]` is 3 bytes
//! per item as JSON but a whole [`Value`] in memory.

use std::mem::size_of;

use serde_json::{Map, Value};

/// What each value costs, besides its text.
const NODE: usize = size_of::<Value>();
/// What each object key costs, besides its text.
const KEY: usize = size_of::<String>();

/// The copies would pass the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverBudget;

/// What the result references of one request may still copy, in bytes of
/// memory (RFC 8620 §3.7; the server uses `maxSizeRequest`: a request
/// shouldn't make more data than it may send).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceBudget {
    remaining: usize,
}

impl ReferenceBudget {
    /// A budget of `bytes`.
    #[must_use]
    pub const fn new(bytes: usize) -> Self {
        Self { remaining: bytes }
    }

    /// What is left.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.remaining
    }

    /// Pays for copying `value` (all of it), before it is copied.
    pub(crate) fn charge(&mut self, value: &Value) -> Result<(), OverBudget> {
        self.pay(0, std::iter::once(value))
    }

    /// Pays for copying an arguments object (all of it).
    pub(crate) fn charge_map(&mut self, map: &Map<String, Value>) -> Result<(), OverBudget> {
        let keys = map.keys().fold(NODE, |sum, key| {
            sum.saturating_add(KEY.saturating_add(key.len()))
        });
        self.pay(keys, map.values())
    }

    /// Pays for one new empty value (the array a `*` builds).
    pub(crate) fn charge_node(&mut self) -> Result<(), OverBudget> {
        self.pay(NODE, std::iter::empty())
    }

    /// Pays `base` plus every value under `roots`, stopping as soon as the
    /// total passes what is left (so measuring a huge value is cheap too).
    fn pay<'a>(
        &mut self,
        base: usize,
        roots: impl Iterator<Item = &'a Value>,
    ) -> Result<(), OverBudget> {
        let limit = self.remaining;
        let mut total = base;
        let mut stack: Vec<&Value> = roots.collect();
        if total > limit {
            return Err(OverBudget);
        }
        while let Some(value) = stack.pop() {
            total = total.saturating_add(NODE);
            match value {
                Value::String(text) => total = total.saturating_add(text.len()),
                Value::Array(items) => {
                    // Each item costs at least a node: check before
                    // stacking a huge array.
                    if total.saturating_add(items.len().saturating_mul(NODE)) > limit {
                        return Err(OverBudget);
                    }
                    stack.extend(items);
                }
                Value::Object(map) => {
                    if total.saturating_add(map.len().saturating_mul(NODE)) > limit {
                        return Err(OverBudget);
                    }
                    for (key, item) in map {
                        total = total.saturating_add(KEY.saturating_add(key.len()));
                        stack.push(item);
                    }
                }
                Value::Null | Value::Bool(_) | Value::Number(_) => {}
            }
            if total > limit {
                return Err(OverBudget);
            }
        }
        self.remaining = limit.saturating_sub(total);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn counts_memory_not_text() {
        let mut budget = ReferenceBudget::new(1_000_000);
        let empties = Value::Array(vec![json!([]); 1000]);
        budget.charge(&empties).unwrap();
        // 1001 values, not ~3 KB of text.
        assert!(1_000_000 - budget.remaining() >= 1001 * NODE);
        let text = json!({"key": "x".repeat(500)});
        let before = budget.remaining();
        budget.charge(&text).unwrap();
        assert!(before - budget.remaining() >= 503);
    }

    #[test]
    fn stops_at_the_limit_and_charges_nothing() {
        let mut budget = ReferenceBudget::new(1000);
        let big = Value::Array(vec![json!(1); 1000]);
        assert_eq!(budget.charge(&big), Err(OverBudget));
        assert_eq!(budget.remaining(), 1000);
        budget.charge(&json!("small")).unwrap();
        assert!(budget.remaining() < 1000);
    }
}
