// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP ids of the bridge's objects: a letter for the kind and the row
//! number (`m3` mailbox, `e17` email, `t9` thread, `i1` identity, `s4`
//! submission), and the state strings (the change-log sequence number in
//! decimal).
//!
//! Parsing is strict: only the exact form we write is accepted (no sign,
//! no leading zeros, the right letter), so every id maps to one row and
//! back.

use nebula_jmap_proto::{
    EmailId as JmapEmailId, EmailSubmissionId, IdentityId as JmapIdentityId,
    MailboxId as JmapMailboxId, State, ThreadId as JmapThreadId,
};
use nebula_store::{EmailId, IdentityId, MailboxId, Seq, SubmissionId, ThreadId};

use crate::error::Bug;

/// The row number after `prefix`, if `text` is exactly `prefix` + a
/// positive decimal without leading zeros.
pub(crate) fn parse_number(text: &str, prefix: char) -> Option<i64> {
    let digits = text.strip_prefix(prefix)?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok().filter(|n| *n > 0)
}

macro_rules! id_mapping {
    ($to:ident, $from:ident, $store:ty, $jmap:ty, $prefix:literal) => {
        /// The JMAP id of a row.
        ///
        /// # Errors
        /// Never in practice: a letter and a positive number is always a
        /// valid JMAP id. It is still checked (no `expect` outside tests).
        pub(crate) fn $to(id: $store) -> Result<$jmap, Bug> {
            format!("{}{}", $prefix, id.get())
                .parse()
                .map_err(|_| Bug("id"))
        }

        /// The row of a JMAP id, if it is one of ours.
        pub(crate) fn $from(id: &$jmap) -> Option<$store> {
            parse_number(id.as_str(), $prefix).and_then(<$store>::from_raw)
        }
    };
}

id_mapping!(mailbox_id, store_mailbox, MailboxId, JmapMailboxId, 'm');
id_mapping!(email_id, store_email, EmailId, JmapEmailId, 'e');
id_mapping!(thread_id, store_thread, ThreadId, JmapThreadId, 't');
id_mapping!(identity_id, store_identity, IdentityId, JmapIdentityId, 'i');
id_mapping!(
    submission_id,
    store_submission,
    SubmissionId,
    EmailSubmissionId,
    's'
);

/// The JMAP state string of a change-log position.
pub(crate) fn state(seq: Seq) -> State {
    State::new(seq.get().to_string())
}

/// The change-log position of a state string we gave out. `None` for
/// anything else (the client then gets `cannotCalculateChanges`).
pub(crate) fn parse_state(state: &State) -> Option<Seq> {
    let text = state.as_str();
    let canonical = text == "0" || (!text.starts_with('0') && !text.is_empty());
    if !canonical || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok().map(Seq::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        let mailbox = MailboxId::from_raw(42).unwrap();
        assert_eq!(mailbox_id(mailbox).unwrap().as_str(), "m42");
        assert_eq!(store_mailbox(&mailbox_id(mailbox).unwrap()), Some(mailbox));
        let email = EmailId::from_raw(7).unwrap();
        assert_eq!(email_id(email).unwrap().as_str(), "e7");
        assert_eq!(store_email(&"e7".parse().unwrap()), Some(email));
    }

    #[test]
    fn strict_parsing() {
        for bad in [
            "m0",
            "m01",
            "m-1",
            "e1",
            "m",
            "m1a",
            "M1",
            "m99999999999999999999",
        ] {
            assert_eq!(store_mailbox(&bad.parse().unwrap()), None, "{bad}");
        }
        assert_eq!(parse_number("t12", 't'), Some(12));
    }

    #[test]
    fn states() {
        assert_eq!(state(Seq::new(15)).as_str(), "15");
        assert_eq!(parse_state(&State::new("15")), Some(Seq::new(15)));
        assert_eq!(parse_state(&State::new("0")), Some(Seq::ZERO));
        for bad in ["", "015", "-1", "1.0", "abc", " 1"] {
            assert_eq!(parse_state(&State::new(bad)), None, "{bad:?}");
        }
    }
}
