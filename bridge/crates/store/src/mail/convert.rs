// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Conversions between the typed values and what SQLite stores.

use nebula_imap_proto::types::{MailboxName, ModSeq, NameEncoding, Uid, UidValidity};
use nebula_jmap_proto::object::email::EmailAddress;

use crate::{EmailId, MailboxId, StoreError, ThreadId};

pub(crate) fn uid_to_db(uid: Uid) -> i64 {
    i64::from(uid.get())
}

pub(crate) fn uid_from_db(value: i64) -> Result<Uid, StoreError> {
    u32::try_from(value)
        .ok()
        .and_then(Uid::new)
        .ok_or(StoreError::Corrupt("UID"))
}

pub(crate) fn validity_to_db(value: UidValidity) -> i64 {
    i64::from(value.get())
}

pub(crate) fn validity_from_db(value: i64) -> Result<UidValidity, StoreError> {
    u32::try_from(value)
        .ok()
        .and_then(UidValidity::new)
        .ok_or(StoreError::Corrupt("UIDVALIDITY"))
}

pub(crate) fn mod_seq_to_db(value: ModSeq) -> Result<i64, StoreError> {
    // `ModSeq` is at most 2^63 - 1, so this never fails in practice.
    i64::try_from(value.get()).map_err(|_| StoreError::Invalid("MODSEQ"))
}

pub(crate) fn mod_seq_from_db(value: i64) -> Result<ModSeq, StoreError> {
    u64::try_from(value)
        .ok()
        .and_then(ModSeq::new)
        .ok_or(StoreError::Corrupt("MODSEQ"))
}

pub(crate) fn opt<T, U>(
    value: Option<T>,
    convert: impl FnOnce(T) -> Result<U, StoreError>,
) -> Result<Option<U>, StoreError> {
    value.map(convert).transpose()
}

pub(crate) fn u32_from_db(value: i64, what: &'static str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::Corrupt(what))
}

pub(crate) fn u64_from_db(value: i64, what: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt(what))
}

pub(crate) fn u64_to_db(value: u64, what: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Invalid(what))
}

/// The stored form of a mailbox name: modified UTF-7 on the wire. Names the
/// server sent with invalid UTF-7 come back unchanged, so they round-trip.
pub(crate) fn name_to_db(name: &MailboxName) -> String {
    name.to_wire(NameEncoding::ModifiedUtf7).into_owned()
}

pub(crate) fn name_from_db(text: &str) -> Result<MailboxName, StoreError> {
    MailboxName::from_wire(text, NameEncoding::ModifiedUtf7)
        .map_err(|_| StoreError::Corrupt("mailbox name"))
}

pub(crate) fn delimiter_to_db(delimiter: Option<char>) -> Option<String> {
    delimiter.map(String::from)
}

pub(crate) fn delimiter_from_db(text: Option<&str>) -> Result<Option<char>, StoreError> {
    let Some(text) = text else {
        return Ok(None);
    };
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(Some(c)),
        _ => Err(StoreError::Corrupt("delimiter")),
    }
}

pub(crate) fn mailbox_id(value: i64) -> Result<MailboxId, StoreError> {
    MailboxId::from_raw(value).ok_or(StoreError::Corrupt("mailbox id"))
}

pub(crate) fn email_id(value: i64) -> Result<EmailId, StoreError> {
    EmailId::from_raw(value).ok_or(StoreError::Corrupt("email id"))
}

pub(crate) fn thread_id(value: i64) -> Result<ThreadId, StoreError> {
    ThreadId::from_raw(value).ok_or(StoreError::Corrupt("thread id"))
}

pub(crate) fn addresses_to_db(
    list: Option<&Vec<EmailAddress>>,
) -> Result<Option<String>, StoreError> {
    list.map(|list| serde_json::to_string(list).map_err(|_| StoreError::Invalid("addresses")))
        .transpose()
}

pub(crate) fn addresses_from_db(
    text: Option<&str>,
) -> Result<Option<Vec<EmailAddress>>, StoreError> {
    text.map(|text| serde_json::from_str(text).map_err(|_| StoreError::Corrupt("addresses")))
        .transpose()
}

pub(crate) fn strings_to_db(list: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(list).map_err(|_| StoreError::Invalid("message ids"))
}

