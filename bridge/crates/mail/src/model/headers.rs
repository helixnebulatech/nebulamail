// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The header fields the message list needs, parsed with `mail-parser`.

use mail_parser::{AddressList, Message, MessageParser, TextList};
use nebula_imap::proto::command::HeaderName;
use nebula_jmap_proto::object::email::EmailAddress;
use nebula_store::{MAX_THREAD_REFS, SentAt, UnixTime};

/// The fields fetched for every message (`BODY.PEEK[HEADER.FIELDS (...)]`):
/// what the list, threading and the `List-*` actions need. The body and
/// other fields are fetched when the email is opened.
pub const LIST_FIELDS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Bcc",
    "Reply-To",
    "Sender",
    "Subject",
    "Date",
    "Message-ID",
    "In-Reply-To",
    "References",
    "List-Id",
    "List-Unsubscribe",
    "List-Unsubscribe-Post",
    "List-Post",
    "Auto-Submitted",
    "Content-Type",
];

/// Header blocks bigger than this are cut at a field boundary before they
/// are stored: a hostile message can't fill the database.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

pub(crate) fn list_fields() -> Vec<HeaderName> {
    LIST_FIELDS
        .iter()
        .filter_map(|name| HeaderName::new(name).ok())
        .collect()
}

/// What we keep from the header fields (also what `Email/parse` shows
/// of a message in a blob, RFC 8621 §4.9).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ParsedHeaders {
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub subject: Option<String>,
    pub from: Option<Vec<EmailAddress>>,
    pub to: Option<Vec<EmailAddress>>,
    pub cc: Option<Vec<EmailAddress>>,
    pub bcc: Option<Vec<EmailAddress>>,
    pub reply_to: Option<Vec<EmailAddress>>,
    pub sender: Option<Vec<EmailAddress>>,
    pub sent_at: Option<SentAt>,
    /// The raw block, bounded by [`MAX_HEADER_BYTES`].
    pub raw: Vec<u8>,
}

/// Parses a header block (or a whole message: only its header is read).
#[must_use]
pub fn parse(raw: &[u8]) -> ParsedHeaders {
    let bounded = select_fields(raw);
    let raw = bounded.raw;
    let Some(message) = MessageParser::default().parse_headers(&raw) else {
        return ParsedHeaders {
            raw,
            ..ParsedHeaders::default()
        };
    };
    let headers = message.headers();
    // Every From and Sender field, so a second sender is shown (and the
    // client can tell there are two). If one was too big to keep, none is
    // shown: a partial list could pass for the only sender.
    let from = if bounded.skipped_from {
        None
    } else {
        all_addresses(headers.all_from())
    };
    let sender = if bounded.skipped_sender {
        None
    } else {
        all_addresses(
            headers
                .all(mail_parser::HeaderName::Sender)
                .filter_map(|header| header.value().as_address()),
        )
    };
    ParsedHeaders {
        message_id: message
            .message_id()
            .map(clean_id)
            .filter(|id| !id.is_empty()),
        in_reply_to: ids(message.in_reply_to()),
        references: ids(message.references()),
        // `subject` is `header:Subject:asText` (RFC 8621 §4.1.3): the
        // last field, in the Text form (§4.1.2.2).
        subject: headers
            .iter()
            .rfind(|header| header.raw_name().eq_ignore_ascii_case("Subject"))
            .map(|header| super::header_form::text(header.raw_value())),
        from,
        to: addresses(message.to()),
        cc: addresses(message.cc()),
        bcc: addresses(message.bcc()),
        reply_to: addresses(message.reply_to()),
        sender,
        sent_at: sent_at(&message),
        raw: raw.clone(),
    }
}

/// Whether a stored header block is all of the message's header (it ends
/// with the blank line), or [`select_fields`] dropped fields from it. Old
/// blocks cut at [`MAX_HEADER_BYTES`] end in a field too, so they count as
/// cut without migrating anything.
#[must_use]
pub fn header_block_is_complete(block: &[u8]) -> bool {
    block.ends_with(b"\r\n\r\n") || block.ends_with(b"\n\n") || block == b"\r\n" || block == b"\n"
}

/// The header block kept from a message, and which sender fields were
/// dropped from it.
#[derive(Debug, Default)]
struct Bounded {
    raw: Vec<u8>,
    skipped_from: bool,
    skipped_sender: bool,
}

