// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! From one complete response (bytes) to our model.
//!
//! 1. Guards that keep hostile input away from the parts of `imap-proto`
//!    that could panic or recurse too deeply (see [`guard`]).
//! 2. Our own small parsers for what `imap-proto` lacks (PREVIEW, SEARCH
//!    with MODSEQ, NAMESPACE, CAPABILITY without `IMAP4rev1`, LIST with
//!    extended data).
//! 3. `imap_proto::Response::parse`, then conversion to validated types.
//!
//! Untagged data we cannot use becomes [`Untagged::Unknown`]; tagged
//! completions are always recovered, so a command never hangs on a response
//! the parser did not like.

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::ops::Range;

use imap_proto::{MailboxDatum, NameAttribute, StatusAttribute};

use super::code::{convert_code, parse_code_from_text};
use super::fetch::{FetchExtras, ObjectId, Preview, ThreadId, convert_fetch, convert_flags};
use super::tokens::{Token, TokenError, TokenLimits, tokenize};
use super::{
    Completion, Condition, Continuation, Invalid, ListEntry, MailboxStatus, NamespaceEntry,
    Namespaces, Quota, QuotaResource, QuotaResourceName, QuotaRoot, Response, ResponseCode,
    SearchResult, StatusResponse, Tagged, Unknown, UnknownKind, UnknownReason, Untagged, Vanished,
    VanishedKind,
};
use crate::types::{
    Capabilities, Capability, MailboxAttribute, MailboxName, ModSeq, NameEncoding, SeqBound,
    SeqItem, SeqNum, Tag, Uid, UidSet, UidValidity,
};

/// Connection settings that change how responses are read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context {
    pub(crate) names: NameEncoding,
    pub(crate) max_depth: usize,
    pub(crate) max_tokens: usize,
}

impl Context {
    pub(crate) fn new(names: NameEncoding, limits: &super::FrameLimits) -> Self {
        Self {
            names,
            max_depth: limits.max_depth,
            max_tokens: limits.max_tokens,
        }
    }
}

/// What the first words of the response say it is.
enum Kind<'a> {
    Continuation,
    Tagged { tag: &'a [u8], status: Completion },
    Status(Condition),
    Untagged { word: String, third: String },
}

pub(crate) fn parse_frame(frame: &[u8], literals: &[Range<usize>], context: &Context) -> Response {
    let unknown = |reason, about| {
        Response::Untagged(Untagged::Unknown(Unknown {
            len: frame.len(),
            reason,
            about,
        }))
    };
    if guard::nesting_depth(frame, literals) > context.max_depth {
        return match classify(frame) {
            Kind::Tagged { tag, status } => fallback_tagged(frame, tag, status),
            Kind::Untagged { word, third } => unknown(
                UnknownReason::TooDeep,
                UnknownKind::from_words(&word, &third),
            ),
            _ => unknown(UnknownReason::TooDeep, UnknownKind::Other),
        };
    }
    match classify(frame) {
        Kind::Continuation | Kind::Status(_) => parse_status_like(frame, literals, context),
        Kind::Tagged { tag, status } => {
            let sanitized = guard::ascii_only(frame, literals);
            match parse_with_imap_proto(&sanitized, context, FetchExtras::default()) {
                Ok(response @ Response::Tagged(_)) => response,
                _ => fallback_tagged(&sanitized, tag, status),
            }
        }
        Kind::Untagged { word, third } => parse_untagged(frame, &word, &third, context)
            .unwrap_or_else(|reason| unknown(reason, UnknownKind::from_words(&word, &third))),
    }
}

/// A response that broke a limit and was dropped: only its first line (cut
/// at 1 KiB) is left. A tagged completion keeps its tag and status (the
/// command must end), and a continuation stays one (AUTHENTICATE and APPEND
/// wait for it); both with the text cut and without response code, which
/// may be incomplete. Anything else is unknown.
pub(crate) fn parse_skipped(prefix: &[u8], len: u64) -> Response {
    let line_end = prefix
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(prefix.len());
    let line = prefix.get(..line_end).unwrap_or_default();
    let text = |words| {
        let line = String::from_utf8_lossy(line);
        rest_after_words(&line, words).to_owned()
    };
    match classify(line) {
        Kind::Continuation => Response::Continuation(Continuation {
            code: None,
            text: text(1),
        }),
        Kind::Tagged { tag, status } => {
            let tag = std::str::from_utf8(tag)
                .ok()
                .and_then(|tag| Tag::new(tag).ok());
            match tag {
                Some(tag) => Response::Tagged(Tagged {
                    tag,
                    status,
                    code: None,
                    text: text(2),
                }),
                None => too_large(len),
            }
        }
        Kind::Status(_) | Kind::Untagged { .. } => too_large(len),
    }
}

fn too_large(len: u64) -> Response {
    Response::Untagged(Untagged::Unknown(Unknown {
        len: usize::try_from(len).unwrap_or(usize::MAX),
        reason: UnknownReason::TooLarge,
        // Only counted as dropped (`TooLarge`), never as an unreadable
        // FETCH or SEARCH result: callers already fail or retry on that.
        about: UnknownKind::Other,
    }))
}

