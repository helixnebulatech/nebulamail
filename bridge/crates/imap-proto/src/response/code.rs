// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Response codes (`[UIDVALIDITY 3]`, `[COPYUID ...]`…).
//!
//! `imap-proto` knows the RFC 3501, CONDSTORE and UIDPLUS codes. For the
//! rest (`NOMODSEQ`, `CLOSED`, `MODIFIED`, RFC 5530 codes such as
//! `AUTHENTICATIONFAILED`, `CAPABILITY` without `IMAP4rev1`…) it leaves the
//! whole `[...]` in the text, and we parse it here.

use std::borrow::Cow;

use imap_proto::rfc4315::UidSetMember;

use super::fetch::{ObjectId, convert_flags};
use crate::types::{
    Capabilities, Capability, Flag, ModSeq, SeqBound, SeqItem, SeqNum, Uid, UidSet, UidValidity,
};

/// Whether `\*` was in PERMANENTFLAGS: new keywords can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NewKeywords {
    /// The server accepts new keywords.
    Allowed,
    /// Only the listed flags can be stored.
    NotAllowed,
}

/// A response code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseCode {
    /// `ALERT`: the text must be shown to the user.
    Alert,
    /// `PARSE`: the server could not parse a message.
    Parse,
    /// `READ-ONLY`
    ReadOnly,
    /// `READ-WRITE`
    ReadWrite,
    /// `TRYCREATE`: the target mailbox does not exist.
    TryCreate,
    /// `BADCHARSET (...)`
    BadCharset(Vec<String>),
    /// `CAPABILITY ...`
    Capabilities(Capabilities),
    /// `PERMANENTFLAGS (...)`
    PermanentFlags {
        /// Flags that are stored permanently.
        flags: Vec<Flag>,
        /// Whether new keywords can be created (`\*`).
        new_keywords: NewKeywords,
    },
    /// `UIDNEXT n`
    UidNext(Uid),
    /// `UIDVALIDITY n`
    UidValidity(UidValidity),
    /// `UNSEEN n`: first unseen message.
    Unseen(SeqNum),
    /// `HIGHESTMODSEQ n` (CONDSTORE).
    HighestModSeq(ModSeq),
    /// `NOMODSEQ`: the mailbox has no mod-sequences.
    NoModSeq,
    /// `MODIFIED set`: conditional STORE failed for these (UIDs for UID STORE).
    Modified(UidSet),
    /// `APPENDUID uidvalidity uids` (UIDPLUS).
    AppendUid {
        /// UIDVALIDITY of the target mailbox.
        uid_validity: UidValidity,
        /// The new UIDs.
        uids: UidSet,
    },
    /// `COPYUID uidvalidity source destination` (UIDPLUS).
    CopyUid {
        /// UIDVALIDITY of the target mailbox.
        uid_validity: UidValidity,
        /// Source UIDs.
        source: UidSet,
        /// Destination UIDs, in the same order.
        destination: UidSet,
    },
    /// `UIDNOTSTICKY`
    UidNotSticky,
    /// `CLOSED` (QRESYNC): the previous mailbox is closed.
    Closed,
    /// `MAILBOXID (id)` (OBJECTID).
    MailboxId(ObjectId),
    /// `AUTHENTICATIONFAILED` (RFC 5530).
    AuthenticationFailed,
    /// `AUTHORIZATIONFAILED`
    AuthorizationFailed,
    /// `UNAVAILABLE`: try again later.
    Unavailable,
    /// `EXPIRED`: the password expired.
    Expired,
    /// `PRIVACYREQUIRED`: needs TLS.
    PrivacyRequired,
    /// `CONTACTADMIN`
    ContactAdmin,
    /// `NOPERM`
    NoPerm,
    /// `INUSE`
    InUse,
    /// `EXPUNGEISSUED`
    ExpungeIssued,
    /// `CORRUPTION`
    Corruption,
    /// `SERVERBUG`
    ServerBug,
    /// `CLIENTBUG`
    ClientBug,
    /// `CANNOT`
    Cannot,
    /// `LIMIT`
    Limit,
    /// `OVERQUOTA`
    OverQuota,
    /// `ALREADYEXISTS`
    AlreadyExists,
    /// `NONEXISTENT`
    NonExistent,
    /// A known code whose value is invalid (for example `UIDNEXT 0`).
    Malformed(String),
    /// Any other code: its name, upper-cased.
    Other(String),
}

