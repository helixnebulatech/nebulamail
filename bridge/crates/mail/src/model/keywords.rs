// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! IMAP flags ↔ JMAP keywords (RFC 8621 §4.1.1).
//!
//! - System flags map to `$seen`, `$answered`, `$flagged`, `$draft`.
//! - `\Recent` is not a keyword (it's per session) and is dropped.
//! - `\Deleted` is not a keyword either: a message marked `\Deleted` is
//!   treated as already gone (hidden), because it will disappear at the next
//!   EXPUNGE of any client. If another client clears the flag, the next sync
//!   sees the message again (as a new email unless the server has object
//!   ids).
//! - Keywords are lowercased (`$Forwarded` → `$forwarded`); Thunderbird's
//!   `Junk`/`NonJunk` become `$junk`/`$notjunk`. Unknown system flags
//!   (`\Foo`) and keywords JMAP can't express are dropped.
//! - Back to IMAP, the registered keywords get their usual spelling
//!   (`$Forwarded`, `$Junk`, `$NotJunk`, `$MDNSent`, `$Phishing`).

use std::collections::BTreeSet;

use nebula_imap::PermanentFlags;
use nebula_imap::proto::types::{Flag, Keyword as ImapKeyword};
use nebula_jmap_proto::Keyword;

/// Flags of one message, split the way the bridge uses them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MessageFlags {
    /// JMAP keywords.
    pub(crate) keywords: BTreeSet<Keyword>,
    /// `\Deleted`: treat the message as gone.
    pub(crate) deleted: bool,
}

pub(crate) fn from_flags(flags: &[Flag]) -> MessageFlags {
    let mut result = MessageFlags::default();
    for flag in flags {
        let keyword = match flag {
            Flag::Seen => Some(Keyword::seen()),
            Flag::Answered => Some(Keyword::answered()),
            Flag::Flagged => Some(Keyword::flagged()),
            Flag::Draft => Some(Keyword::draft()),
            Flag::Deleted => {
                result.deleted = true;
                None
            }
            Flag::Keyword(keyword) => from_imap_keyword(keyword),
            // `\Recent`, unknown system flags and future variants.
            _ => None,
        };
        if let Some(keyword) = keyword {
            result.keywords.insert(keyword);
        }
    }
    result
}

fn from_imap_keyword(keyword: &ImapKeyword) -> Option<Keyword> {
    let text = keyword.as_str();
    if text.eq_ignore_ascii_case("nonjunk") {
        return Some(Keyword::not_junk());
    }
    if text.eq_ignore_ascii_case("junk") {
        return Some(Keyword::junk());
    }
    text.parse().ok()
}

/// The IMAP flag for a keyword.
pub(crate) fn to_flag(keyword: &Keyword) -> Option<Flag> {
    let text = match keyword.as_str() {
        "$seen" => return Some(Flag::Seen),
        "$answered" => return Some(Flag::Answered),
        "$flagged" => return Some(Flag::Flagged),
        "$draft" => return Some(Flag::Draft),
        "$forwarded" => "$Forwarded",
        "$junk" => "$Junk",
        "$notjunk" => "$NotJunk",
        "$mdnsent" => "$MDNSent",
        "$phishing" => "$Phishing",
        other => other,
    };
    // Every JMAP keyword character is valid in an IMAP keyword.
    ImapKeyword::new(text).ok().map(Flag::Keyword)
}

/// The IMAP flags to clear for a keyword: its own flag, plus the old
/// spellings [`from_flags`] also reads as it (`Junk` for `$junk`, `NonJunk`
/// for `$notjunk`), so clearing the keyword really clears it. Clearing a
/// flag the message doesn't have does nothing (RFC 9051 §6.4.6).
pub(crate) fn removal_flags(keyword: &Keyword) -> Option<Vec<Flag>> {
    let flag = to_flag(keyword)?;
    let alias = match keyword.as_str() {
        "$junk" => Some("Junk"),
        "$notjunk" => Some("NonJunk"),
        _ => None,
    };
    let mut flags = vec![flag];
    if let Some(alias) = alias.and_then(|text| ImapKeyword::new(text).ok()) {
        flags.push(Flag::Keyword(alias));
    }
    Some(flags)
}

