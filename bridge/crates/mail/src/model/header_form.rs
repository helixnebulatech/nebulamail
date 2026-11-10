// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The Raw and Text forms of a header field value (RFC 8621 §4.1.2.1 and
//! §4.1.2.2), from the bytes after the field name's colon.
//!
//! mail-parser's own text parser trims both ends and turns folding white
//! space into one space; RFC 8621 keeps every octet except the ones its
//! steps remove, so the steps are done here and only single encoded words
//! go to `encodify` and mail-parser's charsets.

use std::borrow::Cow;

use encodify::rfc2047::EncodedWord;
use icu_normalizer::ComposingNormalizerBorrowed;
use mail_parser::Charset;

/// The value without the field's terminating line break: CRLF, or a bare
/// LF in a message stored with Unix line ends.
fn without_terminator(value: &[u8]) -> &[u8] {
    value
        .strip_suffix(b"\r\n")
        .or_else(|| value.strip_suffix(b"\n"))
        .unwrap_or(value)
}

/// Bytes as text: invalid UTF-8 becomes U+FFFD and NUL octets are dropped
/// (RFC 8621 §4.1.2.1).
fn lossy_without_nul(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\0') {
        text.replace('\0', "")
    } else {
        text.into_owned()
    }
}

/// The Raw form (RFC 8621 §4.1.2.1): "from the first octet following the
/// header field name terminating colon, up to but excluding the header field
/// terminating CRLF", with invalid UTF-8 replaced and NUL dropped. Leading
/// and trailing white space is kept.
#[must_use]
pub fn raw(value: &[u8]) -> String {
    lossy_without_nul(without_terminator(value))
}

/// Unfolding (RFC 5322 §2.2.3): "removing any CRLF that is immediately
/// followed by WSP" (or a bare LF, for Unix line ends).
fn unfold(value: &[u8]) -> Cow<'_, [u8]> {
    if !value.contains(&b'\n') {
        return Cow::Borrowed(value);
    }
    let mut out = Vec::with_capacity(value.len());
    let mut rest = value;
    while let Some((&byte, tail)) = rest.split_first() {
        let line_break = match byte {
            b'\r' if tail.first() == Some(&b'\n') => 2,
            b'\n' => 1,
            _ => 0,
        };
        let followed_by_wsp = matches!(rest.get(line_break), Some(b' ' | b'\t'));
        if line_break > 0 && followed_by_wsp {
            rest = rest.get(line_break..).unwrap_or_default();
        } else {
            out.push(byte);
            rest = tail;
        }
    }
    Cow::Owned(out)
}

/// NFC (Unicode Normalization Form C).
#[must_use]
pub fn nfc(text: &str) -> Cow<'_, str> {
    ComposingNormalizerBorrowed::new_nfc().normalize(text)
}

/// Decoded encoded words waiting to be converted: adjacent words in one
/// charset are joined first, so a character a sloppy encoder split across
/// two words still decodes.
struct Pending<'a> {
    charset: Option<(Charset, &'a [u8])>,
    bytes: Vec<u8>,
}

impl Pending<'_> {
    fn flush(&mut self, out: &mut String) {
        if let Some((charset, _)) = self.charset.take() {
            // RFC 8621 §4.1.2.2 step 4: NUL and control characters
            // encoded per RFC 2047 are dropped.
            out.extend(
                charset
                    .decode(&self.bytes)
                    .chars()
                    .filter(|c| !c.is_control()),
            );
        }
        self.bytes.clear();
    }
}

/// The token as one whole RFC 2047 encoded word with a known charset.
fn encoded_word(token: &[u8]) -> Option<(EncodedWord<'_>, Charset)> {
    let (word, consumed) = EncodedWord::parse(token)?;
    if consumed != token.len() {
        // RFC 2047 §5 (1): an encoded word in text is separated from
        // other text by white space; `foo=?…?=` is not one.
        return None;
    }
    let charset = Charset::from_label(word.charset)?;
    Some((word, charset))
}

