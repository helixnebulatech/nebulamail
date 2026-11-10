// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Encapsulated messages and MIME boundaries: pure
//! functions, no I/O.
//!
//! - A forwarded `message/rfc822` goes as it is (RFC 2046 §5.2.1 allows
//!   only 7bit, 8bit or binary): [`classify`] says which, on the bytes with
//!   CRLF line ends ([`canonical`]). What can't be 7bit or 8bit (a NUL, a
//!   line over [`MAX_LINE`] octets) goes as `message/global` in base64
//!   (RFC 6532 §3.7 allows any encoding), and so does a message with 8-bit
//!   header fields (that is a `message/global` by definition).
//! - Boundaries are 128 random bits in hex ([`boundary`]): mail-builder's
//!   are predictable (one field is a per-thread seed written in clear in
//!   every message). A part written as it is could still declare a
//!   boundary that is a prefix of ours (`"0"`), and parsers that match
//!   delimiters by prefix while inside it would end it at our line: so
//!   every such part is scanned for `--` lines and `boundary` values
//!   ([`Delimiters`]; a `boundary` parameter in any form but the plain
//!   `boundary=value` makes it go encoded, since RFC 2231 and comments
//!   hide the value from the scan) and the boundaries are chosen not to clash with them
//!   ([`plan_boundaries`]); a part that clashes with every try goes
//!   encoded instead.
//! - [`downgrade_7bit`] makes a message 7-bit for an SMTP server without
//!   8BITMIME: each 8-bit `message/rfc822` or `message/global` becomes a
//!   `message/global` in base64.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

use base64::Engine as _;
use nebula_jmap_proto::{SetError, SetErrorType};
use nebula_mail::ParsedMessage;

/// Longest line (without its CRLF) that 7bit and 8bit allow (RFC 5322
/// §2.1.1, RFC 2045 §2.7–§2.8).
pub(crate) const MAX_LINE: usize = 998;

/// How an encapsulated message can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encapsulation {
    /// `message/rfc822`, `7bit`: as it is.
    SevenBit,
    /// `message/rfc822`, `8bit`: as it is.
    EightBit,
    /// `message/global`, `8bit`: as it is (8-bit header fields, RFC 6532).
    GlobalEightBit,
    /// `message/global` in base64, with the original bytes.
    Global,
}

impl Encapsulation {
    /// Whether the bytes are written as they are (not encoded).
    pub(crate) fn is_raw(self) -> bool {
        !matches!(self, Self::Global)
    }
}

/// How `bytes` (with CRLF line ends, see [`canonical`]) can go.
pub(crate) fn classify(bytes: &[u8]) -> Encapsulation {
    let unencodable = bytes.contains(&0)
        || bytes
            .split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .any(|line| line.len() > MAX_LINE);
    if unencodable {
        return Encapsulation::Global;
    }
    if bytes.is_ascii() {
        return Encapsulation::SevenBit;
    }
    if header_block(bytes).is_ascii() {
        Encapsulation::EightBit
    } else {
        Encapsulation::GlobalEightBit
    }
}

/// The header block of a message: up to the first empty line (all of it
/// if there is none).
fn header_block(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(b"\r\n") {
        return &[];
    }
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .and_then(|at| bytes.get(..at))
        .unwrap_or(bytes)
}

/// `bytes` with every line ending CRLF; borrowed when they already do.
pub(crate) fn canonical(bytes: &[u8]) -> Cow<'_, [u8]> {
    let mut previous = 0_u8;
    let mut bare = false;
    for (at, byte) in bytes.iter().enumerate() {
        let next = bytes.get(at.saturating_add(1)).copied();
        if (*byte == b'\n' && previous != b'\r') || (*byte == b'\r' && next != Some(b'\n')) {
            bare = true;
            break;
        }
        previous = *byte;
    }
    if bare {
        Cow::Owned(nebula_smtp::normalize_line_endings(bytes))
    } else {
        Cow::Borrowed(bytes)
    }
}

