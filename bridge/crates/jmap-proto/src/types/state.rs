// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Opaque strings: state strings and method call ids.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A JMAP state string (RFC 8620 §5.1, §5.2): opaque to the client, made by
/// the server. The bridge only compares states for equality.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct State(String);

impl State {
    /// Wraps a state string made by the server.
    #[must_use]
    pub fn new(state: impl Into<String>) -> Self {
        Self(state.into())
    }

    /// The state as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("State").field(&self.0).finish()
    }
}

/// The client's id for one method call (RFC 8620 §3.2), echoed in the
/// response. Any string; not an [`Id`](crate::Id) (e.g. `"#1"` is valid).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallId(String);

impl CallId {
    /// Wraps a method call id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The call id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CallId").field(&self.0).finish()
    }
}