pub(crate) fn strings_from_db(text: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(text).map_err(|_| StoreError::Corrupt("message ids"))
}

/// A JSON array of numbers, bound as one parameter and read back with
/// `json_each` (no limit on how many ids a query can take).
pub(crate) fn id_list(ids: impl IntoIterator<Item = i64>) -> String {
    let mut out = String::from("[");
    for (index, id) in ids.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&id.to_string());
    }
    out.push(']');
    out
}

/// The subject for sorting: without `Re:`/`Fwd:`-style prefixes and
/// `[list]` tags, trimmed and lowercased (a simple RFC 5256 base subject).
pub(crate) fn sort_subject(subject: Option<&str>) -> String {
    let mut rest = subject.unwrap_or("").trim();
    loop {
        let before = rest;
        rest = strip_list_tag(rest).trim_start();
        rest = strip_reply_prefix(rest).trim_start();
        if rest == before {
            break;
        }
    }
    let rest = rest.trim_end();
    let rest = rest.strip_suffix("(fwd)").unwrap_or(rest).trim_end();
    rest.to_lowercase()
}

fn strip_list_tag(text: &str) -> &str {
    if let Some(inner) = text.strip_prefix('[')
        && let Some(end) = inner.find(']')
    {
        // Don't strip a subject that is only a tag.
        let rest = inner.get(end.saturating_add(1)..).unwrap_or("");
        if !rest.trim().is_empty() {
            return rest;
        }
    }
    text
}

fn strip_reply_prefix(text: &str) -> &str {
    const PREFIXES: [&str; 6] = ["re", "fwd", "fw", "aw", "rv", "sv"];
    for prefix in PREFIXES {
        let Some(head) = text.get(..prefix.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(prefix) {
            continue;
        }
        let mut rest = text.get(prefix.len()..).unwrap_or("");
        // Optional "[2]" counter, as in "Re[2]:".
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(end) = inner.find(']')
            && inner
                .get(..end)
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        {
            rest = inner.get(end.saturating_add(1)..).unwrap_or("");
        }
        if let Some(after) = rest.trim_start().strip_prefix(':') {
            return after;
        }
    }
    text
}

/// The first sender for sorting: its name, else its address, lowercased.
pub(crate) fn sort_from(from: Option<&Vec<EmailAddress>>) -> String {
    from.and_then(|list| list.first())
        .map(|address| {
            address
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&address.email)
                .trim()
                .to_lowercase()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_subjects() {
        assert_eq!(sort_subject(Some("Re: Hola")), "hola");
        assert_eq!(sort_subject(Some("RE: Fwd: re[2]: Hola")), "hola");
        assert_eq!(sort_subject(Some("[lista] Re: Hola (fwd)")), "hola");
        assert_eq!(sort_subject(Some("[solo]")), "[solo]");
        assert_eq!(sort_subject(Some("Reunión")), "reunión");
        assert_eq!(sort_subject(Some("AW: Termin")), "termin");
        assert_eq!(sort_subject(None), "");
    }

    #[test]
    fn first_sender() {
        let list = vec![
            EmailAddress {
                name: Some(" Bob ".to_owned()),
                email: "b@x".to_owned(),
            },
            EmailAddress {
                name: None,
                email: "a@x".to_owned(),
            },
        ];
        assert_eq!(sort_from(Some(&list)), "bob");
        assert_eq!(sort_from(Some(&list[1..].to_vec())), "a@x");
        assert_eq!(sort_from(None), "");
    }

    #[test]
    fn names_round_trip() {
        for name in ["INBOX", "Enviados é", "A&B", "Work/2026"] {
            let name = MailboxName::new(name).unwrap();
            assert_eq!(name_from_db(&name_to_db(&name)).unwrap(), name);
        }
        let raw = MailboxName::from_wire("Bad&-x&Zz", NameEncoding::ModifiedUtf7).unwrap();
        assert_eq!(name_from_db(&name_to_db(&raw)).unwrap(), raw);
    }

    #[test]
    fn id_lists() {
        assert_eq!(id_list([1, 22, 3]), "[1,22,3]");
        assert_eq!(id_list([]), "[]");
    }
}