fn classify(frame: &[u8]) -> Kind<'_> {
    let line_end = frame
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(frame.len());
    let line = frame.get(..line_end).unwrap_or_default();
    let mut words = line.split(|&b| b == b' ');
    let first = words.next().unwrap_or_default();
    let second = words.next().unwrap_or_default();
    let third = words.next().unwrap_or_default();
    if first.first() == Some(&b'+') {
        return Kind::Continuation;
    }
    let second_upper = String::from_utf8_lossy(second).to_ascii_uppercase();
    if first == b"*" {
        let condition = match second_upper.as_str() {
            "OK" => Some(Condition::Ok),
            "NO" => Some(Condition::No),
            "BAD" => Some(Condition::Bad),
            "PREAUTH" => Some(Condition::PreAuth),
            "BYE" => Some(Condition::Bye),
            _ => None,
        };
        return match condition {
            Some(condition) => Kind::Status(condition),
            None => Kind::Untagged {
                word: second_upper,
                third: String::from_utf8_lossy(third).to_ascii_uppercase(),
            },
        };
    }
    let status = match second_upper.as_str() {
        "OK" => Completion::Ok,
        "NO" => Completion::No,
        // Anything else from a tagged line (even garbage) ends the command;
        // BAD is the safest reading.
        _ => Completion::Bad,
    };
    Kind::Tagged { tag: first, status }
}

/// `+ ...` and `* OK|NO|BAD|PREAUTH|BYE ...`.
fn parse_status_like(frame: &[u8], literals: &[Range<usize>], context: &Context) -> Response {
    let sanitized = guard::ascii_only(frame, literals);
    if let Ok(response) = parse_with_imap_proto(&sanitized, context, FetchExtras::default()) {
        return response;
    }
    // Fall back to the text after the first space (or two words for `*`).
    let line = String::from_utf8_lossy(&sanitized);
    let line = line.trim_end_matches(['\r', '\n']);
    let (code, text) = split_code(rest_after_words(
        line,
        if line.starts_with('+') { 1 } else { 2 },
    ));
    match classify(frame) {
        Kind::Status(condition) => Response::Untagged(Untagged::Status(StatusResponse {
            condition,
            code,
            text,
        })),
        _ => Response::Continuation(Continuation { code, text }),
    }
}

fn rest_after_words(line: &str, words: usize) -> &str {
    let mut rest = line;
    for _ in 0..words {
        rest = rest.split_once(' ').map_or("", |(_, after)| after);
    }
    rest
}

fn split_code(text: &str) -> (Option<ResponseCode>, String) {
    match parse_code_from_text(text) {
        Some((code, rest)) => (Some(code), rest),
        None => (None, text.to_owned()),
    }
}

/// A tagged response `imap-proto` could not parse: keep tag and status so
/// the command completes.
fn fallback_tagged(frame: &[u8], tag: &[u8], status: Completion) -> Response {
    let line = String::from_utf8_lossy(frame);
    let line = line.trim_end_matches(['\r', '\n']);
    let (code, text) = split_code(rest_after_words(line, 2));
    let tag = std::str::from_utf8(tag)
        .ok()
        .and_then(|tag| Tag::new(tag).ok());
    match tag {
        Some(tag) => Response::Tagged(Tagged {
            tag,
            status,
            code,
            text,
        }),
        None => Response::Untagged(Untagged::Unknown(Unknown {
            len: frame.len(),
            reason: UnknownReason::Unparsed,
            about: UnknownKind::Other,
        })),
    }
}

fn parse_untagged(
    frame: &[u8],
    word: &str,
    third: &str,
    context: &Context,
) -> Result<Response, UnknownReason> {
    let untagged = |data| Ok(Response::Untagged(data));
    match word {
        // `imap-proto` can panic on METADATA with non-UTF-8 data; we do not
        // use METADATA, so never pass it on.
        "METADATA" => Err(UnknownReason::Unsupported),
        "CAPABILITY" => untagged(Untagged::Capabilities(Capabilities::new(atoms_after(
            frame, 2,
        )?))),
        "ENABLED" => untagged(Untagged::Enabled(atoms_after(frame, 2)?)),
        "SEARCH" => untagged(Untagged::Search(own::search(frame)?)),
        "ESEARCH" => untagged(Untagged::ESearch(own::esearch(frame, context)?)),
        "NAMESPACE" => untagged(Untagged::Namespace(own::namespace(frame, context)?)),
        "LIST" | "LSUB" => match parse_with_imap_proto(frame, context, FetchExtras::default()) {
            Ok(response) => Ok(response),
            // LIST-EXTENDED may add data `imap-proto` does not know.
            Err(_) => untagged(Untagged::List(own::list(frame, context)?)),
        },
        _ if third == "FETCH" => {
            // `imap-proto` knows neither BINARY items nor `~{n}` literals.
            if let Some(data) = own::fetch_binary(frame, context)? {
                return untagged(Untagged::Fetch(Box::new(data)));
            }
            let extras = own::fetch_guard(frame, context)?;
            parse_with_imap_proto(frame, context, extras)
        }
        _ => parse_with_imap_proto(frame, context, FetchExtras::default()),
    }
}

fn atoms_after(frame: &[u8], skip: usize) -> Result<Vec<Capability>, UnknownReason> {
    let line = frame.strip_suffix(b"\r\n").ok_or(UnknownReason::Unparsed)?;
    let line = std::str::from_utf8(line).map_err(|_| UnknownReason::Unparsed)?;
    Ok(line
        .split(' ')
        .skip(skip)
        .filter(|atom| !atom.is_empty())
        .map(Capability::parse)
        .collect())
}

fn parse_with_imap_proto(
    frame: &[u8],
    context: &Context,
    extras: FetchExtras,
) -> Result<Response, UnknownReason> {
    // imap-proto 0.17 can panic on hostile input. We check
    // the known cases before calling it; this catches the unknown ones, so a
    // bad server response becomes `Unknown` instead of killing the bridge.
    // Needs `panic = "unwind"` (the workspace release profile).
    let parsed = std::panic::catch_unwind(|| imap_proto::Response::parse(frame));
    match parsed {
        Ok(Ok(([], response))) => convert(response, context, extras)
            .map_err(|Invalid(what)| UnknownReason::InvalidValue(what)),
        _ => Err(UnknownReason::Unparsed),
    }
}