/// Converts a code parsed by `imap-proto`.
pub(crate) fn convert_code(code: imap_proto::ResponseCode<'_>) -> ResponseCode {
    use imap_proto::ResponseCode as Proto;
    let malformed = |name: &str| ResponseCode::Malformed(name.to_owned());
    match code {
        Proto::Alert => ResponseCode::Alert,
        Proto::Parse => ResponseCode::Parse,
        Proto::ReadOnly => ResponseCode::ReadOnly,
        Proto::ReadWrite => ResponseCode::ReadWrite,
        Proto::TryCreate => ResponseCode::TryCreate,
        Proto::UidNotSticky => ResponseCode::UidNotSticky,
        Proto::BadCharset(list) => ResponseCode::BadCharset(
            list.unwrap_or_default()
                .into_iter()
                .map(Cow::into_owned)
                .collect(),
        ),
        Proto::Capabilities(list) => {
            ResponseCode::Capabilities(super::convert::convert_capabilities(&list))
        }
        Proto::PermanentFlags(list) => permanent_flags(&list),
        Proto::UidNext(value) => {
            Uid::new(value).map_or_else(|| malformed("UIDNEXT"), ResponseCode::UidNext)
        }
        Proto::UidValidity(value) => UidValidity::new(value)
            .map_or_else(|| malformed("UIDVALIDITY"), ResponseCode::UidValidity),
        Proto::Unseen(value) => {
            SeqNum::new(value).map_or_else(|| malformed("UNSEEN"), ResponseCode::Unseen)
        }
        Proto::HighestModSeq(value) => ModSeq::new(value)
            .map_or_else(|| malformed("HIGHESTMODSEQ"), ResponseCode::HighestModSeq),
        Proto::AppendUid(validity, uids) => match (UidValidity::new(validity), uid_set(&uids)) {
            (Some(uid_validity), Some(uids)) => ResponseCode::AppendUid { uid_validity, uids },
            _ => malformed("APPENDUID"),
        },
        Proto::CopyUid(validity, source, destination) => {
            match (
                UidValidity::new(validity),
                uid_set(&source),
                uid_set(&destination),
            ) {
                (Some(uid_validity), Some(source), Some(destination)) => ResponseCode::CopyUid {
                    uid_validity,
                    source,
                    destination,
                },
                _ => malformed("COPYUID"),
            }
        }
        Proto::MetadataLongEntries(_)
        | Proto::MetadataMaxSize(_)
        | Proto::MetadataTooMany
        | Proto::MetadataNoPrivate => ResponseCode::Other("METADATA".to_owned()),
        // The enum is non-exhaustive: a code added to `imap-proto` later.
        _ => ResponseCode::Other("UNKNOWN".to_owned()),
    }
}