/// The header block itself: up to the blank line, and at most
/// [`MAX_HEADER_BYTES`]. A bigger block keeps, in order, the whole fields
/// that fit and drops the others (a huge `To` doesn't hide the `Subject`
/// after it); it then has no blank line at the end, which is how
/// [`header_block_is_complete`] knows.
fn select_fields(raw: &[u8]) -> Bounded {
    let end = find(raw, b"\r\n\r\n")
        .map(|at| at.saturating_add(4))
        .or_else(|| find(raw, b"\n\n").map(|at| at.saturating_add(2)))
        .unwrap_or(raw.len());
    let block = raw.get(..end).unwrap_or(raw);
    if block.len() <= MAX_HEADER_BYTES {
        return Bounded {
            raw: block.to_vec(),
            ..Bounded::default()
        };
    }
    // Room for the fields, leaving two bytes for the blank line.
    let budget = MAX_HEADER_BYTES.saturating_sub(2);
    let mut bounded = Bounded::default();
    let mut skipped = false;
    let mut terminator: &[u8] = &[];
    let (mut field_start, mut at) = (0, 0);
    let mut fields = Vec::new();
    while at < block.len() {
        let rest = block.get(at..).unwrap_or_default();
        let line_len = rest
            .iter()
            .position(|b| *b == b'\n')
            .map_or(rest.len(), |pos| pos.saturating_add(1));
        let line = rest.get(..line_len).unwrap_or_default();
        if line == b"\r\n" || line == b"\n" {
            terminator = line;
            break;
        }
        let continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if !continuation && at > field_start {
            fields.push(field_start..at);
            field_start = at;
        }
        at = at.saturating_add(line_len);
    }
    if at > field_start {
        fields.push(field_start..at.min(block.len()));
    }
    for range in fields {
        let field = block.get(range).unwrap_or_default();
        if bounded.raw.len().saturating_add(field.len()) <= budget {
            bounded.raw.extend_from_slice(field);
            continue;
        }
        skipped = true;
        let name = field_name(field);
        if name.eq_ignore_ascii_case(b"from") {
            bounded.skipped_from = true;
        } else if name.eq_ignore_ascii_case(b"sender") {
            bounded.skipped_sender = true;
        }
    }
    if !skipped {
        bounded.raw.extend_from_slice(terminator);
    }
    bounded
}

/// The name of a header field, without the blanks some write before `:`.
fn field_name(field: &[u8]) -> &[u8] {
    let name = field
        .iter()
        .position(|b| *b == b':')
        .and_then(|colon| field.get(..colon))
        .unwrap_or(field);
    name.trim_ascii()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn clean_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

fn ids(value: Option<TextList<'_>>) -> Vec<String> {
    let list: Vec<String> = value
        .map(|list| list.iter().map(clean_id).collect())
        .unwrap_or_default();
    // Keep the newest ids: they matter most for threading.
    let skip = list.len().saturating_sub(MAX_THREAD_REFS);
    list.into_iter()
        .skip(skip)
        .filter(|id| !id.is_empty())
        .collect()
}

fn addresses(list: Option<AddressList<'_>>) -> Option<Vec<EmailAddress>> {
    all_addresses(list)
}

/// The addresses of several fields with the same name, in order; `None`
/// if there is no such field.
fn all_addresses<'a>(
    lists: impl IntoIterator<Item = AddressList<'a>>,
) -> Option<Vec<EmailAddress>> {
    let mut found = None;
    for list in lists {
        let addresses: &mut Vec<EmailAddress> = found.get_or_insert_with(Vec::new);
        addresses.extend(list.mailboxes().filter_map(|addr| {
            let email = addr.address()?.trim().to_owned();
            let name = addr
                .name()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
            Some(EmailAddress { name, email })
        }));
    }
    found
}

