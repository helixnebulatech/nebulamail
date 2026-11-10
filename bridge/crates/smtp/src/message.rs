// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Preparing a raw RFC 5322 message for sending: pure functions, no I/O.

use std::fmt;

use crate::error::SmtpError;

/// A message ready for `DATA`: `Bcc` and `Resent-Bcc` removed and every line ending CRLF.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingMessage(Vec<u8>);

impl OutgoingMessage {
    /// Prepares a raw message: normalizes line endings to CRLF
    /// ([`normalize_line_endings`]) and then removes the `Bcc` and
    /// `Resent-Bcc` header fields ([`remove_bcc`]).
    ///
    /// # Errors
    ///
    /// [`SmtpError::EmptyMessage`] if `raw` is empty.
    pub fn new(raw: &[u8]) -> Result<Self, SmtpError> {
        if raw.is_empty() {
            return Err(SmtpError::EmptyMessage);
        }
        Ok(Self(remove_bcc(&normalize_line_endings(raw))))
    }

    /// The bytes to send.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The fields of the top-level header block, split exactly as
    /// [`remove_bcc`] splits them (the same lines, the same end of the
    /// block), so a check on them sees what is sent.
    #[must_use]
    pub fn header_fields(&self) -> Vec<HeaderField<'_>> {
        header_fields(&self.0)
    }

    /// Whether the message has bytes above 127, which need 8BITMIME.
    #[must_use]
    pub fn is_8bit(&self) -> bool {
        !self.0.is_ascii()
    }

    /// Whether the top-level header block has bytes above 127 (UTF-8
    /// header fields, RFC 6532): then the message needs SMTPUTF8
    /// (RFC 6531 §3.4), not just 8BITMIME.
    #[must_use]
    pub fn has_8bit_headers(&self) -> bool {
        !header_block(&self.0).is_ascii()
    }

    /// Whether some line is longer than RFC 5322 §2.1.1 allows (998
    /// octets before CRLF; RFC 5321 §4.5.3.1.6) or there is a NUL, which
    /// 8BITMIME doesn't allow either (RFC 6152 §3): such a message can't
    /// go as it is.
    #[must_use]
    pub fn breaks_line_limits(&self) -> bool {
        self.0.contains(&0)
            || self
                .0
                .split(|&b| b == b'\n')
                .any(|line| line.strip_suffix(b"\r").unwrap_or(line).len() > MAX_LINE_OCTETS)
    }

    /// The bytes for lettre, which adds `CRLF . CRLF` itself: a final CRLF
    /// here would add an empty line at the end of the message.
    pub(crate) fn data(&self) -> &[u8] {
        self.0.strip_suffix(b"\r\n").unwrap_or(&self.0)
    }
}

impl fmt::Debug for OutgoingMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print mail content.
        f.debug_struct("OutgoingMessage")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Turns every bare LF and bare CR into CRLF.
///
/// SMTP lines end in CRLF (RFC 5321 §2.3.8). Bare LF also opens the door to
/// SMTP smuggling: `LF . CRLF` is not dot-stuffed by lettre but some servers
/// take it as the end of the message, and whatever follows as new commands.
#[must_use]
pub fn normalize_line_endings(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len().saturating_add(raw.len() / 32));
    let mut bytes = raw.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\r' => {
                bytes.next_if_eq(&b'\n');
                out.extend_from_slice(b"\r\n");
            }
            b'\n' => out.extend_from_slice(b"\r\n"),
            other => out.push(other),
        }
    }
    out
}

/// Removes every `Bcc` and `Resent-Bcc` header field (RFC 5322 §3.6.3 and
/// §3.6.6) from the top-level header block: both list blind recipients.
///
/// - Matches the field name without regard to case (`Bcc`, `BCC`,
///   `Resent-BCC`, and the obsolete `Bcc :` with spaces before the colon),
///   and only those names: `X-Bcc` stays.
/// - Removes the folded continuation lines of the field too.
/// - The header block ends at the first empty line; the body, and so every
///   nested part and attached message, is never touched.
/// - Everything else is returned byte for byte: no re-encoding. Works with
///   CRLF and with bare LF line endings.
#[must_use]
pub fn remove_bcc(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut in_bcc = false;
    let mut rest = raw;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        if is_blank(line) {
            // End of the header block: copy the separator and the body.
            out.extend_from_slice(rest);
            return out;
        }
        let is_continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if !is_continuation {
            in_bcc = is_blind_field(line);
        }
        if !in_bcc {
            out.extend_from_slice(line);
        }
        rest = after;
    }
    out
}

/// The longest line RFC 5322 §2.1.1 allows, without its CRLF.
const MAX_LINE_OCTETS: usize = 998;