/// The Text form (RFC 8621 §4.1.2.2) of a field value:
/// 1. white space unfolded (RFC 5322 §2.2.3);
/// 2. the terminating CRLF removed;
/// 3. any SP at the beginning removed (only SP: a leading tab stays, and
///    so does trailing white space);
/// 4. syntactically correct RFC 2047 encoded words with a known charset
///    decoded (the white space between two of them dropped, RFC 2047
///    §6.2), NUL and control characters from them dropped; text that only
///    looks like an encoded word is kept as written; a word that can't be
///    decoded becomes U+FFFD;
/// 5. the result in NFC.
///
/// Octets outside encoded words are read as UTF-8 (RFC 6532), invalid
/// ones replaced, and NUL dropped as in the Raw form.
#[must_use]
pub fn text(value: &[u8]) -> String {
    let unfolded = unfold(without_terminator(value));
    let value = unfolded
        .iter()
        .position(|&b| b != b' ')
        .and_then(|start| unfolded.get(start..))
        .unwrap_or_default();
    let mut out = String::with_capacity(value.len());
    let mut pending = Pending {
        charset: None,
        bytes: Vec::new(),
    };
    // White space after an encoded word, kept back until we know whether
    // another encoded word follows it.
    let mut held: &[u8] = &[];
    let mut after_word = false;
    let mut rest = value;
    while !rest.is_empty() {
        let is_wsp = |b: &u8| matches!(b, b' ' | b'\t');
        let space = rest.iter().take_while(|b| is_wsp(b)).count();
        if space > 0 {
            let (run, tail) = rest.split_at(space);
            rest = tail;
            if after_word {
                held = run;
            } else {
                out.push_str(&lossy_without_nul(run));
            }
            continue;
        }
        let len = rest.iter().take_while(|b| !is_wsp(b)).count();
        let (token, tail) = rest.split_at(len);
        rest = tail;
        if let Some((word, charset)) = encoded_word(token) {
            let same = pending
                .charset
                .is_some_and(|(_, label)| label.eq_ignore_ascii_case(word.charset));
            if !same {
                pending.flush(&mut out);
            }
            let mut decoded = Vec::new();
            if word.decode_append(&mut decoded).is_ok() {
                pending.bytes.extend_from_slice(&decoded);
                pending.charset = Some((charset, word.charset));
            } else {
                pending.flush(&mut out);
                out.push(char::REPLACEMENT_CHARACTER);
            }
            held = &[];
            after_word = true;
        } else {
            pending.flush(&mut out);
            out.push_str(&lossy_without_nul(held));
            held = &[];
            out.push_str(&lossy_without_nul(token));
            after_word = false;
        }
    }
    pending.flush(&mut out);
    out.push_str(&lossy_without_nul(held));
    match nfc(&out) {
        Cow::Borrowed(_) => out,
        Cow::Owned(normalized) => normalized,
    }
}

/// A display name (RFC 8621 §4.1.2.3): white space unfolded, then leading
/// and trailing white space removed, in NFC like the Text form whose
/// rules its encoded words follow.
#[must_use]
pub fn display_name(name: &str) -> String {
    let unfolded: String = name.chars().filter(|&c| c != '\r' && c != '\n').collect();
    nfc(unfolded.trim()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_keeps_everything_but_the_terminating_crlf() {
        assert_eq!(
            raw(b"  Hola \t \r\n  mundo  \r\n"),
            "  Hola \t \r\n  mundo  "
        );
        assert_eq!(raw(b" z \n"), " z ");
        assert_eq!(raw(b" no end"), " no end");
        // Only the last CRLF goes.
        assert_eq!(raw(b" a\r\n\r\n"), " a\r\n");
        assert_eq!(raw(b" a\x00b\r\n"), " ab");
        assert_eq!(raw(b" \xffx\r\n"), " \u{fffd}x");
    }

    #[test]
    fn text_follows_the_steps() {
        // Unfold (only the CRLF goes), leading SP, trailing kept.
        assert_eq!(text(b"  Hola \t \r\n  mundo  \r\n"), "Hola \t   mundo  ");
        assert_eq!(text(b" a\n b\n"), "a b");
        // Only SP is removed at the start.
        assert_eq!(text(b"\tx\r\n"), "\tx");
        // NFC.
        assert_eq!(text(b" cafe\xcc\x81\r\n"), "caf\u{e9}");
        // Encoded words, and the space between two of them.
        assert_eq!(text(b" =?utf-8?q?caf=C3=A9?=  x \r\n"), "caf\u{e9}  x ");
        assert_eq!(text(b" =?utf-8?q?_sp_?= =?utf-8?q?a?=\r\n"), " sp a");
        assert_eq!(
            text(b" Re: =?utf-8?q?caf=C3?=\r\n =?UTF-8?Q?=A9?= menu\r\n"),
            "Re: caf\u{e9} menu"
        );
        assert_eq!(
            text(b" =?iso-8859-1?q?caf=E9?= =?utf-8?b?w7E=?=\r\n"),
            "caf\u{e9}\u{f1}"
        );
        // Placement rules: not decoded.
        assert_eq!(text(b" foo=?utf-8?q?x?= \r\n"), "foo=?utf-8?q?x?= ");
        assert_eq!(text(b" (=?utf-8?q?x?=)\r\n"), "(=?utf-8?q?x?=)");
        // Unknown charset: as written.
        assert_eq!(text(b" =?x-nope?q?x?=\r\n"), "=?x-nope?q?x?=");
        // Encoded NUL and controls dropped; raw NUL dropped.
        assert_eq!(text(b" =?utf-8?q?a=00=07b?=\r\n"), "ab");
        assert_eq!(text(b" a\x00b\r\n"), "ab");
        // A word that fails to decode.
        assert_eq!(text(b" =?utf-8?b?###?= x\r\n"), "\u{fffd} x");
        assert_eq!(text(b""), "");
        assert_eq!(text(b"   \r\n"), "");
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("  James Smythe"), "James Smythe");
        assert_eq!(display_name(" A\r\n  B "), "A  B");
        assert_eq!(display_name("cafe\u{301}"), "caf\u{e9}");
    }
}
