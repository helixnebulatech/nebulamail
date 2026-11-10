// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Script names (RFC 5804 §1.6).

use std::fmt;

use crate::NameError;

/// A valid script name: UTF-8, 1 to [`ScriptName::MAX_LEN`] bytes, without
/// control characters (U+0000–U+001F, U+007F–U+009F) or the line and
/// paragraph separators (U+2028, U+2029). Such a name always fits in a
/// quoted string, so it can never break a command line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptName(String);

impl ScriptName {
    /// Longest name in bytes. RFC 5804 sets none; JMAP's
    /// `maxSizeScriptName` (RFC 9661) suggests at least 512.
    pub const MAX_LEN: usize = 512;

    /// Checks a name.
    ///
    /// # Errors
    ///
    /// The name breaks one of the rules above.
    pub fn new(name: impl Into<String>) -> Result<Self, NameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        if name.len() > Self::MAX_LEN {
            return Err(NameError::TooLong);
        }
        if name.chars().any(is_forbidden) {
            return Err(NameError::ControlCharacter);
        }
        Ok(Self(name))
    }

    /// Checks a name sent as bytes (by a server).
    ///
    /// # Errors
    ///
    /// Not UTF-8, or one of [`ScriptName::new`]'s errors.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NameError> {
        let text = std::str::from_utf8(bytes).map_err(|_| NameError::NotUtf8)?;
        Self::new(text)
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name, owned.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

fn is_forbidden(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
}

impl fmt::Display for ScriptName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for ScriptName {
    type Error = NameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        for good in [
            "vacation",
            "nebula-mail-filters",
            "Ünïcödé 名前",
            "a\"b\\c",
            "x y",
        ] {
            assert_eq!(ScriptName::new(good).map(|n| n.0), Ok(good.to_owned()));
        }
        assert_eq!(ScriptName::new(""), Err(NameError::Empty));
        for bad in [
            "a\r\nb",
            "a\0",
            "tab\there",
            "\u{7f}",
            "\u{85}",
            "a\u{2028}",
            "\u{2029}",
        ] {
            assert_eq!(
                ScriptName::new(bad),
                Err(NameError::ControlCharacter),
                "{bad:?}"
            );
        }
        assert_eq!(ScriptName::new("x".repeat(513)), Err(NameError::TooLong));
        assert!(ScriptName::new("x".repeat(512)).is_ok());
        assert_eq!(ScriptName::from_bytes(b"\xff"), Err(NameError::NotUtf8));
    }
}