/// The top-level header block, as [`remove_bcc`] finds it: up to the first
/// blank line (or all of `raw` without one).
fn header_block(raw: &[u8]) -> &[u8] {
    let mut offset = 0_usize;
    let mut rest = raw;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        if is_blank(line) {
            break;
        }
        offset = offset.saturating_add(line.len());
        rest = after;
    }
    raw.get(..offset).unwrap_or(raw)
}

/// One field of a header block ([`OutgoingMessage::header_fields`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeaderField<'a> {
    /// `name: value`. The name is cut like [`remove_bcc`] does (without
    /// the spaces the obsolete syntax allows before the colon); the value
    /// is everything after the colon, folded lines and line endings
    /// included.
    Field {
        /// The field name, as written.
        name: &'a [u8],
        /// The raw value.
        value: &'a [u8],
    },
    /// A line that is not a field: no colon, or a continuation line with
    /// no field before it. Servers read such lines in different ways, so
    /// a message with one can't be checked.
    Malformed,
}

/// Pushes the field being read, which ends at `end`.
fn finish<'a>(
    raw: &'a [u8],
    current: Option<(&'a [u8], usize)>,
    end: usize,
    out: &mut Vec<HeaderField<'a>>,
) {
    if let Some((name, start)) = current {
        out.push(HeaderField::Field {
            name,
            value: raw.get(start..end).unwrap_or_default(),
        });
    }
}

fn header_fields(raw: &[u8]) -> Vec<HeaderField<'_>> {
    let mut out = Vec::new();
    // The field being read: its name and where its value starts.
    let mut current: Option<(&[u8], usize)> = None;
    let mut offset = 0_usize;
    let mut rest = raw;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        if is_blank(line) {
            break;
        }
        let is_continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if is_continuation {
            if current.is_none() {
                out.push(HeaderField::Malformed);
            }
        } else {
            finish(raw, current.take(), offset, &mut out);
            match field_name(line) {
                Some((name, colon)) => {
                    current = Some((name, offset.saturating_add(colon).saturating_add(1)));
                }
                None => out.push(HeaderField::Malformed),
            }
        }
        offset = offset.saturating_add(line.len());
        rest = after;
    }
    finish(raw, current, offset, &mut out);
    out
}

/// The name of a header line (cut as in [`is_blind_field`]) and where its
/// colon is.
fn field_name(line: &[u8]) -> Option<(&[u8], usize)> {
    let colon = line.iter().position(|&b| b == b':')?;
    let name = line.get(..colon).unwrap_or_default();
    // RFC 5322 §4.5.3 (obsolete syntax): spaces may come before the colon.
    Some((name.trim_ascii_end(), colon))
}

/// Splits off the first line, terminator included.
fn split_line(bytes: &[u8]) -> (&[u8], &[u8]) {
    match bytes.iter().position(|&b| b == b'\n') {
        Some(index) => bytes.split_at(index.saturating_add(1)),
        None => (bytes, &[]),
    }
}

/// Whether a line (with its terminator) is empty: the end of the headers.
fn is_blank(line: &[u8]) -> bool {
    matches!(line, b"\n" | b"\r\n")
}

