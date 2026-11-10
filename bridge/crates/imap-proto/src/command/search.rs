// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `UID SEARCH` with typed criteria.

use super::{Writer, command};
use crate::EncodeError;
use crate::types::{ImapDate, Keyword, ModSeq, NameEncoding, UidSet};

/// Deepest nesting of AND/OR/NOT we encode. Criteria come from user filters,
/// so this bounds the recursion.
const MAX_DEPTH: usize = 32;

/// A search criterion (subset of RFC 3501 §6.4.4 plus CONDSTORE).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchKey {
    /// `ALL`
    All,
    /// `UID set`
    Uid(UidSet),
    /// `TEXT s`: header or body contains `s`.
    Text(String),
    /// `BODY s`
    Body(String),
    /// `SUBJECT s`
    Subject(String),
    /// `FROM s`
    From(String),
    /// `TO s`
    To(String),
    /// `CC s`
    Cc(String),
    /// `BCC s`
    Bcc(String),
    /// `SINCE date` (internal date, on or after).
    Since(ImapDate),
    /// `BEFORE date` (internal date, before).
    Before(ImapDate),
    /// `ON date` (internal date).
    On(ImapDate),
    /// `LARGER n` (bytes).
    Larger(u32),
    /// `SMALLER n` (bytes).
    Smaller(u32),
    /// `KEYWORD k`
    Keyword(Keyword),
    /// `UNKEYWORD k`
    Unkeyword(Keyword),
    /// `SEEN`
    Seen,
    /// `UNSEEN`
    Unseen,
    /// `FLAGGED`
    Flagged,
    /// `UNFLAGGED`
    Unflagged,
    /// `ANSWERED`
    Answered,
    /// `UNANSWERED`
    Unanswered,
    /// `DELETED`
    Deleted,
    /// `UNDELETED`
    Undeleted,
    /// `DRAFT`
    Draft,
    /// `UNDRAFT`
    Undraft,
    /// `MODSEQ n` (CONDSTORE): changed at or after `n`.
    ModSeq(ModSeq),
    /// All of these (a parenthesised list).
    And(Vec<SearchKey>),
    /// Either of these.
    Or(Box<SearchKey>, Box<SearchKey>),
    /// Not this.
    Not(Box<SearchKey>),
}

impl SearchKey {
    /// Whether any string in the criteria is not ASCII (needs `CHARSET UTF-8`).
    fn needs_utf8(&self, depth: usize) -> Result<bool, EncodeError> {
        if depth > MAX_DEPTH {
            return Err(EncodeError::TooDeep);
        }
        let next = depth.saturating_add(1);
        Ok(match self {
            Self::Text(s)
            | Self::Body(s)
            | Self::Subject(s)
            | Self::From(s)
            | Self::To(s)
            | Self::Cc(s)
            | Self::Bcc(s) => !s.is_ascii(),
            Self::And(keys) => {
                let mut any = false;
                for key in keys {
                    any |= key.needs_utf8(next)?;
                }
                any
            }
            Self::Or(left, right) => left.needs_utf8(next)? | right.needs_utf8(next)?,
            Self::Not(key) => key.needs_utf8(next)?,
            _ => false,
        })
    }
}

/// How the server should answer a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SearchReturn {
    /// Plain `* SEARCH 1 2 3` (RFC 3501): every number, one by one.
    Plain,
    /// `RETURN (ALL)` (ESEARCH, RFC 4731): `* ESEARCH (TAG "A1") UID ALL
    /// 1:3`, a compact sequence set. Only with the `ESEARCH` capability.
    All,
}

/// `UID SEARCH [RETURN (ALL)] [CHARSET UTF-8] criteria`. Results are UIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidSearch<'a> {
    /// The criteria.
    pub criteria: &'a SearchKey,
    /// Plain SEARCH or ESEARCH answer.
    pub returns: SearchReturn,
}

command!(UidSearch<'_>, "UID SEARCH", |search, writer| {
    writer.raw(b"UID SEARCH ");
    match search.returns {
        SearchReturn::Plain => {}
        SearchReturn::All => writer.raw(b"RETURN (ALL) "),
    }
    // RFC 6855 §3: after `ENABLE UTF8=ACCEPT` the client must not send a
    // CHARSET (strings are UTF-8 anyway). `needs_utf8` also checks depth.
    if search.criteria.needs_utf8(0)? && writer.options().names != NameEncoding::Utf8 {
        writer.raw(b"CHARSET UTF-8 ");
    }
    write_key(writer, search.criteria, 0)
});