/// Flags we cannot represent are dropped one by one, like in FETCH.
fn permanent_flags(list: &[Cow<'_, str>]) -> ResponseCode {
    let mut new_keywords = NewKeywords::NotAllowed;
    let mut flags = Vec::new();
    for flag in list {
        if flag == "\\*" {
            new_keywords = NewKeywords::Allowed;
        } else {
            flags.push(flag.clone());
        }
    }
    let flags = convert_flags(&flags);
    ResponseCode::PermanentFlags {
        flags,
        new_keywords,
    }
}

fn uid_set(members: &[UidSetMember]) -> Option<UidSet> {
    let mut items = Vec::with_capacity(members.len());
    for member in members {
        let item = match member {
            UidSetMember::Uid(uid) => SeqItem::Single(SeqBound::Value(Uid::new(*uid)?)),
            UidSetMember::UidRange(range) => SeqItem::Range(
                SeqBound::Value(Uid::new(*range.start())?),
                SeqBound::Value(Uid::new(*range.end())?),
            ),
        };
        items.push(item);
    }
    UidSet::from_items(items)
}

/// Splits `[CODE args] text` when `imap-proto` did not recognise the code.
///
/// Returns the code and the remaining text, or `None` if the text does not
/// start with a code.
pub(crate) fn parse_code_from_text(text: &str) -> Option<(ResponseCode, String)> {
    let inner = text.strip_prefix('[')?;
    let (code, rest) = inner.split_once(']')?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest).to_owned();
    let (name, args) = code.split_once(' ').unwrap_or((code, ""));
    let name = name.to_ascii_uppercase();
    let parsed = match name.as_str() {
        "CAPABILITY" => ResponseCode::Capabilities(Capabilities::new(
            args.split(' ')
                .filter(|atom| !atom.is_empty())
                .map(Capability::parse),
        )),
        "NOMODSEQ" => ResponseCode::NoModSeq,
        "CLOSED" => ResponseCode::Closed,
        "MODIFIED" => args.parse::<UidSet>().map_or_else(
            |_| ResponseCode::Malformed(name.clone()),
            ResponseCode::Modified,
        ),
        "MAILBOXID" => args
            .strip_prefix('(')
            .and_then(|id| id.strip_suffix(')'))
            .and_then(|id| ObjectId::new(id).ok())
            .map_or_else(
                || ResponseCode::Malformed(name.clone()),
                ResponseCode::MailboxId,
            ),
        "AUTHENTICATIONFAILED" => ResponseCode::AuthenticationFailed,
        "AUTHORIZATIONFAILED" => ResponseCode::AuthorizationFailed,
        "UNAVAILABLE" => ResponseCode::Unavailable,
        "EXPIRED" => ResponseCode::Expired,
        "PRIVACYREQUIRED" => ResponseCode::PrivacyRequired,
        "CONTACTADMIN" => ResponseCode::ContactAdmin,
        "NOPERM" => ResponseCode::NoPerm,
        "INUSE" => ResponseCode::InUse,
        "EXPUNGEISSUED" => ResponseCode::ExpungeIssued,
        "CORRUPTION" => ResponseCode::Corruption,
        "SERVERBUG" => ResponseCode::ServerBug,
        "CLIENTBUG" => ResponseCode::ClientBug,
        "CANNOT" => ResponseCode::Cannot,
        "LIMIT" => ResponseCode::Limit,
        "OVERQUOTA" => ResponseCode::OverQuota,
        "ALREADYEXISTS" => ResponseCode::AlreadyExists,
        "NONEXISTENT" => ResponseCode::NonExistent,
        // Codes `imap-proto` knows but could not parse (bad numbers…).
        "UIDNEXT" | "UIDVALIDITY" | "UNSEEN" | "HIGHESTMODSEQ" | "APPENDUID" | "COPYUID"
        | "PERMANENTFLAGS" | "BADCHARSET" => ResponseCode::Malformed(name),
        _ => ResponseCode::Other(name),
    };
    Some((parsed, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_from_text() {
        let (code, rest) = parse_code_from_text("[NOMODSEQ] Sorry").unwrap();
        assert_eq!(code, ResponseCode::NoModSeq);
        assert_eq!(rest, "Sorry");
        let (code, _) = parse_code_from_text("[MODIFIED 7,9:10] done").unwrap();
        assert_eq!(code, ResponseCode::Modified("7,9:10".parse().unwrap()));
        let (code, _) = parse_code_from_text("[AUTHENTICATIONFAILED] Invalid").unwrap();
        assert_eq!(code, ResponseCode::AuthenticationFailed);
        let (code, _) = parse_code_from_text("[CAPABILITY IMAP4rev2 IDLE]").unwrap();
        assert!(matches!(code, ResponseCode::Capabilities(caps) if caps.has(&Capability::Idle)));
        let (code, _) =
            parse_code_from_text("[MAILBOXID (F2212ea87-6097-4256-9d51-71338625)] Ok").unwrap();
        assert!(matches!(code, ResponseCode::MailboxId(_)));
        let (code, _) = parse_code_from_text("[X-SOMETHING 1 2] x").unwrap();
        assert_eq!(code, ResponseCode::Other("X-SOMETHING".to_owned()));
        assert!(parse_code_from_text("no code").is_none());
    }
}
