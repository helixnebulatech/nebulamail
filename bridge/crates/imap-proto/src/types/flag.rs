// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Message flags and keywords.

use std::fmt;

use super::is_astring_char;
use crate::ValueError;

/// A keyword (user flag) such as `$Forwarded` or `$Junk`.
///
/// Keywords are atoms. We also accept `]`, because some servers use it in
/// keywords (`OIB-Seen-[Gmail]/All`) and expect it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Keyword(Box<str>);

/// Longest keyword we accept: real ones are short, and this keeps command
/// lines small.
const MAX_KEYWORD_LEN: usize = 255;

impl Keyword {
    /// Validates a keyword.
    ///
    /// # Errors
    ///
    /// Empty, too long, or with characters outside the atom grammar.
    pub fn new(text: &str) -> Result<Self, ValueError> {
        if text.is_empty() {
            return Err(ValueError::Empty("keyword"));
        }
        if text.len() > MAX_KEYWORD_LEN {
            return Err(ValueError::OutOfRange("keyword"));
        }
        if !text.bytes().all(is_astring_char) {
            return Err(ValueError::Invalid("keyword"));
        }
        Ok(Self(text.into()))
    }

    /// The keyword as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A message flag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Flag {
    /// `\Seen`
    Seen,
    /// `\Answered`
    Answered,
    /// `\Flagged`
    Flagged,
    /// `\Deleted`
    Deleted,
    /// `\Draft`
    Draft,
    /// `\Recent` (only sent by the server; it cannot be stored).
    Recent,
    /// A keyword such as `$Forwarded`.
    Keyword(Keyword),
    /// A `\Something` flag we do not know; stored without the backslash.
    Extension(Keyword),
}

impl Flag {
    /// Whether both are the same flag for the server: keywords and unknown
    /// `\Something` flags compare without case (RFC 3501 §2.3.2 treats
    /// flag names as case-insensitive). `PartialEq` stays exact, so that it
    /// agrees with `Hash` and `Ord`.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Keyword(a), Self::Keyword(b)) | (Self::Extension(a), Self::Extension(b)) => {
                a.as_str().eq_ignore_ascii_case(b.as_str())
            }
            _ => self == other,
        }
    }

    /// Parses a flag as sent by the server (`\Seen`, `$Junk`…).
    ///
    /// # Errors
    ///
    /// Text that is neither a system flag nor a valid keyword.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        let Some(name) = text.strip_prefix('\\') else {
            return Keyword::new(text).map(Self::Keyword);
        };
        let flag = match name.to_ascii_lowercase().as_str() {
            "seen" => Self::Seen,
            "answered" => Self::Answered,
            "flagged" => Self::Flagged,
            "deleted" => Self::Deleted,
            "draft" => Self::Draft,
            "recent" => Self::Recent,
            _ => Self::Extension(Keyword::new(name)?),
        };
        Ok(flag)
    }

    /// The flag as it appears on the wire.
    #[must_use]
    pub fn as_wire(&self) -> String {
        match self {
            Self::Seen => "\\Seen".to_owned(),
            Self::Answered => "\\Answered".to_owned(),
            Self::Flagged => "\\Flagged".to_owned(),
            Self::Deleted => "\\Deleted".to_owned(),
            Self::Draft => "\\Draft".to_owned(),
            Self::Recent => "\\Recent".to_owned(),
            Self::Keyword(keyword) => keyword.as_str().to_owned(),
            Self::Extension(name) => format!("\\{}", name.as_str()),
        }
    }

    /// Whether a client may set or clear this flag with STORE or APPEND.
    #[must_use]
    pub fn is_storable(&self) -> bool {
        !matches!(self, Self::Recent)
    }
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ignores_case() {
        let parse = |text| Flag::parse(text).unwrap();
        assert!(parse("$Forwarded").matches(&parse("$forwarded")));
        assert!(parse("\\X-Thing").matches(&parse("\\x-thing")));
        assert!(parse("\\Seen").matches(&parse("\\SEEN")));
        assert!(!parse("$Forwarded").matches(&parse("$Junk")));
        // A keyword is not the extension flag of the same name.
        assert!(!parse("Thing").matches(&parse("\\Thing")));
        assert_ne!(parse("$Forwarded"), parse("$forwarded"));
    }

    #[test]
    fn parse_flags() {
        assert_eq!(Flag::parse("\\SEEN"), Ok(Flag::Seen));
        assert_eq!(Flag::parse("\\Draft"), Ok(Flag::Draft));
        assert_eq!(
            Flag::parse("$Forwarded"),
            Ok(Flag::Keyword(Keyword::new("$Forwarded").unwrap()))
        );
        assert_eq!(Flag::parse("\\Foo").unwrap().as_wire(), "\\Foo");
        assert!(Flag::parse("a b").is_err());
        assert!(Flag::parse("a\r\n").is_err());
        assert!(Flag::parse("").is_err());
        assert!(Flag::parse("\\").is_err());
        assert!(Flag::parse("\\\\x").is_err());
    }
}
