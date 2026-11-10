// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command tags.

use std::fmt;

use super::is_astring_char;
use crate::ValueError;

/// A command tag: one or more `ASTRING-CHAR` except `+` (RFC 3501 §9).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag(Box<str>);

impl Tag {
    /// Validates a tag, for example one received from the server.
    ///
    /// # Errors
    ///
    /// Empty tags or tags with characters outside the grammar.
    pub fn new(text: &str) -> Result<Self, ValueError> {
        if text.is_empty() {
            return Err(ValueError::Empty("tag"));
        }
        if !text
            .bytes()
            .all(|byte| is_astring_char(byte) && byte != b'+')
        {
            return Err(ValueError::Invalid("tag"));
        }
        Ok(Self(text.into()))
    }

    /// The tag as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Generates unique tags for one connection: `A0001`, `A0002`…
///
/// Tags only need to be unique among the commands in flight, so the counter
/// wraps around after 2^32 commands.
#[derive(Debug, Clone, Default)]
pub struct TagGenerator {
    next: u32,
}

impl TagGenerator {
    /// A generator that starts at `A0001`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The next tag.
    pub fn next_tag(&mut self) -> Tag {
        self.next = self.next.wrapping_add(1);
        // Digits and a letter are always valid tag characters.
        Tag(format!("A{:04}", self.next).into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tags_are_valid_and_unique() {
        let mut tags = TagGenerator::new();
        let first = tags.next_tag();
        let second = tags.next_tag();
        assert_eq!(first.as_str(), "A0001");
        assert_ne!(first, second);
        assert!(Tag::new(second.as_str()).is_ok());
    }

    #[test]
    fn rejects_bad_tags() {
        assert!(Tag::new("").is_err());
        assert!(Tag::new("A+1").is_err());
        assert!(Tag::new("A 1").is_err());
        assert!(Tag::new("*").is_err());
    }
}