/// Whether a header line starts a `Bcc` or `Resent-Bcc` field.
fn is_blind_field(line: &[u8]) -> bool {
    field_name(line).is_some_and(|(name, _)| {
        name.eq_ignore_ascii_case(b"bcc") || name.eq_ignore_ascii_case(b"resent-bcc")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn bcc_free(raw: &str) -> String {
        String::from_utf8(remove_bcc(raw.as_bytes())).unwrap()
    }

    #[test]
    fn utf8_header_fields_are_told_from_an_8bit_body() {
        let headers = OutgoingMessage::new("Subject: Ñandú\r\n\r\nhola\r\n".as_bytes()).unwrap();
        assert!(headers.has_8bit_headers());
        let body = OutgoingMessage::new("Subject: hola\r\n\r\nÑandú\r\n".as_bytes()).unwrap();
        assert!(!body.has_8bit_headers());
        assert!(body.is_8bit());
        // Without a body, it is all header.
        let only = OutgoingMessage::new("Subject: Ñ".as_bytes()).unwrap();
        assert!(only.has_8bit_headers());
    }

    #[test]
    fn line_limits_of_rfc_5322() {
        let ok = format!("Subject: a\r\n\r\n{}\r\n", "x".repeat(998));
        assert!(
            !OutgoingMessage::new(ok.as_bytes())
                .unwrap()
                .breaks_line_limits()
        );
        let long = format!("Subject: a\r\n\r\n{}\r\n", "x".repeat(999));
        assert!(
            OutgoingMessage::new(long.as_bytes())
                .unwrap()
                .breaks_line_limits()
        );
        assert!(
            OutgoingMessage::new(b"Subject: a\r\n\r\na\0b\r\n")
                .unwrap()
                .breaks_line_limits()
        );
    }

    #[test]
    fn header_fields_split_like_remove_bcc() {
        let message = OutgoingMessage::new(
            b"From : a@x\r\nfRoM: b@x,\r\n c@x\r\nBcc: z@x\r\nX: \r\n\r\nFrom: body",
        )
        .unwrap();
        assert_eq!(
            message.header_fields(),
            [
                HeaderField::Field {
                    name: b"From",
                    value: b" a@x\r\n"
                },
                HeaderField::Field {
                    name: b"fRoM",
                    value: b" b@x,\r\n c@x\r\n"
                },
                HeaderField::Field {
                    name: b"X",
                    value: b" \r\n"
                },
            ]
        );
        let message = OutgoingMessage::new(b"From: a@x\r\nbasura\r\n\r\nbody").unwrap();
        assert_eq!(
            message.header_fields().last(),
            Some(&HeaderField::Malformed)
        );
        let message = OutgoingMessage::new(b" folded\r\nFrom: a@x\r\n\r\n").unwrap();
        assert_eq!(
            message.header_fields().first(),
            Some(&HeaderField::Malformed)
        );
        // A bare CR is a line end once normalized: it starts a new field.
        let message = OutgoingMessage::new(b"From: a@x\rFrom: b@x\r\n\r\n").unwrap();
        assert_eq!(message.header_fields().len(), 2);
        // No blank line: everything is headers.
        let message = OutgoingMessage::new(b"From: a@x").unwrap();
        assert_eq!(
            message.header_fields(),
            [HeaderField::Field {
                name: b"From",
                value: b" a@x"
            }]
        );
    }

    #[test]
    fn removes_a_simple_bcc() {
        let raw = "From: a@x\r\nTo: b@x\r\nBcc: c@x\r\nSubject: hi\r\n\r\nbody\r\n";
        assert_eq!(
            bcc_free(raw),
            "From: a@x\r\nTo: b@x\r\nSubject: hi\r\n\r\nbody\r\n"
        );
    }

    #[test]
    fn any_case_and_space_before_colon() {
        for name in ["Bcc", "BCC", "bcc", "bCc", "Bcc ", "Bcc\t"] {
            let raw = format!("To: b@x\r\n{name}: c@x\r\n\r\nbody");
            assert_eq!(bcc_free(&raw), "To: b@x\r\n\r\nbody", "{name:?}");
        }
    }

    #[test]
    fn removes_folded_lines() {
        let raw = "To: b@x\r\nBcc: c@x,\r\n  d@x,\r\n\te@x\r\nSubject: s\r\n  folded\r\n\r\nbody";
        assert_eq!(
            bcc_free(raw),
            "To: b@x\r\nSubject: s\r\n  folded\r\n\r\nbody"
        );
    }

    #[test]
    fn removes_every_bcc_field() {
        let raw = "Bcc: one@x\r\nTo: b@x\r\nbcc: two@x\r\n three@x\r\nBCC:\r\n\r\nbody";
        assert_eq!(bcc_free(raw), "To: b@x\r\n\r\nbody");
    }

    #[test]
    fn keeps_similar_names() {
        let raw = "X-Bcc: keep\r\nResent-Bccx: keep\r\nBccx: keep\r\nB cc: keep\r\n\r\n";
        assert_eq!(bcc_free(raw), raw);
    }

    #[test]
    fn removes_resent_bcc_with_folding() {
        let raw =
            "Resent-From: a@x\r\nResent-BCC : c@x,\r\n d@x\r\nTo: b@x\r\n\r\nResent-Bcc: body\r\n";
        assert_eq!(
            bcc_free(raw),
            "Resent-From: a@x\r\nTo: b@x\r\n\r\nResent-Bcc: body\r\n"
        );
        assert_eq!(
            bcc_free("resent-bcc: c@x\r\nBcc: d@x\r\nX-Resent-Bcc: keep\r\n\r\n"),
            "X-Resent-Bcc: keep\r\n\r\n"
        );
    }

    #[test]
    fn keeps_bcc_in_the_body_and_in_nested_parts() {
        let raw = concat!(
            "Content-Type: multipart/mixed; boundary=b\r\n",
            "Bcc: secret@x\r\n",
            "\r\n",
            "Bcc: this is body text\r\n",
            "--b\r\n",
            "Content-Type: message/rfc822\r\n",
            "\r\n",
            "Bcc: inner@x\r\n",
            "Subject: attached\r\n",
            "\r\n",
            "inner body\r\n",
            "--b--\r\n",
        );
        let expected = raw.replace("Bcc: secret@x\r\n", "");
        assert_eq!(bcc_free(raw), expected);
    }

    #[test]
    fn works_with_bare_lf() {
        let raw = "To: b@x\nBcc: c@x\n d@x\nSubject: s\n\nBcc: body\n";
        assert_eq!(bcc_free(raw), "To: b@x\nSubject: s\n\nBcc: body\n");
    }

    #[test]
    fn headers_only_and_last_line_without_terminator() {
        assert_eq!(bcc_free("To: b@x\r\nBcc: c@x"), "To: b@x\r\n");
        assert_eq!(bcc_free("Bcc: c@x\r\n"), "");
        assert_eq!(bcc_free(""), "");
    }

    #[test]
    fn non_utf8_bytes_are_kept() {
        let raw = b"Subject: \xff\xfe\r\nBcc: c@x\r\n\r\n\x80body";
        assert_eq!(remove_bcc(raw), b"Subject: \xff\xfe\r\n\r\n\x80body");
    }

    #[test]
    fn line_endings() {
        assert_eq!(normalize_line_endings(b"a\nb\rc\r\nd"), b"a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_line_endings(b"\r\r\n\n"), b"\r\n\r\n\r\n");
        assert_eq!(normalize_line_endings(b"x\n.\r\ny"), b"x\r\n.\r\ny");
    }

    #[test]
    fn outgoing_message() {
        let message = OutgoingMessage::new(b"To: b@x\nBcc: c@x\n\nhi\n").unwrap();
        assert_eq!(message.as_bytes(), b"To: b@x\r\n\r\nhi\r\n");
        assert_eq!(message.data(), b"To: b@x\r\n\r\nhi");
        assert!(!message.is_8bit());
        assert_eq!(OutgoingMessage::new(b""), Err(SmtpError::EmptyMessage));
        assert_eq!(format!("{message:?}"), "OutgoingMessage { len: 15, .. }");
    }

    proptest! {
        #[test]
        fn output_is_input_minus_whole_lines(raw in proptest::collection::vec(any::<u8>(), 0..400)) {
            let out = remove_bcc(&raw);
            prop_assert!(out.len() <= raw.len());
            // The output is the input with some lines left out, in order.
            let mut lines = raw.split_inclusive(|&b| b == b'\n');
            for kept in out.split_inclusive(|&b| b == b'\n') {
                prop_assert!(lines.any(|line| line == kept));
            }
        }

        #[test]
        fn no_bcc_field_survives_in_the_headers(
            headers in proptest::collection::vec(
                prop_oneof![
                    Just("Bcc: x@y".to_owned()),
                    Just("bCC : x@y".to_owned()),
                    Just("Resent-Bcc: x@y".to_owned()),
                    Just("RESENT-BCC : x@y".to_owned()),
                    Just(" folded".to_owned()),
                    "[A-Za-z-]{1,8}: [ -~]{0,10}",
                ],
                0..12,
            ),
            crlf in any::<bool>(),
        ) {
            let eol = if crlf { "\r\n" } else { "\n" };
            let mut lines = vec!["To: a@b".to_owned()];
            lines.extend(headers);
            let raw = format!("{}{eol}{eol}Bcc: body", lines.join(eol));
            let out = String::from_utf8(remove_bcc(raw.as_bytes())).unwrap();
            let (head, body) = out.split_once(&format!("{eol}{eol}")).unwrap_or((out.as_str(), ""));
            // An oracle of its own, not `is_blind_field`: the field name,
            // lowercased and without the spaces before the colon.
            for line in head.split(eol) {
                let name = line
                    .split_once(':')
                    .map(|(name, _)| name.trim_end().to_ascii_lowercase());
                prop_assert!(
                    !matches!(name.as_deref(), Some("bcc" | "resent-bcc")),
                    "{line:?} in {out:?}"
                );
            }
            prop_assert_eq!(body, "Bcc: body");
        }

        #[test]
        fn normalized_text_has_only_crlf(raw in proptest::collection::vec(any::<u8>(), 0..200)) {
            let out = normalize_line_endings(&raw);
            for (index, &byte) in out.iter().enumerate() {
                if byte == b'\n' {
                    prop_assert_eq!(out.get(index.wrapping_sub(1)), Some(&b'\r'));
                }
                if byte == b'\r' {
                    prop_assert_eq!(out.get(index + 1), Some(&b'\n'));
                }
            }
        }
    }
}