/// The first flag of `add` that one of the mailboxes (PERMANENTFLAGS) can't
/// store ([`PermanentFlags::can_store`]).
pub(crate) fn first_unstorable<'a>(
    mailboxes: &[&PermanentFlags],
    add: &'a [Flag],
) -> Option<&'a Flag> {
    add.iter()
        .find(|flag| mailboxes.iter().any(|permanent| !permanent.can_store(flag)))
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::NewKeywords;

    use super::*;

    fn kw(text: &str) -> Keyword {
        text.parse().unwrap()
    }

    fn imap(text: &str) -> Flag {
        Flag::parse(text).unwrap()
    }

    #[test]
    fn flags_to_keywords() {
        let flags = [
            imap("\\Seen"),
            imap("\\Answered"),
            imap("\\Recent"),
            imap("\\Deleted"),
            imap("$Forwarded"),
            imap("NonJunk"),
            imap("\\Unknown"),
            imap("Custom]"),
        ];
        let result = from_flags(&flags);
        assert!(result.deleted);
        assert_eq!(
            result.keywords,
            BTreeSet::from([
                Keyword::seen(),
                Keyword::answered(),
                kw("$forwarded"),
                Keyword::not_junk()
            ])
        );
    }

    #[test]
    fn keywords_to_flags() {
        assert_eq!(to_flag(&Keyword::seen()), Some(Flag::Seen));
        assert_eq!(to_flag(&Keyword::draft()), Some(Flag::Draft));
        assert_eq!(to_flag(&kw("$forwarded")), Some(imap("$Forwarded")));
        assert_eq!(to_flag(&kw("$notjunk")), Some(imap("$NotJunk")));
        assert_eq!(to_flag(&kw("work")), Some(imap("work")));
    }

    #[test]
    fn round_trip_of_registered_keywords() {
        for keyword in [
            "$seen",
            "$flagged",
            "$answered",
            "$draft",
            "$forwarded",
            "$junk",
            "x-y",
        ] {
            let flag = to_flag(&kw(keyword)).unwrap();
            assert_eq!(
                from_flags(&[flag]).keywords,
                BTreeSet::from([kw(keyword)]),
                "{keyword}"
            );
        }
    }

    #[test]
    fn removing_junk_clears_its_aliases() {
        let junk = removal_flags(&Keyword::junk()).unwrap();
        assert!(junk.contains(&imap("$Junk")) && junk.contains(&imap("Junk")));
        let not_junk = removal_flags(&Keyword::not_junk()).unwrap();
        assert!(not_junk.contains(&imap("$NotJunk")) && not_junk.contains(&imap("NonJunk")));
        assert_eq!(removal_flags(&Keyword::seen()), Some(vec![Flag::Seen]));
        // A message with the old spelling reads as `$junk`, and clearing
        // `$junk` clears that spelling too.
        let read = from_flags(&[imap("Junk")]).keywords;
        assert_eq!(read, BTreeSet::from([Keyword::junk()]));
        let cleared: Vec<Flag> = read.iter().filter_map(removal_flags).flatten().collect();
        assert!(cleared.contains(&imap("Junk")));
    }

    #[test]
    fn unstorable_in_any_copy() {
        let all = PermanentFlags::NotAnnounced;
        let few = PermanentFlags::Listed {
            flags: vec![Flag::Seen],
            new_keywords: NewKeywords::NotAllowed,
        };
        let open = PermanentFlags::Listed {
            flags: vec![Flag::Seen],
            new_keywords: NewKeywords::Allowed,
        };
        let add = [Flag::Seen, imap("work")];
        assert_eq!(first_unstorable(&[&all], &add), None);
        assert_eq!(first_unstorable(&[&all, &few], &add), Some(&imap("work")));
        assert_eq!(first_unstorable(&[&open], &add), None);
    }
}