fn write_key(writer: &mut Writer, key: &SearchKey, depth: usize) -> Result<(), EncodeError> {
    if depth > MAX_DEPTH {
        return Err(EncodeError::TooDeep);
    }
    let next = depth.saturating_add(1);
    match key {
        SearchKey::Uid(set) => {
            writer.raw(b"UID ");
            writer.display(set);
        }
        SearchKey::Text(s) => with_string(writer, b"TEXT ", s)?,
        SearchKey::Body(s) => with_string(writer, b"BODY ", s)?,
        SearchKey::Subject(s) => with_string(writer, b"SUBJECT ", s)?,
        SearchKey::From(s) => with_string(writer, b"FROM ", s)?,
        SearchKey::To(s) => with_string(writer, b"TO ", s)?,
        SearchKey::Cc(s) => with_string(writer, b"CC ", s)?,
        SearchKey::Bcc(s) => with_string(writer, b"BCC ", s)?,
        SearchKey::Since(date) => with_display(writer, b"SINCE ", date),
        SearchKey::Before(date) => with_display(writer, b"BEFORE ", date),
        SearchKey::On(date) => with_display(writer, b"ON ", date),
        SearchKey::Larger(size) => with_display(writer, b"LARGER ", size),
        SearchKey::Smaller(size) => with_display(writer, b"SMALLER ", size),
        SearchKey::Keyword(keyword) => with_string_atom(writer, b"KEYWORD ", keyword),
        SearchKey::Unkeyword(keyword) => with_string_atom(writer, b"UNKEYWORD ", keyword),
        SearchKey::ModSeq(mod_seq) => with_display(writer, b"MODSEQ ", mod_seq),
        SearchKey::And(keys) => {
            if keys.is_empty() {
                return Err(EncodeError::Empty("search criteria"));
            }
            writer.raw(b"(");
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    writer.space();
                }
                write_key(writer, key, next)?;
            }
            writer.raw(b")");
        }
        SearchKey::Or(left, right) => {
            writer.raw(b"OR ");
            write_key(writer, left, next)?;
            writer.space();
            write_key(writer, right, next)?;
        }
        SearchKey::Not(key) => {
            writer.raw(b"NOT ");
            write_key(writer, key, next)?;
        }
        SearchKey::All => writer.raw(b"ALL"),
        SearchKey::Seen => writer.raw(b"SEEN"),
        SearchKey::Unseen => writer.raw(b"UNSEEN"),
        SearchKey::Flagged => writer.raw(b"FLAGGED"),
        SearchKey::Unflagged => writer.raw(b"UNFLAGGED"),
        SearchKey::Answered => writer.raw(b"ANSWERED"),
        SearchKey::Unanswered => writer.raw(b"UNANSWERED"),
        SearchKey::Deleted => writer.raw(b"DELETED"),
        SearchKey::Undeleted => writer.raw(b"UNDELETED"),
        SearchKey::Draft => writer.raw(b"DRAFT"),
        SearchKey::Undraft => writer.raw(b"UNDRAFT"),
    }
    Ok(())
}

fn with_string(writer: &mut Writer, name: &[u8], value: &str) -> Result<(), EncodeError> {
    writer.raw(name);
    writer.astring(value.as_bytes(), "search string")
}

fn with_display(writer: &mut Writer, name: &[u8], value: &impl std::fmt::Display) {
    writer.raw(name);
    writer.display(value);
}

fn with_string_atom(writer: &mut Writer, name: &[u8], keyword: &Keyword) {
    writer.raw(name);
    // Keywords are validated atoms (see `Keyword::new`).
    writer.display(&keyword.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::{basic, wire};
    use crate::command::{EncodeOptions, Tag, encode};

    #[test]
    fn encodes_criteria() {
        let key = SearchKey::And(vec![
            SearchKey::Unseen,
            SearchKey::Since(ImapDate::new(2024, 2, 1).unwrap()),
            SearchKey::Or(
                Box::new(SearchKey::From("bob".into())),
                Box::new(SearchKey::Subject("hola mundo".into())),
            ),
            SearchKey::Not(Box::new(SearchKey::Keyword(Keyword::new("$Junk").unwrap()))),
        ]);
        assert_eq!(
            basic(&UidSearch {
                criteria: &key,
                returns: SearchReturn::Plain
            }),
            "A1 UID SEARCH (UNSEEN SINCE 1-Feb-2024 OR FROM bob SUBJECT \"hola mundo\" NOT KEYWORD $Junk)\r\n"
        );
    }

    #[test]
    fn utf8_needs_charset() {
        let key = SearchKey::Text("café".into());
        assert_eq!(
            basic(&UidSearch {
                criteria: &key,
                returns: SearchReturn::Plain
            }),
            "A1 UID SEARCH CHARSET UTF-8 TEXT {5}\r\n<WAIT>café\r\n"
        );
    }

    #[test]
    fn search_utf8_mode_omits_charset() {
        let key = SearchKey::Text("café".into());
        let search = UidSearch {
            criteria: &key,
            returns: SearchReturn::Plain,
        };
        let options = EncodeOptions {
            names: NameEncoding::Utf8,
            ..EncodeOptions::basic()
        };
        assert_eq!(wire(&search, options), "A1 UID SEARCH TEXT \"café\"\r\n");
        // Without `ENABLE UTF8=ACCEPT` the charset stays, with a literal.
        assert_eq!(
            basic(&search),
            "A1 UID SEARCH CHARSET UTF-8 TEXT {5}\r\n<WAIT>café\r\n"
        );
    }

    #[test]
    fn esearch_return_goes_before_charset() {
        let key = SearchKey::Text("café".into());
        assert_eq!(
            basic(&UidSearch {
                criteria: &key,
                returns: SearchReturn::All
            }),
            "A1 UID SEARCH RETURN (ALL) CHARSET UTF-8 TEXT {5}\r\n<WAIT>café\r\n"
        );
    }

    #[test]
    fn deep_nesting_is_refused() {
        let mut key = SearchKey::All;
        for _ in 0..100 {
            key = SearchKey::Not(Box::new(key));
        }
        let result = encode(
            &UidSearch {
                criteria: &key,
                returns: SearchReturn::Plain,
            },
            Tag::new("A1").unwrap(),
            EncodeOptions::basic(),
        );
        assert_eq!(result.err(), Some(EncodeError::TooDeep));
    }
}