fn convert(
    response: imap_proto::Response<'_>,
    context: &Context,
    extras: FetchExtras,
) -> Result<Response, Invalid> {
    use imap_proto::Response as Proto;
    let untagged = |data| Ok(Response::Untagged(data));
    match response {
        Proto::Continue(outcome) => {
            let (code, text) = convert_outcome(outcome);
            Ok(Response::Continuation(Continuation { code, text }))
        }
        Proto::Done {
            tag,
            status,
            outcome,
        } => {
            let tag = Tag::new(&tag.0).map_err(|_| Invalid("tag"))?;
            let status = match status {
                imap_proto::Status::Ok => Completion::Ok,
                imap_proto::Status::No => Completion::No,
                imap_proto::Status::Bad | imap_proto::Status::PreAuth | imap_proto::Status::Bye => {
                    Completion::Bad
                }
            };
            let (code, text) = convert_outcome(outcome);
            Ok(Response::Tagged(Tagged {
                tag,
                status,
                code,
                text,
            }))
        }
        Proto::Data { status, outcome } => {
            let condition = match status {
                imap_proto::Status::Ok => Condition::Ok,
                imap_proto::Status::No => Condition::No,
                imap_proto::Status::Bad => Condition::Bad,
                imap_proto::Status::PreAuth => Condition::PreAuth,
                imap_proto::Status::Bye => Condition::Bye,
            };
            let (code, text) = convert_outcome(outcome);
            untagged(Untagged::Status(StatusResponse {
                condition,
                code,
                text,
            }))
        }
        Proto::Capabilities(list) => untagged(Untagged::Capabilities(convert_capabilities(&list))),
        Proto::Expunge(seq) => untagged(Untagged::Expunge(
            SeqNum::new(seq).ok_or(Invalid("EXPUNGE"))?,
        )),
        Proto::Vanished { earlier, uids } => {
            let kind = if earlier {
                VanishedKind::Earlier
            } else {
                VanishedKind::Now
            };
            untagged(Untagged::Vanished(Vanished {
                kind,
                uids: ranges_to_uid_set(&uids)?,
            }))
        }
        Proto::Fetch(seq, attributes) => untagged(Untagged::Fetch(Box::new(convert_fetch(
            seq, attributes, extras,
        )?))),
        Proto::MailboxData(datum) => convert_mailbox_data(datum, context).map(Response::Untagged),
        Proto::Quota(quota) => untagged(Untagged::Quota(convert_quota(quota))),
        Proto::QuotaRoot(root) => untagged(Untagged::QuotaRoot(QuotaRoot {
            mailbox: MailboxName::from_wire(&root.mailbox_name, context.names)
                .map_err(|_| Invalid("mailbox name"))?,
            roots: root
                .quota_root_names
                .into_iter()
                .map(Cow::into_owned)
                .collect(),
        })),
        Proto::Id(fields) => untagged(Untagged::Id(fields.map(|fields| {
            let mut fields: Vec<(String, String)> = fields
                .into_iter()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            fields.sort();
            fields
        }))),
        // ACL, METADATA… are not used.
        _ => Err(Invalid("unsupported response")),
    }
}

pub(crate) fn convert_capabilities(list: &[imap_proto::Capability<'_>]) -> Capabilities {
    Capabilities::new(list.iter().map(|capability| match capability {
        imap_proto::Capability::Imap4rev1 => Capability::Imap4Rev1,
        imap_proto::Capability::Auth(mechanism) => Capability::parse(&format!("AUTH={mechanism}")),
        imap_proto::Capability::Atom(atom) => Capability::parse(atom),
    }))
}

fn convert_outcome(outcome: imap_proto::Outcome<'_>) -> (Option<ResponseCode>, String) {
    let text = outcome.information.map(Cow::into_owned).unwrap_or_default();
    match outcome.code {
        Some(code) => (Some(convert_code(code)), text),
        None => split_code(&text),
    }
}

fn ranges_to_uid_set(ranges: &[std::ops::RangeInclusive<u32>]) -> Result<UidSet, Invalid> {
    let items = ranges
        .iter()
        .map(|range| {
            let first = Uid::new(*range.start()).ok_or(Invalid("UID"))?;
            let last = Uid::new(*range.end()).ok_or(Invalid("UID"))?;
            Ok(if first == last {
                SeqItem::Single(SeqBound::Value(first))
            } else {
                SeqItem::Range(SeqBound::Value(first), SeqBound::Value(last))
            })
        })
        .collect::<Result<Vec<_>, Invalid>>()?;
    UidSet::from_items(items).ok_or(Invalid("UID set"))
}

fn convert_mailbox_data(datum: MailboxDatum<'_>, context: &Context) -> Result<Untagged, Invalid> {
    Ok(match datum {
        MailboxDatum::Exists(count) => Untagged::Exists(count),
        MailboxDatum::Recent(count) => Untagged::Recent(count),
        MailboxDatum::Flags(flags) => {
            // Some servers put `\*` in FLAGS too; it only makes sense in
            // PERMANENTFLAGS.
            let flags: Vec<Cow<'_, str>> = flags.into_iter().filter(|flag| flag != "\\*").collect();
            Untagged::Flags(convert_flags(&flags))
        }
        MailboxDatum::List(list) => Untagged::List(ListEntry {
            attributes: list.name_attributes.iter().map(convert_attribute).collect(),
            delimiter: convert_delimiter(list.delimiter.as_deref())?,
            name: MailboxName::from_wire(&list.name, context.names)
                .map_err(|_| Invalid("mailbox name"))?,
        }),
        MailboxDatum::Status { mailbox, status } => {
            Untagged::MailboxStatus(convert_status(&mailbox, status, context)?)
        }
        MailboxDatum::Search(numbers) => Untagged::Search(SearchResult {
            numbers: numbers
                .into_iter()
                .map(|n| NonZeroU32::new(n).ok_or(Invalid("SEARCH")))
                .collect::<Result<_, _>>()?,
            highest_mod_seq: None,
        }),
        _ => return Err(Invalid("unsupported mailbox data")),
    })
}

fn convert_attribute(attribute: &NameAttribute<'_>) -> MailboxAttribute {
    use crate::types::SpecialUse;
    match attribute {
        NameAttribute::NoInferiors => MailboxAttribute::NoInferiors,
        NameAttribute::NoSelect => MailboxAttribute::NoSelect,
        NameAttribute::Marked => MailboxAttribute::Marked,
        NameAttribute::Unmarked => MailboxAttribute::Unmarked,
        NameAttribute::All => MailboxAttribute::SpecialUse(SpecialUse::All),
        NameAttribute::Archive => MailboxAttribute::SpecialUse(SpecialUse::Archive),
        NameAttribute::Drafts => MailboxAttribute::SpecialUse(SpecialUse::Drafts),
        NameAttribute::Flagged => MailboxAttribute::SpecialUse(SpecialUse::Flagged),
        NameAttribute::Junk => MailboxAttribute::SpecialUse(SpecialUse::Junk),
        NameAttribute::Sent => MailboxAttribute::SpecialUse(SpecialUse::Sent),
        NameAttribute::Trash => MailboxAttribute::SpecialUse(SpecialUse::Trash),
        NameAttribute::Extension(name) => MailboxAttribute::parse(name),
        // Non-exhaustive enum: new attributes become `Other`.
        _ => MailboxAttribute::Other(String::new()),
    }
}

fn convert_delimiter(delimiter: Option<&str>) -> Result<Option<char>, Invalid> {
    let Some(delimiter) = delimiter else {
        return Ok(None);
    };
    let mut chars = delimiter.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(Some(ch)),
        _ => Err(Invalid("hierarchy delimiter")),
    }
}