/// A new boundary: 16 random bytes in hex, no fixed prefix (a fixed one
/// could be declared by a forwarded message, see the module docs).
pub(crate) fn boundary() -> Result<String, SetError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let mut out = String::with_capacity(32);
    for byte in bytes {
        // Writing to a String can't fail.
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

/// More different delimiters than this in one part: it goes encoded
/// rather than be compared with each.
const MAX_DELIMITERS: usize = 4096;

/// What a part written as it is could take for a delimiter: the rest of
/// each line that starts with `--` (without a final `--` and blanks), and
/// every `boundary` value it declares, in lower case. `None` when there
/// are too many, or when a `boundary` parameter is written in a form the
/// scanner doesn't read (see [`boundary_values`]): that part goes encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delimiters(Option<HashSet<Vec<u8>>>);

impl Delimiters {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        let Some(declared) = boundary_values(bytes) else {
            return Self(None);
        };
        let mut found = HashSet::new();
        let mut add = |text: &[u8]| {
            // A boundary has at least one character (RFC 2046 §5.1.1):
            // an empty one can't be declared.
            if !text.is_empty() {
                found.insert(text.to_ascii_lowercase());
            }
        };
        for line in bytes.split(|byte| *byte == b'\n') {
            if let Some(rest) = line.strip_prefix(b"--") {
                let rest = rest.trim_ascii_end();
                let rest = rest.strip_suffix(b"--").unwrap_or(rest).trim_ascii_end();
                add(rest);
            }
        }
        for value in declared {
            add(value);
        }
        // A second layer: what mail-parser itself decodes (RFC 2231,
        // comments, quoted pairs) in every part of every nested message.
        if let Some(parsed) = ParsedMessage::parse(bytes) {
            for part in parsed.message().parts() {
                if let Some(content_type) = part.content_type() {
                    for (name, value) in content_type.attributes() {
                        if name.eq_ignore_ascii_case("boundary") {
                            add(value.as_bytes());
                            add(value.trim().as_bytes());
                        }
                    }
                }
            }
        }
        if found.len() > MAX_DELIMITERS {
            Self(None)
        } else {
            Self(Some(found))
        }
    }

    /// Whether `boundary` could be mistaken for one of these (or one of
    /// these for it), by a parser that matches by prefix.
    pub(crate) fn clash(&self, boundary: &str) -> bool {
        let Some(found) = &self.0 else {
            return true;
        };
        let boundary = boundary.as_bytes();
        found
            .iter()
            .any(|text| boundary.starts_with(text) || text.starts_with(boundary))
    }
}

/// Longest run of blanks and comments [`skip_cfws`] goes through: past
/// it, the `boundary` is taken as unreadable (and the scan stays linear).
const MAX_CFWS: usize = 256;

/// Blanks (folding included) and comments from `at` on: where the next
/// thing starts. `None` for an unclosed comment or a run over
/// [`MAX_CFWS`].
fn skip_cfws(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut at = start;
    while let Some(byte) = bytes.get(at) {
        if at.saturating_sub(start) > MAX_CFWS {
            return None;
        }
        match byte {
            b'(' => depth = depth.saturating_add(1),
            b')' if depth > 0 => depth = depth.saturating_sub(1),
            b'\\' if depth > 0 => at = at.saturating_add(1),
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ if depth > 0 => {}
            _ => return Some(at),
        }
        at = at.saturating_add(1);
    }
    (depth == 0).then_some(at)
}

/// Whether a byte can be in an unquoted parameter value (an RFC 2045
/// token).
fn is_token(byte: u8) -> bool {
    byte.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?=".contains(&byte)
}