fn sent_at(message: &Message<'_>) -> Option<SentAt> {
    let date = message.date()?;
    if !date.is_valid() {
        return None;
    }
    let minutes = i16::from(date.tz_hour)
        .saturating_mul(60)
        .saturating_add(i16::from(date.tz_minute));
    let offset_minutes = if date.tz_before_gmt {
        minutes.saturating_neg()
    } else {
        minutes
    };
    Some(SentAt {
        utc: UnixTime::from_secs(date.to_timestamp()),
        offset_minutes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &[u8] = b"From: =?utf-8?q?Jos=C3=A9?= <jose@example.com>\r\n\
        To: alice@example.com, \"Bob B\" <bob@example.com>\r\n\
        Subject: =?utf-8?b?wqFIb2xhIQ==?=\r\n\
        Date: Thu, 24 Oct 2026 10:00:00 +0200\r\n\
        Message-ID: <abc@example.com>\r\n\
        In-Reply-To: <parent@example.com>\r\n\
        References: <root@example.com>\r\n <parent@example.com>\r\n\
        \r\n";

    #[test]
    fn parses_the_list_fields() {
        let parsed = parse(HEADER);
        assert_eq!(parsed.message_id.as_deref(), Some("abc@example.com"));
        assert_eq!(parsed.in_reply_to, vec!["parent@example.com"]);
        assert_eq!(
            parsed.references,
            vec!["root@example.com", "parent@example.com"]
        );
        assert_eq!(parsed.subject.as_deref(), Some("¡Hola!"));
        let from = parsed.from.unwrap();
        assert_eq!(from[0].name.as_deref(), Some("José"));
        assert_eq!(from[0].email, "jose@example.com");
        let to = parsed.to.unwrap();
        assert_eq!(to.len(), 2);
        assert_eq!(to[1].name.as_deref(), Some("Bob B"));
        assert_eq!(parsed.cc, None);
        let sent = parsed.sent_at.unwrap();
        assert_eq!(sent.offset_minutes, 120);
        // 2026-10-24T08:00:00Z
        assert_eq!(sent.utc.secs(), 1_792_828_800);
        assert_eq!(parsed.raw, HEADER);
    }

    #[test]
    fn garbage_and_missing_fields() {
        let parsed = parse(b"Subject: x\r\nDate: not a date\r\n\r\nbody");
        assert_eq!(parsed.subject.as_deref(), Some("x"));
        assert_eq!(parsed.sent_at, None);
        assert_eq!(parsed.raw, b"Subject: x\r\nDate: not a date\r\n\r\n");
        let empty = parse(b"");
        assert_eq!(empty.subject, None);
    }

    #[test]
    fn huge_headers_are_cut_at_a_field() {
        let mut raw = b"Subject: hi\r\n".to_vec();
        raw.extend_from_slice(b"References:");
        for n in 0..10_000 {
            raw.extend_from_slice(format!(" <{n}@x>\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        let parsed = parse(&raw);
        assert!(parsed.raw.len() <= MAX_HEADER_BYTES);
        assert_eq!(parsed.subject.as_deref(), Some("hi"));
        assert!(!header_block_is_complete(&parsed.raw));
    }

    fn folded(name: &str, bytes: usize) -> Vec<u8> {
        let mut field = format!("{name}:").into_bytes();
        let mut n = 0;
        while field.len() < bytes {
            field.extend_from_slice(format!(" <{n}@x.example>\r\n").as_bytes());
            n += 1;
        }
        field
    }

    #[test]
    fn every_from_is_kept() {
        let parsed = parse(b"From: <[EMAIL]>\r\nFrom: <[EMAIL]>\r\n\r\n");
        let from = parsed.from.unwrap();
        let emails: Vec<&str> = from.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(emails, ["[EMAIL]", "[EMAIL]"]);
        assert!(header_block_is_complete(&parsed.raw));

        let mut raw = b"From: <[EMAIL]>\r\n".to_vec();
        raw.extend(folded("References", 70 * 1024));
        raw.extend_from_slice(b"From: <[EMAIL]>\r\nSubject: s\r\n\r\n");
        let parsed = parse(&raw);
        assert_eq!(parsed.from.map(|f| f.len()), Some(2));
        assert_eq!(parsed.subject.as_deref(), Some("s"));
        assert!(!header_block_is_complete(&parsed.raw));
    }

    #[test]
    fn a_huge_first_field_does_not_hide_the_rest() {
        let mut raw = folded("To", 70 * 1024);
        raw.extend_from_slice(b"From: <[EMAIL]>\r\nSubject: s\r\nMessage-ID: <m@y>\r\n\r\nbody");
        let parsed = parse(&raw);
        assert_eq!(parsed.from.unwrap()[0].email, "[EMAIL]");
        assert_eq!(parsed.subject.as_deref(), Some("s"));
        assert_eq!(parsed.message_id.as_deref(), Some("m@y"));
        assert_eq!(parsed.to, None);
        assert!(parsed.raw.len() <= MAX_HEADER_BYTES);
        assert!(!header_block_is_complete(&parsed.raw));
    }

    #[test]
    fn a_dropped_from_leaves_no_sender() {
        // A huge (signed) From and a small forged one: showing only the
        // small one would look like the only sender.
        let mut raw = folded("From", 70 * 1024);
        raw.extend_from_slice(b"From: <[EMAIL]>\r\nSender: <[EMAIL]>\r\n\r\n");
        let parsed = parse(&raw);
        assert_eq!(parsed.from, None);
        assert_eq!(parsed.sender.unwrap()[0].email, "[EMAIL]");

        let mut raw = b"From: <[EMAIL]>\r\n".to_vec();
        raw.extend(folded("sender ", 70 * 1024));
        raw.extend_from_slice(b"\r\n");
        let parsed = parse(&raw);
        assert_eq!(parsed.sender, None);
        assert_eq!(parsed.from.map(|f| f.len()), Some(1));
    }

    #[test]
    fn complete_blocks() {
        assert!(header_block_is_complete(HEADER));
        assert!(header_block_is_complete(b"\r\n"));
        assert!(header_block_is_complete(b"A: b\n\n"));
        assert!(!header_block_is_complete(b"A: b\r\n"));
        assert!(header_block_is_complete(&parse(HEADER).raw));
    }

    #[test]
    fn many_references_keep_the_newest() {
        let mut raw = b"References:".to_vec();
        for n in 0..300 {
            raw.extend_from_slice(format!(" <{n}@x>").as_bytes());
        }
        raw.extend_from_slice(b"\r\n\r\n");
        let parsed = parse(&raw);
        assert_eq!(parsed.references.len(), MAX_THREAD_REFS);
        assert_eq!(parsed.references.last().map(String::as_str), Some("299@x"));
    }
}