fn convert_status(
    mailbox: &str,
    items: Vec<StatusAttribute>,
    context: &Context,
) -> Result<MailboxStatus, Invalid> {
    let mut status = MailboxStatus {
        mailbox: MailboxName::from_wire(mailbox, context.names)
            .map_err(|_| Invalid("mailbox name"))?,
        messages: None,
        recent: None,
        uid_next: None,
        uid_validity: None,
        unseen: None,
        highest_mod_seq: None,
    };
    for item in items {
        match item {
            StatusAttribute::Messages(n) => status.messages = Some(n),
            StatusAttribute::Recent(n) => status.recent = Some(n),
            StatusAttribute::Unseen(n) => status.unseen = Some(n),
            StatusAttribute::UidNext(n) => {
                status.uid_next = Some(Uid::new(n).ok_or(Invalid("UIDNEXT"))?);
            }
            StatusAttribute::UidValidity(n) => {
                status.uid_validity = Some(UidValidity::new(n).ok_or(Invalid("UIDVALIDITY"))?);
            }
            // RFC 7162 allows HIGHESTMODSEQ 0 in STATUS for a mailbox
            // without mod-sequences yet: treat it as "none".
            StatusAttribute::HighestModSeq(n) => status.highest_mod_seq = ModSeq::new(n),
            _ => {}
        }
    }
    Ok(status)
}

fn convert_quota(quota: imap_proto::rfc2087::Quota<'_>) -> Quota {
    use imap_proto::rfc2087::QuotaResourceName as Name;
    Quota {
        root: quota.root_name.into_owned(),
        resources: quota
            .resources
            .into_iter()
            .map(|resource| QuotaResource {
                name: match resource.name {
                    Name::Storage => QuotaResourceName::Storage,
                    Name::Message => QuotaResourceName::Message,
                    Name::Atom(atom) => QuotaResourceName::Other(atom.to_ascii_uppercase()),
                },
                usage: resource.usage,
                limit: resource.limit,
            })
            .collect(),
    }
}

/// Our own parsers, on top of the tokenizer.
mod own {
    use super::{
        Context, FetchExtras, Invalid, ListEntry, MailboxAttribute, MailboxName, ModSeq,
        NamespaceEntry, Namespaces, NonZeroU32, ObjectId, Preview, SearchResult, Tag, ThreadId,
        Token, TokenError, TokenLimits, UnknownReason, convert_delimiter, tokenize,
    };
    use crate::response::fetch::convert_flags;
    use crate::response::{BinarySection, ESearchAll, ESearchResult, FetchData};
    use crate::types::{SeqNum, Uid};

