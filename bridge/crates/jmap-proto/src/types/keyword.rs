// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Email keywords (RFC 8621 §4.1.1), the JMAP form of IMAP flags.

use std::fmt;
use std::str::FromStr;

use crate::macros::serde_via_str;

/// An Email keyword such as `$seen` (RFC 8621 §4.1.1).
///
/// 1 to 255 characters in `%x21-%x7e`, without `( ) { ] % * " \`.
/// Keywords are case-insensitive and servers must return them in lowercase,
/// so the value is stored lowercased: `"$Seen"` parses to `$seen`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Keyword(String);

/// Why a string is not a valid [`Keyword`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeywordError {
    /// The keyword is empty.
    #[error("keyword is empty")]
    Empty,
    /// The keyword is longer than 255 characters.
    #[error("keyword is longer than 255 characters")]
    TooLong,
    /// The keyword has a character that IMAP does not allow in a flag.
    #[error("keyword contains a character that is not allowed")]
    InvalidCharacter,
}

impl Keyword {
    /// Longest keyword (RFC 8621 §4.1.1).
    pub const MAX_LEN: usize = 255;

    /// The keyword, in lowercase.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `$draft`: the user is composing this Email.
    #[must_use]
    pub fn draft() -> Self {
        Self("$draft".to_owned())
    }

    /// `$seen`: the Email has been read.
    #[must_use]
    pub fn seen() -> Self {
        Self("$seen".to_owned())
    }

    /// `$flagged`: flagged for attention (starred).
    #[must_use]
    pub fn flagged() -> Self {
        Self("$flagged".to_owned())
    }

    /// `$answered`: the Email has been replied to.
    #[must_use]
    pub fn answered() -> Self {
        Self("$answered".to_owned())
    }

    /// `$forwarded`: the Email has been forwarded.
    #[must_use]
    pub fn forwarded() -> Self {
        Self("$forwarded".to_owned())
    }

    /// `$phishing`: very likely phishing.
    #[must_use]
    pub fn phishing() -> Self {
        Self("$phishing".to_owned())
    }

    /// `$junk`: spam.
    #[must_use]
    pub fn junk() -> Self {
        Self("$junk".to_owned())
    }

    /// `$notjunk`: not spam.
    #[must_use]
    pub fn not_junk() -> Self {
        Self("$notjunk".to_owned())
    }
}

const fn is_keyword_byte(b: u8) -> bool {
    matches!(b, 0x21..=0x7e) && !matches!(b, b'(' | b')' | b'{' | b']' | b'%' | b'*' | b'"' | b'\\')
}

impl FromStr for Keyword {
    type Err = KeywordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(KeywordError::Empty);
        }
        // All allowed characters are ASCII, so bytes = characters.
        if s.len() > Self::MAX_LEN {
            return Err(KeywordError::TooLong);
        }
        if !s.bytes().all(is_keyword_byte) {
            return Err(KeywordError::InvalidCharacter);
        }
        Ok(Self(s.to_ascii_lowercase()))
    }
}

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Keyword").field(&self.0).finish()
    }
}

serde_via_str!(Keyword);

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn lowercases() {
        assert_eq!("$Seen".parse::<Keyword>().unwrap(), Keyword::seen());
        assert_eq!("NonJunk".parse::<Keyword>().unwrap().as_str(), "nonjunk");
    }

    #[test]
    fn rejects_imap_specials() {
        for bad in [
            "", "a b", "(", ")", "{", "]", "%", "*", "\"", "\\", "é", "\u{7f}", "a\tb",
        ] {
            assert!(bad.parse::<Keyword>().is_err(), "{bad:?}");
        }
        assert_eq!(
            "x".repeat(256).parse::<Keyword>(),
            Err(KeywordError::TooLong)
        );
        // `[` and `}` are allowed: only `{` and `]` are excluded.
        assert!("[x}".parse::<Keyword>().is_ok());
    }

    proptest! {
        #[test]
        fn valid_keywords_round_trip(s in "[!#-'+-~&&[^{\\]%*\\\\]]{1,255}") {
            let keyword: Keyword = s.parse().unwrap();
            prop_assert_eq!(keyword.as_str(), s.to_ascii_lowercase());
            let reparsed: Keyword = keyword.to_string().parse().unwrap();
            prop_assert_eq!(reparsed, keyword);
        }

        #[test]
        fn case_does_not_matter(s in "[a-zA-Z$]{1,40}") {
            prop_assert_eq!(
                s.to_ascii_uppercase().parse::<Keyword>().unwrap(),
                s.to_ascii_lowercase().parse::<Keyword>().unwrap()
            );
        }

        #[test]
        fn any_string_is_checked(s in "\\PC{0,20}") {
            let ok = s.parse::<Keyword>().is_ok();
            let expected = !s.is_empty() && s.len() <= 255 && s.bytes().all(is_keyword_byte);
            prop_assert_eq!(ok, expected);
        }
    }
}
