// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Delivery reports (DSN, RFC 3464) and read receipts (MDN, RFC 8098)
//! that arrive in the mail, read strictly: they come from anyone.
//!
//! A report is a `multipart/report` with a machine-readable part
//! (`message/delivery-status` or `message/disposition-notification`, or
//! their `global` forms of RFC 6533) and, usually, the original message or
//! its header. Only those two parts are fetched (bounded), never the whole
//! report. The fields are header-like lines in blocks separated by blank
//! lines (one per message, then one per recipient for a DSN).
//!
//! Matching to submissions, and what may change, is the store's
//! (`nebula_store::DeliveryReport`).

use std::num::NonZeroU32;

use mail_parser::MessageParser;
use nebula_imap::proto::response::{BodyStructure, SinglePart};
use nebula_store::{RecipientReport, ReportAction, ReportKind};

/// Largest machine-readable part read (a real one is a few hundred bytes
/// per recipient).
pub(crate) const MAX_STATUS_BYTES: u32 = 64 * 1024;
/// Bytes of the returned message read to find its `Message-ID`.
pub(crate) const MAX_RETURNED_HEADER_BYTES: u32 = 32 * 1024;
/// Recipients read from one DSN.
const MAX_RECIPIENTS: usize = 100;
/// Longest SMTP reply kept.
const MAX_REPLY_CHARS: usize = 512;
/// Longest address or id kept (RFC 5321 paths are at most 256).
const MAX_ADDRESS: usize = 320;

/// Which report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportType {
    Delivery,
    Disposition,
}

/// Whether a header block says this is a report, and which.
pub(crate) fn report_type(header: &[u8]) -> Option<ReportType> {
    let parsed = MessageParser::default().parse_headers(header)?;
    let content_type = parsed.content_type()?;
    if !content_type.ctype().eq_ignore_ascii_case("multipart")
        || !content_type
            .subtype()
            .is_some_and(|s| s.eq_ignore_ascii_case("report"))
    {
        return None;
    }
    match content_type
        .attribute("report-type")?
        .to_ascii_lowercase()
        .as_str()
    {
        "delivery-status" => Some(ReportType::Delivery),
        "disposition-notification" => Some(ReportType::Disposition),
        _ => None,
    }
}

/// Where the parts of a report are: the machine-readable part and the
/// returned message (or header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportParts<'a> {
    pub(crate) status: (Vec<NonZeroU32>, &'a SinglePart),
    pub(crate) returned: Option<Vec<NonZeroU32>>,
}

fn kind(part: &SinglePart) -> String {
    format!("{}/{}", part.common.media_type, part.common.subtype)
}

/// The parts of a report from its BODYSTRUCTURE: direct children of the
/// top `multipart/report` only.
pub(crate) fn report_parts(
    structure: &BodyStructure,
    wanted: ReportType,
) -> Option<ReportParts<'_>> {
    let BodyStructure::Multipart(report) = structure else {
        return None;
    };
    if report.common.subtype != "report" {
        return None;
    }
    let status_types: &[&str] = match wanted {
        ReportType::Delivery => &["message/delivery-status", "message/global-delivery-status"],
        ReportType::Disposition => &[
            "message/disposition-notification",
            "message/global-disposition-notification",
        ],
    };
    let returned_types = [
        "message/rfc822",
        "message/global",
        "text/rfc822-headers",
        "message/global-headers",
    ];
    let mut status = None;
    let mut returned = None;
    for (index, child) in report.parts.iter().enumerate() {
        let BodyStructure::Single(part) = child else {
            continue;
        };
        let number = u32::try_from(index)
            .ok()
            .and_then(|n| n.checked_add(1))
            .and_then(NonZeroU32::new)?;
        let kind = kind(part);
        if status.is_none() && status_types.contains(&kind.as_str()) {
            status = Some((vec![number], part.as_ref()));
        } else if returned.is_none() && returned_types.contains(&kind.as_str()) {
            returned = Some(vec![number]);
        }
    }
    Some(ReportParts {
        status: status?,
        returned,
    })
}

/// One block of fields: lower-cased names and unfolded values.
type Block = Vec<(String, String)>;