    fn tokens<'a>(frame: &'a [u8], context: &Context) -> Result<Vec<Token<'a>>, UnknownReason> {
        let limits = TokenLimits {
            max_depth: context.max_depth,
            max_tokens: context.max_tokens,
        };
        tokenize(frame, limits).map_err(|error| match error {
            TokenError::TooDeep => UnknownReason::TooDeep,
            TokenError::Malformed | TokenError::TooMany => UnknownReason::Unparsed,
        })
    }

    fn text(token: &Token<'_>) -> Result<String, UnknownReason> {
        let bytes = token.string_bytes().ok_or(UnknownReason::Unparsed)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| UnknownReason::InvalidValue("string"))
    }

    fn nz_number(token: &Token<'_>) -> Result<NonZeroU32, UnknownReason> {
        token
            .atom()
            .and_then(crate::types::parse_digits_u32)
            .and_then(NonZeroU32::new)
            .ok_or(UnknownReason::InvalidValue("number"))
    }

    /// `* SEARCH 2 5 [(MODSEQ 917)]`, read in one linear pass (no token
    /// tree: a line may hold a million UIDs, see
    /// [`FrameLimits::max_search_line`](super::super::FrameLimits)).
    /// Repeated spaces and a trailing space are accepted; anything else
    /// than numbers and one final `(MODSEQ n)` is refused.
    pub(super) fn search(frame: &[u8]) -> Result<SearchResult, UnknownReason> {
        let line = frame
            .strip_suffix(b"\r\n")
            .or_else(|| frame.strip_suffix(b"\n"))
            .ok_or(UnknownReason::Unparsed)?;
        // `* SEARCH`, any case (checked by the caller).
        let rest = line.get(8..).ok_or(UnknownReason::Unparsed)?;
        let (numbers, mod_seq) = match rest.iter().position(|&b| b == b'(') {
            Some(open) => (
                rest.get(..open).unwrap_or_default(),
                Some(rest.get(open..).unwrap_or_default()),
            ),
            None => (rest, None),
        };
        let mut result = SearchResult {
            numbers: Vec::new(),
            highest_mod_seq: None,
        };
        for word in numbers.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
            let number = std::str::from_utf8(word)
                .ok()
                .and_then(crate::types::parse_digits_u32)
                .and_then(NonZeroU32::new)
                .ok_or(UnknownReason::InvalidValue("number"))?;
            result.numbers.push(number);
        }
        if let Some(mod_seq) = mod_seq {
            let trimmed = mod_seq.trim_ascii_end();
            let inner = trimmed
                .strip_prefix(b"(")
                .and_then(|m| m.strip_suffix(b")"))
                .ok_or(UnknownReason::Unparsed)?;
            let mut words = inner.split(|&b| b == b' ').filter(|w| !w.is_empty());
            let (Some(name), Some(value), None) = (words.next(), words.next(), words.next()) else {
                return Err(UnknownReason::Unparsed);
            };
            if !name.eq_ignore_ascii_case(b"MODSEQ") {
                return Err(UnknownReason::Unparsed);
            }
            let value = std::str::from_utf8(value)
                .ok()
                .and_then(|v| v.parse::<ModSeq>().ok());
            result.highest_mod_seq = Some(value.ok_or(UnknownReason::InvalidValue("MODSEQ"))?);
        }
        Ok(result)
    }

    /// The token-based reading of `* SEARCH` used before the linear one;
    /// kept to check that both agree on valid input.
    #[cfg(test)]
    pub(super) fn search_with_tokens(
        frame: &[u8],
        context: &Context,
    ) -> Result<SearchResult, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let mut result = SearchResult {
            numbers: Vec::new(),
            highest_mod_seq: None,
        };
        for token in tokens.iter().skip(2) {
            if let Some(items) = token.list() {
                let [name, value] = items else {
                    return Err(UnknownReason::Unparsed);
                };
                if !name
                    .atom()
                    .is_some_and(|n| n.eq_ignore_ascii_case("MODSEQ"))
                {
                    return Err(UnknownReason::Unparsed);
                }
                let value = value.atom().and_then(|v| v.parse::<ModSeq>().ok());
                result.highest_mod_seq = Some(value.ok_or(UnknownReason::InvalidValue("MODSEQ"))?);
            } else {
                result.numbers.push(nz_number(token)?);
            }
        }
        Ok(result)
    }

    /// `* ESEARCH (TAG "A1") UID ALL 1:3,5 MODSEQ 917`
    pub(super) fn esearch(frame: &[u8], context: &Context) -> Result<ESearchResult, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let mut rest = tokens.get(2..).unwrap_or_default();
        let mut result = ESearchResult {
            tag: None,
            uid: false,
            min: None,
            max: None,
            count: None,
            all: None,
            mod_seq: None,
        };
        if let Some((first, after)) = rest.split_first()
            && let Some(items) = first.list()
        {
            // `(TAG "A1")`: a tag we can't read is just not kept.
            if let [name, value] = items
                && name.atom().is_some_and(|n| n.eq_ignore_ascii_case("TAG"))
            {
                result.tag = value
                    .string_bytes()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .and_then(|tag| Tag::new(tag).ok());
            }
            rest = after;
        }
        if let Some((first, after)) = rest.split_first()
            && first.atom().is_some_and(|a| a.eq_ignore_ascii_case("UID"))
        {
            result.uid = true;
            rest = after;
        }
        for pair in rest.chunks(2) {
            let [name, value] = pair else {
                return Err(UnknownReason::Unparsed);
            };
            let name = name.atom().ok_or(UnknownReason::Unparsed)?;
            let number = || {
                value
                    .atom()
                    .and_then(crate::types::parse_digits_u32)
                    .ok_or(UnknownReason::InvalidValue("ESEARCH number"))
            };
            match name.to_ascii_uppercase().as_str() {
                "MIN" => result.min = Some(nz_number(value)?),
                "MAX" => result.max = Some(nz_number(value)?),
                "COUNT" => result.count = Some(number()?),
                "ALL" => {
                    let text = value.atom().ok_or(UnknownReason::Unparsed)?;
                    let invalid = |_| UnknownReason::InvalidValue("ESEARCH ALL");
                    let all = if result.uid {
                        ESearchAll::Uids(text.parse().map_err(invalid)?)
                    } else {
                        ESearchAll::SeqNums(text.parse().map_err(invalid)?)
                    };
                    let has_star = match &all {
                        ESearchAll::Uids(set) => set.has_star(),
                        ESearchAll::SeqNums(set) => set.has_star(),
                    };
                    if has_star {
                        return Err(UnknownReason::InvalidValue("ESEARCH ALL"));
                    }
                    result.all = Some(all);
                }
                "MODSEQ" => {
                    let parsed = value.atom().and_then(|v| v.parse::<ModSeq>().ok());
                    result.mod_seq = Some(parsed.ok_or(UnknownReason::InvalidValue("MODSEQ"))?);
                }
                // Other return data (PARTIAL, RELEVANCY…): one value each.
                _ => {}
            }
        }
        Ok(result)
    }

    /// `* NAMESPACE (("" "/")) NIL (("Shared/" "/"))`
    pub(super) fn namespace(frame: &[u8], context: &Context) -> Result<Namespaces, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let [_, _, personal, other_users, shared] = tokens.as_slice() else {
            return Err(UnknownReason::Unparsed);
        };
        Ok(Namespaces {
            personal: namespace_list(personal, context)?,
            other_users: namespace_list(other_users, context)?,
            shared: namespace_list(shared, context)?,
        })
    }

    fn namespace_list(
        token: &Token<'_>,
        context: &Context,
    ) -> Result<Vec<NamespaceEntry>, UnknownReason> {
        if token.is_nil() {
            return Ok(Vec::new());
        }
        let entries = token.list().ok_or(UnknownReason::Unparsed)?;
        entries
            .iter()
            .map(|entry| {
                let items = entry.list().ok_or(UnknownReason::Unparsed)?;
                // Extensions may follow prefix and delimiter: ignore them.
                let (Some(prefix), Some(delimiter)) = (items.first(), items.get(1)) else {
                    return Err(UnknownReason::Unparsed);
                };
                let prefix = text(prefix)?;
                // An empty prefix is valid; decode like a mailbox name.
                let prefix = match MailboxName::from_wire(&prefix, context.names) {
                    Ok(name) => name.as_str().to_owned(),
                    Err(_) => prefix,
                };
                Ok(NamespaceEntry {
                    prefix,
                    delimiter: delimiter_token(delimiter)?,
                })
            })
            .collect()
    }

    fn delimiter_token(token: &Token<'_>) -> Result<Option<char>, UnknownReason> {
        if token.is_nil() {
            return Ok(None);
        }
        let delimiter = text(token)?;
        convert_delimiter(Some(&delimiter))
            .map_err(|Invalid(what)| UnknownReason::InvalidValue(what))
    }

    /// `* LIST (attributes) "/" name [extended data]`
    pub(super) fn list(frame: &[u8], context: &Context) -> Result<ListEntry, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let (Some(attributes), Some(delimiter), Some(name)) =
            (tokens.get(2), tokens.get(3), tokens.get(4))
        else {
            return Err(UnknownReason::Unparsed);
        };
        let attributes = attributes
            .list()
            .ok_or(UnknownReason::Unparsed)?
            .iter()
            .map(|attribute| {
                attribute
                    .atom()
                    .map(MailboxAttribute::parse)
                    .ok_or(UnknownReason::Unparsed)
            })
            .collect::<Result<_, _>>()?;
        let name = MailboxName::from_wire(&text(name)?, context.names)
            .map_err(|_| UnknownReason::InvalidValue("mailbox name"))?;
        Ok(ListEntry {
            attributes,
            delimiter: delimiter_token(delimiter)?,
            name,
        })
    }

    /// FETCH items that `imap-proto` reads with a parser of its own. Any
    /// other item goes to its catch-all `msg_att_unknown`, which takes a
    /// list value as "`(` up to the first `)`" and a bare value as "up to
    /// a space or `)`", without looking at literals or quotes.
    fn parsed_by_imap_proto(name: &str) -> bool {
        matches!(
            name,
            "UID"
                | "FLAGS"
                | "INTERNALDATE"
                | "RFC822"
                | "RFC822.HEADER"
                | "RFC822.SIZE"
                | "RFC822.TEXT"
                | "BODY"
                | "BODYSTRUCTURE"
                | "ENVELOPE"
                | "MODSEQ"
                | "X-GM-LABELS"
                | "X-GM-MSGID"
                | "X-GM-THRID"
                | "EMAILID"
                | "THREADID"
        ) || name.starts_with("BODY[")
    }

    /// Items we read ourselves (`PREVIEW` is checked to be a string).
    fn read_by_us(name: &str) -> bool {
        name == "PREVIEW" || name.starts_with("BINARY")
    }

    /// The value of an item that `imap-proto` skips with its catch-all
    /// must be flat: an atom (or NIL), a quoted string, or a list of atoms.
    /// With a literal or a quoted `)` inside a list, its "up to the first
    /// `)`" would stop inside the data, and it would go on parsing the
    /// literal as protocol (a hidden, deeply nested BODYSTRUCTURE that
    /// overflows the stack).
    fn is_flat(value: &Token<'_>) -> bool {
        match value {
            Token::Atom(_) | Token::Quoted(_) => true,
            Token::Literal(_) => false,
            Token::List(items) => items.iter().all(|item| matches!(item, Token::Atom(_))),
        }
    }

    /// Literals inside lists (a file name in a BODYSTRUCTURE…). If
    /// `imap-proto` fails on a known item it retries it with the catch-all,
    /// which may stop inside such a literal and read the rest as protocol;
    /// so the parentheses of the literal count towards the depth limit,
    /// from where the literal is.
    fn nested_literals_fit(token: &Token<'_>, depth: usize, max_depth: usize) -> bool {
        match token {
            Token::List(items) => {
                let inner = depth.saturating_add(1);
                items
                    .iter()
                    .all(|item| nested_literals_fit(item, inner, max_depth))
            }
            Token::Literal(bytes) if depth >= 2 => {
                depth.saturating_add(super::guard::raw_depth(bytes)) <= max_depth
            }
            Token::Atom(_) | Token::Quoted(_) | Token::Literal(_) => true,
        }
    }

    /// The FETCH item list, checked so that `imap-proto` sees the same
    /// structure as our tokenizer (see [`is_flat`], [`nested_literals_fit`]).
    fn fetch_items<'t>(
        tokens: &'t [Token<'t>],
        context: &Context,
    ) -> Result<&'t [Token<'t>], UnknownReason> {
        let list = tokens.get(3).ok_or(UnknownReason::Unparsed)?;
        let items = list.list().ok_or(UnknownReason::Unparsed)?;
        if !nested_literals_fit(list, 0, context.max_depth) {
            return Err(UnknownReason::TooDeep);
        }
        for pair in items.chunks(2) {
            let [name, value] = pair else {
                return Err(UnknownReason::Unparsed);
            };
            let name = name
                .atom()
                .ok_or(UnknownReason::Unparsed)?
                .to_ascii_uppercase();
            if !parsed_by_imap_proto(&name) && !read_by_us(&name) && !is_flat(value) {
                return Err(UnknownReason::Unparsed);
            }
        }
        Ok(items)
    }

    /// Looks at a FETCH before `imap-proto` does: reads PREVIEW and
    /// `OBJECTID (...)` (which it drops), refuses `INTERNALDATE NIL` (on
    /// which it panics) and values it would read differently from us.
    pub(super) fn fetch_guard(
        frame: &[u8],
        context: &Context,
    ) -> Result<FetchExtras, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let items = fetch_items(&tokens, context)?;
        let mut extras = FetchExtras::default();
        for pair in items.chunks(2) {
            let [name, value] = pair else {
                return Err(UnknownReason::Unparsed);
            };
            let name = name
                .atom()
                .ok_or(UnknownReason::Unparsed)?
                .to_ascii_uppercase();
            match name.as_str() {
                "INTERNALDATE" if value.is_nil() => {
                    return Err(UnknownReason::InvalidValue("INTERNALDATE"));
                }
                "PREVIEW" => {
                    extras.preview = Some(if value.is_nil() {
                        Preview::None
                    } else {
                        let bytes = value.string_bytes().ok_or(UnknownReason::Unparsed)?;
                        Preview::Text(String::from_utf8_lossy(bytes).into_owned())
                    });
                }
                "OBJECTID" => object_ids(value, &mut extras)?,
                _ => {}
            }
        }
        Ok(extras)
    }

    /// A FETCH with `BINARY[...]` or `BINARY.SIZE[...]` items, read with our
    /// own parser (`None` if it has none). Besides those it reads UID,
    /// FLAGS, MODSEQ and RFC822.SIZE; other items are skipped, so callers
    /// ask for BINARY in a FETCH of its own.
    pub(super) fn fetch_binary(
        frame: &[u8],
        context: &Context,
    ) -> Result<Option<FetchData>, UnknownReason> {
        let tokens = tokens(frame, context)?;
        let items = fetch_items(&tokens, context)?;
        let is_binary = |token: &Token<'_>| {
            token.atom().is_some_and(|name| {
                name.get(..6)
                    .is_some_and(|start| start.eq_ignore_ascii_case("BINARY"))
            })
        };
        if !items.iter().step_by(2).any(is_binary) {
            return Ok(None);
        }
        let seq = tokens
            .get(1)
            .map(nz_number)
            .transpose()?
            .map(SeqNum::from)
            .ok_or(UnknownReason::Unparsed)?;
        let mut data = FetchData::empty(seq);
        for pair in items.chunks(2) {
            let [name, value] = pair else {
                return Err(UnknownReason::Unparsed);
            };
            let name = name.atom().ok_or(UnknownReason::Unparsed)?;
            let upper = name.to_ascii_uppercase();
            if let Some(rest) = upper.strip_prefix("BINARY.SIZE[") {
                let (part, origin) = binary_section(rest)?;
                if origin.is_some() {
                    return Err(UnknownReason::Unparsed);
                }
                let size = value
                    .atom()
                    .and_then(crate::types::parse_digits_u32)
                    .ok_or(UnknownReason::InvalidValue("BINARY.SIZE"))?;
                data.binary_sizes.push((part, size));
            } else if let Some(rest) = upper.strip_prefix("BINARY[") {
                let (part, origin) = binary_section(rest)?;
                let bytes = if value.is_nil() {
                    None
                } else {
                    Some(
                        value
                            .string_bytes()
                            .ok_or(UnknownReason::Unparsed)?
                            .to_vec(),
                    )
                };
                data.binary.push(BinarySection {
                    part,
                    origin,
                    data: bytes,
                });
            } else {
                match upper.as_str() {
                    "UID" => data.uid = Some(Uid::from(nz_number(value)?)),
                    "RFC822.SIZE" => {
                        data.size = Some(
                            value
                                .atom()
                                .and_then(crate::types::parse_digits_u32)
                                .ok_or(UnknownReason::InvalidValue("RFC822.SIZE"))?,
                        );
                    }
                    "MODSEQ" => {
                        let mod_seq = value
                            .list()
                            .and_then(|list| list.first())
                            .and_then(Token::atom)
                            .and_then(|text| text.parse::<ModSeq>().ok())
                            .ok_or(UnknownReason::InvalidValue("MODSEQ"))?;
                        data.mod_seq = Some(mod_seq);
                    }
                    "FLAGS" => {
                        let list = value.list().ok_or(UnknownReason::Unparsed)?;
                        let flags = list
                            .iter()
                            .map(|flag| flag.atom().ok_or(UnknownReason::InvalidValue("flag")))
                            .collect::<Result<Vec<_>, _>>()?;
                        data.flags = Some(convert_flags(&flags));
                    }
                    _ => {}
                }
            }
        }
        Ok(Some(data))
    }

    /// `1.2]<0>` (after `BINARY[`): part numbers and origin.
    fn binary_section(rest: &str) -> Result<(Vec<u32>, Option<u32>), UnknownReason> {
        let invalid = UnknownReason::InvalidValue("BINARY section");
        let (part, after) = rest.split_once(']').ok_or(invalid)?;
        let numbers = if part.is_empty() {
            Vec::new()
        } else {
            part.split('.')
                .map(|n| {
                    crate::types::parse_digits_u32(n)
                        .filter(|&n| n > 0)
                        .ok_or(invalid)
                })
                .collect::<Result<_, _>>()?
        };
        let origin = if after.is_empty() {
            None
        } else {
            let inner = after
                .strip_prefix('<')
                .and_then(|a| a.strip_suffix('>'))
                .ok_or(invalid)?;
            Some(crate::types::parse_digits_u32(inner).ok_or(invalid)?)
        };
        Ok((numbers, origin))
    }

    /// `OBJECTID (EMAILID x THREADID y)` of the `OBJECTID+` draft.
    fn object_ids(value: &Token<'_>, extras: &mut FetchExtras) -> Result<(), UnknownReason> {
        let pairs = value.list().ok_or(UnknownReason::Unparsed)?;
        for pair in pairs.chunks(2) {
            let [key, id] = pair else {
                return Err(UnknownReason::Unparsed);
            };
            let key = key.atom().ok_or(UnknownReason::Unparsed)?;
            let parse = |what| {
                let text = id.atom().ok_or(UnknownReason::InvalidValue(what))?;
                ObjectId::new(text).map_err(|_| UnknownReason::InvalidValue(what))
            };
            if key.eq_ignore_ascii_case("EMAILID") {
                extras.email_id = Some(parse("EMAILID")?);
            } else if key.eq_ignore_ascii_case("THREADID") {
                extras.thread_id = Some(if id.is_nil() {
                    ThreadId::None
                } else {
                    ThreadId::Id(parse("THREADID")?)
                });
            }
        }
        Ok(())
    }
}

