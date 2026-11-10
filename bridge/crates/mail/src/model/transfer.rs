// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Content-Transfer-Encoding decoders that work piece by piece: the input
//! can be cut anywhere (in the middle of a base64 group, of a `=XX` escape
//! or of a line) and the output is the same as decoding it all at once.
//!
//! They give exactly what `mail-parser` gives for the body of a part
//! (`MessagePart::decoded`, that is `mail_parser::Encoding::decode`),
//! because a part's size and contents were shown to the client from
//! `mail-parser`. Downloads only use them for parts where decoding the
//! whole part this way was checked to give `mail-parser`'s bytes
//! ([`super::layout`]); a different result never reaches a client.
//!
//! Both decoders are lenient, like `mail-parser`'s: they never fail. A
//! server that sends other bytes than announced is caught by the size
//! check of the download.

use encodify::base64;

/// How the part's body is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// `7bit`, `8bit`, `binary`: the bytes as they are.
    Identity,
    /// `base64`
    Base64,
    /// `quoted-printable`
    QuotedPrintable,
}

/// A decoder in the middle of a part.
#[derive(Debug, Clone)]
pub(crate) enum Decoder {
    Identity,
    Base64(base64::Decoder),
    QuotedPrintable(QuotedPrintable),
}

impl Decoder {
    pub(crate) fn new(encoding: Encoding) -> Self {
        match encoding {
            Encoding::Identity => Self::Identity,
            Encoding::Base64 => Self::Base64(base64::MIME.decoder()),
            Encoding::QuotedPrintable => Self::QuotedPrintable(QuotedPrintable::default()),
        }
    }

    /// Decodes the next piece, appending to `out`.
    pub(crate) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        match self {
            Self::Identity => out.extend_from_slice(input),
            Self::Base64(decoder) => push_base64(decoder, input, out),
            Self::QuotedPrintable(decoder) => decoder.push(input, out),
        }
    }

    /// Bytes held back, waiting for what comes next (the spaces and tabs
    /// of a quoted-printable line). A hostile part can make this grow with
    /// every piece: the caller caps it ([`check_pending`]).
    pub(crate) fn pending(&self) -> usize {
        match self {
            Self::Identity | Self::Base64(_) => 0,
            Self::QuotedPrintable(decoder) => {
                let blanks = match &decoder.state {
                    QpState::EqualsBlanks(blanks) | QpState::EqualsBlanksCr(blanks) => blanks.len(),
                    _ => 0,
                };
                decoder.held.len().saturating_add(blanks)
            }
        }
    }

    /// The end of the part: appends what was held back.
    pub(crate) fn finish(self, out: &mut Vec<u8>) {
        match self {
            Self::Identity => {}
            Self::Base64(decoder) => decoder.finish(out),
            Self::QuotedPrintable(decoder) => decoder.finish(out),
        }
    }

    /// Decodes a whole body at once.
    pub(crate) fn decode_all(encoding: Encoding, input: &[u8]) -> Vec<u8> {
        let mut decoder = Self::new(encoding);
        let mut out = Vec::with_capacity(input.len());
        decoder.push(input, &mut out);
        decoder.finish(&mut out);
        out
    }
}

/// Fails if `decoder` holds back more than `max` bytes: a run of spaces
/// and tabs that long is not a real message, and keeping it would let a
/// download grow without bound in memory.
pub(crate) fn check_pending(decoder: &Decoder, max: usize) -> Result<(), crate::MailError> {
    if decoder.pending() > max {
        return Err(crate::MailError::Invalid("quoted-printable run too long"));
    }
    Ok(())
}

/// Base64 as `mail-parser` reads it when its strict pass fails: alphabet
/// symbols decoded, `=` completes the group (decoding goes on after it),
/// any other byte skipped, an unfinished group at the end completed. Its
/// strict pass gives the same bytes when it succeeds (checked by the
/// tests below).
fn push_base64(decoder: &mut base64::Decoder, mut input: &[u8], out: &mut Vec<u8>) {
    loop {
        let used = decoder.decode_symbols(input, out);
        input = input.get(used..).unwrap_or_default();
        match input.split_first() {
            None => return,
            Some((b'=', rest)) => {
                decoder.pad(out);
                input = rest;
            }
            Some((_, rest)) => input = rest,
        }
    }
}

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte.saturating_sub(b'0'),
        b'A'..=b'F' => byte.saturating_sub(b'A').saturating_add(10),
        b'a'..=b'f' => byte.saturating_sub(b'a').saturating_add(10),
        _ => 0,
    }
}

fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum QpState {
    #[default]
    Text,
    /// After a `\r` in the text: a line break if `\n` follows, else dropped.
    Cr,
    /// After `=`.
    Equals,
    /// After `=` and a `\r`.
    EqualsCr,
    /// After `=` and one hex digit.
    EqualsHex(u8),
    /// After `=` and spaces or tabs (a soft line break if a line break or
    /// the end follows).
    EqualsBlanks(Vec<u8>),
    /// After `=`, spaces or tabs, and a `\r`.
    EqualsBlanksCr(Vec<u8>),
}

/// Quoted-printable as `mail-parser` reads it (`encodify::qp::BODY`):
/// `=XX` escapes (any case), soft line breaks (`=`, spaces or tabs, then a
/// line break or the end), a malformed escape kept as `=` and the byte
/// after it (`=` alone before a bare `\r`), CRLF and LF line breaks kept,
/// a bare CR dropped, and the spaces and tabs that end a line or the input
/// deleted (not those before a soft line break).
#[derive(Debug, Clone, Default)]
pub(crate) struct QuotedPrintable {
    state: QpState,
    /// Spaces and tabs at the end of the output so far, held back: a line
    /// break or the end drops them.
    held: Vec<u8>,
}

impl QuotedPrintable {
    fn commit(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.held);
    }

    fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &byte in input {
            self.byte(byte, out);
        }
    }

    fn byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        match std::mem::take(&mut self.state) {
            QpState::Text => self.text(byte, out),
            QpState::Cr => {
                if byte == b'\n' {
                    self.held.clear();
                    out.extend_from_slice(b"\r\n");
                } else {
                    // A bare CR is dropped.
                    self.text(byte, out);
                }
            }
            QpState::Equals => match byte {
                b'\n' => {}
                b'\r' => self.state = QpState::EqualsCr,
                _ if is_hex(byte) => self.state = QpState::EqualsHex(byte),
                _ if is_blank(byte) => self.state = QpState::EqualsBlanks(vec![byte]),
                // Malformed: `=` and this byte, as they are.
                _ => out.extend_from_slice(&[b'=', byte]),
            },
            QpState::EqualsCr => {
                if byte != b'\n' {
                    // Malformed: `=` alone, the CR is dropped.
                    out.push(b'=');
                    self.text(byte, out);
                }
            }
            QpState::EqualsHex(high) => {
                if is_hex(byte) {
                    out.push((hex_value(high) << 4) | hex_value(byte));
                } else {
                    out.extend_from_slice(&[b'=', high]);
                    self.text(byte, out);
                }
            }
            QpState::EqualsBlanks(mut blanks) => match byte {
                b'\n' => {}
                b'\r' => self.state = QpState::EqualsBlanksCr(blanks),
                _ if is_blank(byte) => {
                    blanks.push(byte);
                    self.state = QpState::EqualsBlanks(blanks);
                }
                _ => {
                    self.malformed_blanks(&blanks, out);
                    self.text(byte, out);
                }
            },
            QpState::EqualsBlanksCr(blanks) => {
                if byte != b'\n' {
                    // The CR is a bare one: dropped.
                    self.malformed_blanks(&blanks, out);
                    self.text(byte, out);
                }
            }
        }
    }

    /// `=` and blanks not followed by a line break: `=` and the first blank
    /// are kept, the other blanks are text again.
    fn malformed_blanks(&mut self, blanks: &[u8], out: &mut Vec<u8>) {
        if let Some((first, rest)) = blanks.split_first() {
            out.extend_from_slice(&[b'=', *first]);
            self.held.extend_from_slice(rest);
        }
    }

    fn text(&mut self, byte: u8, out: &mut Vec<u8>) {
        match byte {
            b'=' => {
                // Blanks before an escape or a soft line break stay.
                self.commit(out);
                self.state = QpState::Equals;
            }
            b'\r' => self.state = QpState::Cr,
            b'\n' => {
                self.held.clear();
                out.push(b'\n');
            }
            _ if is_blank(byte) => self.held.push(byte),
            _ => {
                self.commit(out);
                out.push(byte);
            }
        }
    }

    fn finish(self, out: &mut Vec<u8>) {
        match self.state {
            // A soft line break at the end, or a dropped CR.
            QpState::Text | QpState::Cr | QpState::Equals | QpState::EqualsBlanks(_) => {}
            QpState::EqualsCr => out.push(b'='),
            QpState::EqualsHex(high) => out.extend_from_slice(&[b'=', high]),
            QpState::EqualsBlanksCr(blanks) => {
                if let Some(first) = blanks.first() {
                    out.extend_from_slice(&[b'=', *first]);
                }
            }
        }
        // The blanks that end the input are deleted.
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn pending_counts_held_blanks() {
        let mut decoder = Decoder::new(Encoding::QuotedPrintable);
        let mut out = Vec::new();
        decoder.push(b"a   ", &mut out);
        assert_eq!(decoder.pending(), 3);
        decoder.push(b"=  \t", &mut out);
        assert_eq!(decoder.pending(), 3);
        assert!(check_pending(&decoder, 3).is_ok());
        assert!(check_pending(&decoder, 2).is_err());
        decoder.push(b"x", &mut out);
        assert_eq!(decoder.pending(), 0);
        decoder.push(b"  \r\n", &mut out);
        assert_eq!(decoder.pending(), 0);
        let mut base64 = Decoder::new(Encoding::Base64);
        base64.push(b"SG9s    ", &mut out);
        assert_eq!(base64.pending(), 0);
    }

    /// What `mail-parser` decodes for a part body in this encoding (and
    /// checks that the size it gives without decoding, JMAP's `size`, is
    /// that length).
    fn mail_parser(encoding: Encoding, body: &[u8]) -> Vec<u8> {
        let parsed = match encoding {
            Encoding::Identity => mail_parser::Encoding::None,
            Encoding::Base64 => mail_parser::Encoding::Base64,
            Encoding::QuotedPrintable => mail_parser::Encoding::QuotedPrintable,
        };
        let decoded = parsed.decode(body).into_owned();
        assert_eq!(parsed.decoded_len(body), decoded.len(), "{body:?}");
        decoded
    }

    fn pieces(encoding: Encoding, body: &[u8], cuts: &[usize]) -> Vec<u8> {
        let mut decoder = Decoder::new(encoding);
        let mut out = Vec::new();
        let mut start = 0;
        let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (body.len() + 1)).collect();
        cuts.sort_unstable();
        for cut in cuts.into_iter().chain([body.len()]) {
            decoder.push(&body[start..cut], &mut out);
            start = cut;
        }
        decoder.finish(&mut out);
        out
    }

    #[test]
    fn base64_examples() {
        let all = |s: &[u8]| Decoder::decode_all(Encoding::Base64, s);
        assert_eq!(all(b"SGVs\r\nbG8=\r\n"), b"Hello");
        assert_eq!(all(b"SGVsbG8"), b"Hello", "unpadded end decoded");
        assert_eq!(all(b"SGk=SGk="), b"HiHi");
        assert_eq!(all(b"SGVs*bG8="), b"Hello", "other bytes skipped");
        assert_eq!(
            pieces(Encoding::Base64, b"SGVsbG8gd29ybGQ=", &[1, 2, 7, 14]),
            b"Hello world"
        );
    }

    #[test]
    fn quoted_printable_examples() {
        let all = |s: &[u8]| Decoder::decode_all(Encoding::QuotedPrintable, s);
        assert_eq!(
            all(b"caf=C3=A9 =\r\nau lait  \r\nfin  "),
            "café au lait\r\nfin".as_bytes()
        );
        assert_eq!(all(b"a=ZZb=4"), b"a=ZZb=4");
        assert_eq!(all(b"a==41"), b"a==41");
        assert_eq!(all(b"soft=  "), b"soft");
        assert_eq!(all(b"x=3d=3D"), b"x==");
        assert_eq!(
            pieces(
                Encoding::QuotedPrintable,
                b"caf=C3=A9 =\r\nau  \r\nx",
                &[3, 4, 5, 11, 12, 16]
            ),
            "café au\r\nx".as_bytes()
        );
    }

    fn check(encoding: Encoding, body: &[u8], cuts: &[usize]) {
        let whole = Decoder::decode_all(encoding, body);
        assert_eq!(whole, mail_parser(encoding, body), "{body:?}");
        assert_eq!(
            pieces(encoding, body, cuts),
            whole,
            "cut anywhere = all at once: {body:?}"
        );
    }

    fn base64_text() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(
            prop_oneof![
                8 => proptest::sample::select(
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".to_vec()
                ),
                1 => proptest::sample::select(b"= \t\r\n-".to_vec()),
                1 => any::<u8>(),
            ],
            0..200,
        )
    }

    fn qp_text() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(
            prop_oneof![
                6 => proptest::sample::select(b"abcXYZ019AFaf.,;".to_vec()),
                3 => proptest::sample::select(b"= \t\r\n\x0c".to_vec()),
                1 => any::<u8>(),
            ],
            0..200,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        #[test]
        fn base64_matches_mail_parser(body in base64_text(), cuts in proptest::collection::vec(any::<usize>(), 0..6)) {
            check(Encoding::Base64, &body, &cuts);
        }

        #[test]
        fn quoted_printable_matches_mail_parser(body in qp_text(), cuts in proptest::collection::vec(any::<usize>(), 0..6)) {
            check(Encoding::QuotedPrintable, &body, &cuts);
        }
    }
}