/// Splits the machine-readable part into blocks of fields. Lines that are
/// not fields are skipped; folded lines are joined.
fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut current: Block = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = current.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic()) {
                current.push((name.to_ascii_lowercase(), value.trim().to_owned()));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn field<'a>(block: &'a Block, name: &str) -> Option<&'a str> {
    block
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Text without control characters, cut to `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// `rfc822; bob@example.com` (or `utf-8;`) → `bob@example.com`, lower
/// case. Other address types are not addresses we sent to.
fn address(value: &str) -> Option<String> {
    let (kind, address) = value.split_once(';')?;
    let kind = kind.trim();
    if !kind.eq_ignore_ascii_case("rfc822") && !kind.eq_ignore_ascii_case("utf-8") {
        return None;
    }
    let address = address
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    let valid = !address.is_empty()
        && address.len() <= MAX_ADDRESS
        && address.contains('@')
        && !address.chars().any(|c| c.is_control() || c.is_whitespace());
    valid.then(|| nebula_store::report_address_key(address))
}

/// A `Message-ID` value without `<>`, if it looks like one.
pub(crate) fn message_id(value: &str) -> Option<String> {
    let id = value.trim().strip_prefix('<')?.split('>').next()?.trim();
    let valid = !id.is_empty()
        && id.len() <= MAX_ADDRESS
        && !id.chars().any(|c| c.is_control() || c.is_whitespace());
    valid.then(|| id.to_owned())
}

/// `smtp; 550 5.1.1 …` → `550 5.1.1 …` (only SMTP replies).
fn smtp_reply(value: &str) -> Option<String> {
    let (kind, reply) = value.split_once(';')?;
    if !kind.trim().eq_ignore_ascii_case("smtp") {
        return None;
    }
    let reply = clean(reply, MAX_REPLY_CHARS);
    let code = reply.get(..3)?;
    let valid = code.bytes().all(|b| b.is_ascii_digit())
        && matches!(code.as_bytes().first(), Some(b'2'..=b'5'))
        && reply
            .as_bytes()
            .get(3)
            .is_none_or(|b| *b == b' ' || *b == b'-');
    valid.then_some(reply)
}

fn action(value: &str) -> Option<ReportAction> {
    // `Action: failed (some comment)`
    let word = value.split([' ', '(', ';']).next()?.to_ascii_lowercase();
    Some(match word.as_str() {
        "failed" => ReportAction::Failed,
        "delayed" => ReportAction::Delayed,
        "delivered" => ReportAction::Delivered,
        "relayed" | "expanded" => ReportAction::Relayed,
        _ => return None,
    })
}

fn recipients_of(block: &Block) -> Vec<String> {
    ["original-recipient", "final-recipient"]
        .into_iter()
        .filter_map(|name| field(block, name).and_then(address))
        .collect()
}

/// What a report says: `(envid, original Message-ID, kind)`.
pub(crate) type Parsed = (Option<String>, Option<String>, ReportKind);

/// Reads the decoded machine-readable part. `None` if it says nothing we
/// can use.
pub(crate) fn parse(wanted: ReportType, status: &[u8]) -> Option<Parsed> {
    let text = String::from_utf8_lossy(status);
    let blocks = blocks(&text);
    match wanted {
        ReportType::Delivery => {
            let (first, rest) = blocks.split_first()?;
            let envid = field(first, "original-envelope-id")
                .map(|v| clean(v, 100))
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_graphic()));
            let recipients: Vec<RecipientReport> = rest
                .iter()
                .take(MAX_RECIPIENTS)
                .filter_map(|block| {
                    let addresses = recipients_of(block);
                    if addresses.is_empty() {
                        return None;
                    }
                    Some(RecipientReport {
                        addresses,
                        action: action(field(block, "action")?)?,
                        smtp_reply: field(block, "diagnostic-code").and_then(smtp_reply),
                    })
                })
                .collect();
            (!recipients.is_empty()).then_some((envid, None, ReportKind::Delivery(recipients)))
        }
        ReportType::Disposition => {
            let block = blocks.first()?;
            let disposition = field(block, "disposition")?;
            // `manual-action/MDN-sent-manually; displayed`
            let (_, kind) = disposition.split_once(';')?;
            let kind = kind
                .trim()
                .split(['/', ' ', ';'])
                .next()?
                .to_ascii_lowercase();
            if kind != "displayed" {
                return None;
            }
            let recipients = recipients_of(block);
            if recipients.is_empty() {
                return None;
            }
            let original = field(block, "original-message-id").and_then(message_id);
            Some((None, original, ReportKind::Displayed(recipients)))
        }
    }
}

/// The `Message-ID` of the returned message (its first bytes).
pub(crate) fn returned_message_id(returned: &[u8]) -> Option<String> {
    let parsed = MessageParser::default().parse_headers(returned)?;
    let id = parsed.message_id()?;
    message_id(&format!("<{id}>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DSN: &[u8] = b"Reporting-MTA: dns; mx.example.com\r\n\
Original-Envelope-Id: ENV-1\r\n\
Arrival-Date: Tue, 27 Oct 2026 10:00:00 +0000\r\n\r\n\
Original-Recipient: rfc822;Bob@Example.com\r\n\
Final-Recipient: rfc822; bob@example.com\r\n\
Action: failed\r\nStatus: 5.1.1\r\n\
Diagnostic-Code: smtp; 550 5.1.1 <bob@example.com>:\r\n  user unknown\r\n\r\n\
Final-Recipient: rfc822; carol@example.com\r\nAction: delayed\r\nStatus: 4.4.1\r\n\r\n\
Final-Recipient: x400; whatever\r\nAction: delivered\r\n\r\n\
Final-Recipient: rfc822; dave@example.com\r\nAction: exploded\r\n";

    #[test]
    fn reads_a_dsn() {
        let (envid, original, kind) = parse(ReportType::Delivery, DSN).unwrap();
        assert_eq!(envid.as_deref(), Some("ENV-1"));
        assert_eq!(original, None);
        let ReportKind::Delivery(list) = kind else {
            panic!("not a DSN")
        };
        assert_eq!(list.len(), 2, "x400 and unknown actions are skipped");
        assert_eq!(
            list[0].addresses,
            vec!["bob@example.com", "bob@example.com"]
        );
        assert_eq!(list[0].action, ReportAction::Failed);
        assert_eq!(
            list[0].smtp_reply.as_deref(),
            Some("550 5.1.1 <bob@example.com>: user unknown")
        );
        assert_eq!(list[1].action, ReportAction::Delayed);
        assert_eq!(list[1].smtp_reply, None);
    }

    #[test]
    fn hostile_fields_are_cleaned_or_dropped() {
        assert_eq!(smtp_reply("smtp; 550\u{7}bad").as_deref(), Some("550 bad"));
        assert_eq!(smtp_reply("smtp; 650 no"), None);
        assert_eq!(smtp_reply("x-unix; 550 no"), None);
        assert_eq!(
            smtp_reply("smtp; 250 ok\u{1b}[31m").as_deref(),
            Some("250 ok [31m")
        );
        assert_eq!(address("rfc822; bob @example.com"), None);
        assert_eq!(
            address("rfc822; <Bob@Example.com>").as_deref(),
            Some("bob@example.com")
        );
        assert_eq!(address("rfc822; noat"), None);
        // The same lowercase as the envelope side, not ASCII only.
        assert_eq!(
            address("rfc822; Ñandú@Ejemplo.es").as_deref(),
            Some(nebula_store::report_address_key("Ñandú@Ejemplo.es").as_str())
        );
        assert_eq!(
            address("rfc822; Ñandú@Ejemplo.es").as_deref(),
            Some("ñandú@ejemplo.es")
        );
        assert_eq!(message_id("<a@b> trailing").as_deref(), Some("a@b"));
        assert_eq!(message_id("a@b"), None);
        let many = (0..500)
            .map(|i| format!("Final-Recipient: rfc822; r{i}@x\r\nAction: failed\r\n\r\n"))
            .collect::<Vec<_>>()
            .concat();
        let ReportKind::Delivery(list) = parse(
            ReportType::Delivery,
            format!("Reporting-MTA: dns; x\r\n\r\n{many}").as_bytes(),
        )
        .unwrap()
        .2
        else {
            panic!("not a DSN")
        };
        assert_eq!(list.len(), MAX_RECIPIENTS);
        assert!(parse(ReportType::Delivery, b"").is_none());
        assert!(parse(ReportType::Delivery, b"Reporting-MTA: dns; x\r\n").is_none());
    }

    #[test]
    fn reads_an_mdn() {
        let mdn = b"Reporting-UA: x; y\r\nFinal-Recipient: rfc822; bob@example.com\r\n\
Original-Message-ID: <m1@example.com>\r\n\
Disposition: manual-action/MDN-sent-manually; displayed\r\n";
        let (envid, original, kind) = parse(ReportType::Disposition, mdn).unwrap();
        assert_eq!(envid, None);
        assert_eq!(original.as_deref(), Some("m1@example.com"));
        assert_eq!(
            kind,
            ReportKind::Displayed(vec!["bob@example.com".to_owned()])
        );
        let deleted = b"Final-Recipient: rfc822; bob@example.com\r\n\
Disposition: manual-action/MDN-sent-manually; deleted\r\n";
        assert!(parse(ReportType::Disposition, deleted).is_none());
    }

    #[test]
    fn report_headers() {
        assert_eq!(
            report_type(b"Content-Type: multipart/report; report-type=delivery-status;\r\n boundary=x\r\n\r\n"),
            Some(ReportType::Delivery)
        );
        assert_eq!(
            report_type(b"Content-Type: Multipart/Report; report-type=\"disposition-notification\"; boundary=x\r\n\r\n"),
            Some(ReportType::Disposition)
        );
        assert_eq!(
            report_type(b"Content-Type: multipart/mixed; boundary=x\r\n\r\n"),
            None
        );
        assert_eq!(
            report_type(b"Content-Type: multipart/report; report-type=other; boundary=x\r\n\r\n"),
            None
        );
        assert_eq!(
            returned_message_id(
                b"From: a@b\r\nMessage-ID: <orig@example.com>\r\nSubject: x\r\n\r\n"
            )
            .as_deref(),
            Some("orig@example.com")
        );
    }
}