/// Checks done on the raw bytes before `imap-proto` sees them.
mod guard {
    use std::borrow::Cow;
    use std::ops::Range;

    /// Deepest nesting of `(` in raw bytes (quotes not looked at).
    pub(super) fn raw_depth(bytes: &[u8]) -> usize {
        let mut depth: usize = 0;
        let mut deepest: usize = 0;
        for &byte in bytes {
            match byte {
                b'(' => {
                    depth = depth.saturating_add(1);
                    deepest = deepest.max(depth);
                }
                b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        deepest
    }

    /// Deepest nesting of `(` outside literals. Quotes are ignored on
    /// purpose: counting them could let a crafted response hide parentheses
    /// that the real parser does see. For example the catch-all of
    /// `imap-proto` for unknown FETCH items takes `a"b` whole, so a counter
    /// that switched state at `"` would think a string starts there and
    /// skip the `(((` that `imap-proto` then parses. A string with many
    /// `(` (a PREVIEW, a quoted header) is therefore refused as `TooDeep`
    /// by design; the client asks again without those items.
    pub(super) fn nesting_depth(frame: &[u8], literals: &[Range<usize>]) -> usize {
        let mut depth: usize = 0;
        let mut deepest: usize = 0;
        outside_literals(frame, literals, |_, bytes| {
            for &byte in bytes {
                match byte {
                    b'(' => {
                        depth = depth.saturating_add(1);
                        deepest = deepest.max(depth);
                    }
                    b')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        });
        deepest
    }

    /// Calls `f` with each run of bytes outside literals and its offset.
    /// Literal ranges are sorted, as the framing produces them.
    fn outside_literals(frame: &[u8], literals: &[Range<usize>], mut f: impl FnMut(usize, &[u8])) {
        let mut start = 0;
        for range in literals {
            f(start, frame.get(start..range.start).unwrap_or_default());
            start = range.end;
        }
        f(start, frame.get(start..).unwrap_or_default());
    }

    /// Replaces bytes above 0x7F outside literals with `?`.
    ///
    /// `imap-proto` slices the text after a response code at byte 1, which
    /// panics if that byte starts a multi-byte character. Status texts are
    /// for humans only, so ASCII is enough.
    pub(super) fn ascii_only<'a>(frame: &'a [u8], literals: &[Range<usize>]) -> Cow<'a, [u8]> {
        let mut positions = Vec::new();
        outside_literals(frame, literals, |offset, bytes| {
            for (index, &byte) in bytes.iter().enumerate() {
                if byte > 0x7f {
                    positions.push(offset.saturating_add(index));
                }
            }
        });
        if positions.is_empty() {
            return Cow::Borrowed(frame);
        }
        let mut owned = frame.to_vec();
        for position in positions {
            if let Some(byte) = owned.get_mut(position) {
                *byte = b'?';
            }
        }
        Cow::Owned(owned)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use proptest::prelude::*;

    use super::{Context, own};
    use crate::response::FrameLimits;
    use crate::types::NameEncoding;

    proptest! {
        /// The linear SEARCH parser agrees with the token-based one on
        /// valid lines.
        #[test]
        fn linear_search_matches_tokens(
            numbers in proptest::collection::vec(1u32.., 0..50),
            mod_seq in proptest::option::of(1u64..(1 << 62)),
            lower in any::<bool>(),
        ) {
            let mut line = if lower { "* search".to_owned() } else { "* SEARCH".to_owned() };
            for number in &numbers {
                write!(line, " {number}").unwrap();
            }
            if let Some(mod_seq) = mod_seq {
                write!(line, " (MODSEQ {mod_seq})").unwrap();
            }
            line.push_str("\r\n");
            let context = Context::new(NameEncoding::ModifiedUtf7, &FrameLimits::default());
            let linear = own::search(line.as_bytes());
            let tokens = own::search_with_tokens(line.as_bytes(), &context);
            prop_assert!(linear.is_ok());
            prop_assert_eq!(linear, tokens);
        }
    }
}