/// The values of every `boundary` parameter (any case) in `bytes`, only
/// in the plain forms `boundary=token` and `boundary="text"`. `None` when
/// one is written any other way: blanks or a comment around `=`, an RFC
/// 2231 name (`boundary*=`, `boundary*0=`, `boundary*0*=`), a value that
/// starts with a blank or a comment, a quoted value with `\\` or without
/// its closing quote, an empty value, or a token followed by something
/// that isn't a blank, `;` or a comment. Parsers decode those to values
/// this scanner can't see (`boundary*=utf-8''%30` is `0`), so the part
/// must go encoded. A `boundary` not followed by `=` or `*` is just a
/// word.
fn boundary_values(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    const KEY: &[u8] = b"boundary";
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(rest) = bytes.get(at..) {
        let Some(found) = rest
            .windows(KEY.len())
            .position(|window| window.eq_ignore_ascii_case(KEY))
        else {
            break;
        };
        let name_end = at.saturating_add(found).saturating_add(KEY.len());
        at = name_end;
        let next = skip_cfws(bytes, name_end)?;
        match bytes.get(next) {
            // `boundary=`, right after the name: the plain form.
            Some(b'=') if next == name_end => {}
            // RFC 2231 (`boundary*`), or CFWS before `=`.
            Some(b'*' | b'=') => return None,
            _ => continue,
        }
        let start = name_end.saturating_add(1);
        let value = bytes.get(start..).unwrap_or_default();
        let value = if let Some(quoted) = value.strip_prefix(b"\"") {
            let end = quoted.iter().position(|b| matches!(b, b'"' | b'\\'))?;
            if quoted.get(end) != Some(&b'"') {
                return None;
            }
            quoted.get(..end).unwrap_or_default()
        } else {
            let end = value
                .iter()
                .position(|b| !is_token(*b))
                .unwrap_or(value.len());
            if !matches!(
                value.get(end),
                None | Some(b';' | b' ' | b'\t' | b'\r' | b'\n' | b'(')
            ) {
                return None;
            }
            value.get(..end).unwrap_or_default()
        };
        if value.is_empty() {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

/// Tries per boundary before the parts it clashes with go encoded.
const TRIES: usize = 8;

/// The boundaries of a message and the parts that must go encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundaryPlan {
    pub(crate) boundaries: Vec<String>,
    /// By position in the parts given: `true` for those that must be
    /// encoded (none of the tries avoided them).
    pub(crate) encoded: Vec<bool>,
}

/// `count` boundaries (from `next`) that clash with none of `parts`
/// (written as they are) nor with each other. A part that clashes with
/// every try is marked to go encoded; then the try is taken.
pub(crate) fn plan_boundaries(
    count: usize,
    parts: &[Delimiters],
    mut next: impl FnMut() -> Result<String, SetError>,
) -> Result<BoundaryPlan, SetError> {
    let mut encoded: Vec<bool> = parts.iter().map(|part| part.0.is_none()).collect();
    let mut boundaries: Vec<String> = Vec::with_capacity(count);
    for _ in 0..count {
        let mut tries = 0;
        loop {
            let candidate = next()?;
            let taken = boundaries.iter().any(|other| {
                other.starts_with(candidate.as_str()) || candidate.starts_with(other.as_str())
            });
            if taken {
                continue;
            }
            let clashes: Vec<usize> = parts
                .iter()
                .enumerate()
                .filter(|(at, part)| {
                    !encoded.get(*at).copied().unwrap_or(true) && part.clash(&candidate)
                })
                .map(|(at, _)| at)
                .collect();
            tries += 1;
            if clashes.is_empty() || tries >= TRIES {
                for at in clashes {
                    if let Some(flag) = encoded.get_mut(at) {
                        *flag = true;
                    }
                }
                boundaries.push(candidate);
                break;
            }
        }
    }
    Ok(BoundaryPlan {
        boundaries,
        encoded,
    })
}

/// `bytes` in base64, lines of 76 characters separated by CRLF (no CRLF
/// at the end: the one before the next delimiter belongs to it).
pub(crate) fn base64_lines(bytes: &[u8]) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = Vec::with_capacity(encoded.len().saturating_add(encoded.len() / 38));
    for (at, line) in encoded.as_bytes().chunks(76).enumerate() {
        if at > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(line);
    }
    out
}

/// The `Content-Transfer-Encoding` of a part, lower case and trimmed.
fn transfer_encoding(part: &mail_parser::MessagePart<'_>) -> Option<String> {
    part.content_transfer_encoding()
        .map(|value| value.trim().to_ascii_lowercase())
}

/// `raw_value` of a Content-Type field with its type replaced by
/// `message/global` (the parameters kept as written).
fn global_content_type(raw_value: &[u8]) -> Vec<u8> {
    let start = raw_value
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t'))
        .unwrap_or(raw_value.len());
    let rest = raw_value.get(start..).unwrap_or_default();
    let end = rest
        .iter()
        .position(|b| matches!(b, b';' | b' ' | b'\t' | b'\r' | b'\n' | b'('))
        .unwrap_or(rest.len());
    let mut out = b" message/global".to_vec();
    out.extend_from_slice(rest.get(end..).unwrap_or_default());
    if !out.ends_with(b"\n") {
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// The longest line RFC 5322 §2.1.1 allows, without its CRLF.
const MAX_LINE_OCTETS: usize = 998;

/// Whether `bytes` can go over SMTP as they are: no NUL and no line over
/// 998 octets (RFC 5322 §2.1.1, RFC 5321 §4.5.3.1.6; RFC 6152 §3 keeps
/// both rules for 8BITMIME), and only ASCII unless 8 bits are allowed.
fn transferable_as_is(bytes: &[u8], allow_8bit: bool) -> bool {
    (allow_8bit || bytes.is_ascii())
        && !bytes.contains(&0)
        && bytes
            .split(|&b| b == b'\n')
            .all(|line| line.strip_suffix(b"\r").unwrap_or(line).len() <= MAX_LINE_OCTETS)
}

/// The message 7-bit for a server without 8BITMIME ([`transferable`]
/// with no 8 bits allowed).
#[cfg(test)]
pub(crate) fn downgrade_7bit(raw: &[u8]) -> Option<Vec<u8>> {
    transferable(raw, false)
}

/// The message as SMTP can carry it: every leaf part of the root message
/// whose content can't go as it is (a NUL, a line over 998 octets, or 8
/// bits when `allow_8bit` is false, or `binary`) gets a transfer encoding
/// that can (RFC 2045 §6, RFC 6152 §3): a `message/rfc822` (or
/// `message/global`) becomes a `message/global` in base64 (RFC 2046
/// §5.2.1 forbids base64 on message/rfc822; RFC 6532 §3.5 allows it on
/// message/global), any other part keeps its type and goes in base64.
/// Everything else stays byte for byte. `None` if the message can't be
/// parsed or the result still can't go (a header field over 998 octets,
/// or 8-bit header fields without 8 bits allowed, which need SMTPUTF8 and
/// are left to the sender).
pub(crate) fn transferable(raw: &[u8], allow_8bit: bool) -> Option<Vec<u8>> {
    let parsed = ParsedMessage::parse(raw)?;
    let mut out = Vec::with_capacity(raw.len().saturating_add(raw.len() / 2));
    let mut cursor = 0_usize;
    for (index, part) in parsed.parts() {
        if part.is_multipart() {
            continue;
        }
        let header = usize::try_from(part.offset_header()).ok()?;
        let body = usize::try_from(part.offset_body()).ok()?;
        let end = usize::try_from(part.offset_end()).ok()?;
        if header < cursor {
            // Inside a part already re-encoded.
            continue;
        }
        let content = raw.get(body..end)?;
        let encoding = transfer_encoding(&part);
        let binary = encoding.as_deref() == Some("binary");
        let identity = matches!(encoding.as_deref(), None | Some("8bit" | "7bit" | "binary"));
        if !(binary || (identity && !transferable_as_is(content, allow_8bit))) {
            continue;
        }
        if body < header || end < body {
            return None;
        }
        let media = nebula_mail::media_type(&part);
        let message = media == "message/rfc822" || media == "message/global";
        out.extend_from_slice(raw.get(cursor..header)?);
        let mut has_type = false;
        let mut has_mime_version = false;
        for field in part.headers() {
            let name = field.raw_name();
            let field_start = usize::try_from(field.offset_field()).ok()?;
            let field_end = usize::try_from(field.offset_end()).ok()?;
            // A header block cut short by a delimiter: not rewritten.
            if field_end > body {
                return None;
            }
            if name.eq_ignore_ascii_case("content-transfer-encoding") {
                continue;
            }
            has_mime_version |= name.eq_ignore_ascii_case("mime-version");
            if message && name.eq_ignore_ascii_case("content-type") && !has_type {
                has_type = true;
                out.extend_from_slice(name.as_bytes());
                out.push(b':');
                out.extend_from_slice(&global_content_type(field.raw_value()));
                continue;
            }
            out.extend_from_slice(raw.get(field_start..field_end)?);
        }
        if message && !has_type {
            out.extend_from_slice(b"Content-Type: message/global\r\n");
        }
        // The root of a single-part message: a transfer encoding needs
        // MIME (RFC 2045 §4).
        if index == 0 && !has_mime_version {
            out.extend_from_slice(b"MIME-Version: 1.0\r\n");
        }
        out.extend_from_slice(b"Content-Transfer-Encoding: base64\r\n\r\n");
        out.extend_from_slice(&base64_lines(content));
        cursor = end;
    }
    if cursor == 0 {
        return None;
    }
    out.extend_from_slice(raw.get(cursor..)?);
    transferable_as_is(&out, allow_8bit).then_some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use super::*;

    /// The decoded content of the first part with this transfer encoding.
    fn decoded_base64_body(raw: &[u8]) -> Vec<u8> {
        let text = std::str::from_utf8(raw).unwrap();
        let (_, rest) = text
            .split_once("Content-Transfer-Encoding: base64\r\n\r\n")
            .unwrap();
        let encoded = rest.split("\r\n--").next().unwrap_or(rest);
        base64::engine::general_purpose::STANDARD
            .decode(encoded.replace("\r\n", "").trim())
            .unwrap()
    }

    #[test]
    fn long_lines_are_re_encoded_even_with_8bitmime() {
        let long = "x".repeat(5000);
        let raw = format!(
            "Subject: a\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
             --B\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
             <p>{long}ñ</p>\r\n--B\r\nContent-Type: text/plain\r\n\r\ncorto\r\n--B--\r\n"
        );
        assert!(!transferable_as_is(raw.as_bytes(), true));
        let out = transferable(raw.as_bytes(), true).unwrap();
        assert!(transferable_as_is(&out, true));
        // Only the long part changed; its bytes are the same inside.
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains(
            "Content-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"
        ));
        assert!(text.contains("Content-Type: text/plain\r\n\r\ncorto\r\n"));
        assert_eq!(
            decoded_base64_body(&out),
            format!("<p>{long}ñ</p>").into_bytes()
        );
    }

    #[test]
    fn a_single_part_message_gets_mime_with_its_encoding() {
        let raw = format!("Subject: a\r\n\r\n{}\r\n", "y".repeat(1200));
        let out = transferable(raw.as_bytes(), true).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.starts_with(
                "Subject: a\r\nMIME-Version: 1.0\r\nContent-Transfer-Encoding: base64\r\n\r\n"
            ),
            "{text}"
        );
        // The body is all of it, with its last CRLF.
        let decoded = decoded_base64_body(&out);
        assert_eq!(decoded.len(), 1202);
        assert!(decoded.starts_with(&[b'y'; 1200]) && decoded.ends_with(b"\r\n"));
    }

    #[test]
    fn a_nul_is_re_encoded_and_8_bits_too_without_8bitmime() {
        let out = transferable(b"Subject: a\r\n\r\na\0b\r\n", true).unwrap();
        assert!(transferable_as_is(&out, true));
        let out = transferable("Subject: a\r\n\r\nñandú\r\n".as_bytes(), false).unwrap();
        assert!(out.is_ascii());
        assert_eq!(decoded_base64_body(&out), "ñandú\r\n".as_bytes());
    }

    #[test]
    fn what_no_encoding_fixes_is_refused() {
        // A header line over 998 octets.
        let raw = format!("Subject: {}\r\n\r\nhola\r\n", "z".repeat(1200));
        assert!(transferable(raw.as_bytes(), true).is_none());
        // UTF-8 header fields without 8 bits (they need SMTPUTF8).
        assert!(transferable("Subject: Ñandú\r\n\r\nhola\r\n".as_bytes(), false).is_none());
        // Nothing to change: no copy.
        assert!(transferable(b"Subject: a\r\n\r\nhola\r\n", true).is_none());
    }

    #[test]
    fn classes() {
        assert_eq!(
            classify(b"Subject: Hola\r\n\r\nHola\r\n"),
            Encapsulation::SevenBit
        );
        assert_eq!(
            classify("Subject: Hola\r\n\r\nÑandú\r\n".as_bytes()),
            Encapsulation::EightBit
        );
        assert_eq!(
            classify("Subject: Ñandú\r\n\r\nHola\r\n".as_bytes()),
            Encapsulation::GlobalEightBit
        );
        // Without a body, the whole thing is the header block.
        assert_eq!(
            classify("Subject: Ñandú".as_bytes()),
            Encapsulation::GlobalEightBit
        );
        assert_eq!(
            classify(b"Subject: a\r\n\r\nx\0y\r\n"),
            Encapsulation::Global
        );
        let line = |n: usize| {
            let mut bytes = b"Subject: a\r\n\r\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'a', n));
            bytes.extend_from_slice(b"\r\nfin");
            bytes
        };
        assert_eq!(classify(&line(MAX_LINE)), Encapsulation::SevenBit);
        assert_eq!(classify(&line(MAX_LINE + 1)), Encapsulation::Global);
        // The last line, without CRLF, counts too.
        let mut last = b"Subject: a\r\n\r\n".to_vec();
        last.extend(std::iter::repeat_n(b'a', MAX_LINE + 1));
        assert_eq!(classify(&last), Encapsulation::Global);
    }

    #[test]
    fn canonical_line_ends() {
        assert!(matches!(canonical(b"a\r\nb\r\n"), Cow::Borrowed(_)));
        assert_eq!(canonical(b"a\nb\rc\r\n").as_ref(), b"a\r\nb\r\nc\r\n");
        assert_eq!(canonical(b"a\r").as_ref(), b"a\r\n");
    }

    #[test]
    fn boundaries_are_random_hex() {
        let one = boundary().unwrap();
        let two = boundary().unwrap();
        assert_ne!(one, two);
        assert_eq!(one.len(), 32);
        assert!(
            one.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn delimiters_clash_by_prefix() {
        let found = Delimiters::of(
            b"Content-Type: multipart/mixed; boundary=\"Ab\"\r\n\r\n--ab\r\nx\r\n--ab--  \r\n--  \r\n",
        );
        assert!(found.clash("ab12"));
        assert!(found.clash("AB12".to_ascii_lowercase().as_str()));
        assert!(!found.clash("a0"));
        // `--` alone (a signature) can't be a boundary.
        assert!(!found.clash("ff"));
        let undeclared = Delimiters::of(b"Content-Type: multipart/mixed; boundary=0\r\n\r\n");
        assert!(undeclared.clash("0abc"));
        // A line that starts with our boundary clashes too.
        assert!(Delimiters::of(b"--0123456789abcdef\r\n").clash("0123"));
    }

    /// R1: forms of `boundary` that parsers decode (mail-parser 1.0,
    /// RFC 2231) but the scan can't read: they clash with everything, so
    /// the part goes encoded.
    #[test]
    fn hidden_boundaries_clash_with_everything() {
        for declared in [
            "boundary = \"0\"",
            "boundary =0",
            "boundary= 0",
            "boundary*=''0",
            "boundary*=utf-8''%30",
            "boundary*0=\"0\"",
            "boundary*0*=''0",
            "BOUNDARY*1=0",
            "boundary=(c)0",
            "boundary(c)=0",
            "boundary=\"\\0\"",
            "boundary=\"0",
            "boundary=\"\"",
            "boundary=",
            "boundary=0\"x",
            "boundary\r\n =0",
        ] {
            let message = format!("Content-Type: multipart/mixed; {declared}\r\n\r\nx\r\n");
            assert_eq!(boundary_values(message.as_bytes()), None, "{declared}");
            let found = Delimiters::of(message.as_bytes());
            assert!(found.clash("0abc") && found.clash("fabc"), "{declared}");
        }
        // Plain forms are read, with a comment after the value.
        let plain = b"Content-Type: multipart/mixed; boundary=0 (c); x=y\r\n\r\n";
        assert_eq!(boundary_values(plain), Some(vec![&b"0"[..]]));
        let found = Delimiters::of(plain);
        assert!(found.clash("0abc") && !found.clash("fabc"));
        // A word in the text is no parameter.
        let prose = b"Subject: x\r\n\r\nThe boundary is here; boundaryless (or not).\r\n";
        assert_eq!(boundary_values(prose), Some(Vec::new()));
        assert!(!Delimiters::of(prose).clash("0abc"));
        // An unclosed or overlong comment after the name: unreadable.
        assert_eq!(boundary_values(b"boundary (never closed"), None);
        let long = format!("boundary ({}) =0", "c".repeat(MAX_CFWS));
        assert_eq!(boundary_values(long.as_bytes()), None);
    }

    /// The second layer: what mail-parser decodes is added too, also from
    /// a nested message.
    #[test]
    fn parsed_boundaries_are_added() {
        let nested = b"Subject: x\r\nContent-Type: message/rfc822\r\n\r\n\
            Subject: y\r\nContent-Type: multipart/mixed; boundary=\"Ab\"\r\n\r\n\
            --Ab\r\n\r\nuno\r\n--Ab--\r\n";
        let found = Delimiters::of(nested);
        assert!(found.clash("ab12"));
        assert!(!found.clash("a0"));
    }

    #[test]
    fn plans_avoid_or_encode() {
        let tries = std::cell::Cell::new(0_u32);
        let next = || {
            tries.set(tries.get() + 1);
            Ok(format!("{:x}{}", tries.get() % 16, "0".repeat(31)))
        };
        // Clashes with 1…3: the fourth try is taken.
        let part = Delimiters::of(b"--1\r\n--2\r\n--3\r\n");
        let plan = plan_boundaries(1, &[part], next).unwrap();
        assert_eq!(plan.encoded, vec![false]);
        assert!(plan.boundaries[0].starts_with('4'));
        // Clashes with everything: encoded, and the plan goes on.
        let all = Delimiters::of(
            (0..16_u8)
                .fold(String::new(), |mut out, n| {
                    let _ = write!(out, "--{n:x}\r\n");
                    out
                })
                .as_bytes(),
        );
        let clean = Delimiters::of(b"nothing\r\n");
        let plan = plan_boundaries(2, &[all, clean], boundary).unwrap();
        assert_eq!(plan.encoded, vec![true, false]);
        assert_eq!(plan.boundaries.len(), 2);
        // Too many to compare: encoded.
        let many = (0..=MAX_DELIMITERS).fold(String::new(), |mut out, n| {
            let _ = write!(out, "--x{n}\r\n");
            out
        });
        let plan = plan_boundaries(1, &[Delimiters::of(many.as_bytes())], boundary).unwrap();
        assert_eq!(plan.encoded, vec![true]);
    }

    #[test]
    fn base64_in_lines() {
        let out = base64_lines(&[0xff; 100]);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 76);
        assert!(!text.ends_with("\r\n"));
        assert!(base64_lines(b"").is_empty());
    }

    #[test]
    fn content_type_keeps_its_parameters() {
        assert_eq!(
            global_content_type(b" message/rfc822; name=\"a.eml\"\r\n"),
            b" message/global; name=\"a.eml\"\r\n"
        );
        assert_eq!(
            global_content_type(b" Message/RFC822\r\n"),
            b" message/global\r\n"
        );
    }
}
