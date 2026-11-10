// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helpers to turn JSON arguments and objects into typed values, with the
//! JMAP error each failure maps to.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};

/// Deserializes a whole arguments object; any failure is
/// `invalidArguments` with serde's message as the description.
pub(crate) fn from_arguments<T: DeserializeOwned>(
    arguments: Map<String, Value>,
) -> Result<T, MethodError> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|error| MethodError::invalid_arguments(error.to_string()))
}

/// Removes and deserializes one argument. Missing or `null` → `None`.
pub(crate) fn take_argument<T: DeserializeOwned>(
    arguments: &mut Map<String, Value>,
    name: &str,
) -> Result<Option<T>, MethodError> {
    match arguments.remove(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(|error| MethodError::invalid_arguments(format!("{name}: {error}"))),
    }
}

/// Collects the invalid properties of an object being created or patched,
/// to report them all at once in one `invalidProperties` `SetError`
/// (RFC 8620 §5.3 asks for *all* of them).
#[derive(Debug, Default)]
pub(crate) struct PropertyErrors {
    invalid: Vec<String>,
    descriptions: Vec<String>,
}

impl PropertyErrors {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Deserializes `value` as property `name`; on failure records the
    /// property and returns `None`.
    pub(crate) fn parse<T: DeserializeOwned>(&mut self, name: &str, value: Value) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                self.invalid_because(name, error.to_string());
                None
            }
        }
    }

    /// Records an invalid property.
    pub(crate) fn invalid(&mut self, name: impl Into<String>) {
        self.invalid.push(name.into());
    }

    /// Records an invalid property and why.
    pub(crate) fn invalid_because(&mut self, name: impl Into<String>, why: impl Into<String>) {
        let name = name.into();
        self.descriptions.push(format!("{name}: {}", why.into()));
        self.invalid.push(name);
    }

    /// Records `name` as invalid if `value` is `None` (a required
    /// property). Returns the value.
    pub(crate) fn required<T>(&mut self, name: &str, value: Option<T>) -> Option<T> {
        if value.is_none() {
            self.invalid_because(name, "required");
        }
        value
    }

    /// `Ok` if nothing was invalid, else an `invalidProperties` `SetError`.
    pub(crate) fn finish(self) -> Result<(), SetError> {
        if self.invalid.is_empty() {
            return Ok(());
        }
        // A property can be both badly typed and required: list it once.
        let mut invalid: Vec<String> = Vec::with_capacity(self.invalid.len());
        for name in self.invalid {
            if !invalid.contains(&name) {
                invalid.push(name);
            }
        }
        let error = SetError::invalid_properties(invalid);
        Err(if self.descriptions.is_empty() {
            error
        } else {
            error.with_description(self.descriptions.join("; "))
        })
    }
}
