// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Modified UTF-7 for mailbox names (RFC 3501 §5.1.3).
//!
//! Printable ASCII except `&` stands for itself, `&` is written `&-`, and
//! any other text is UTF-16BE in a base64 variant (`,` instead of `/`, no
//! padding) between `&` and `-`.

use base64::Engine as _;
use base64::alphabet::IMAP_MUTF7;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// The base64 variant of modified UTF-7. Decoding is strict (no padding,
/// no stray trailing bits), so every valid name has exactly one encoding.
const ENGINE: GeneralPurpose = GeneralPurpose::new(
    &IMAP_MUTF7,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

/// Why a name is not valid modified UTF-7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Utf7Error {
    /// A byte outside printable US-ASCII.
    #[error("byte outside printable ASCII")]
    NotPrintable,
    /// `&` without its closing `-`.
    #[error("unterminated shift sequence")]
    Unterminated,
    /// Bad base64, or bits left over.
    #[error("invalid base64 in shift sequence")]
    Base64,
    /// The UTF-16 inside is broken (odd length, lone surrogate).
    #[error("invalid UTF-16 in shift sequence")]
    Utf16,
    /// Not the canonical form: printable ASCII encoded, or two shift
    /// sequences next to each other.
    #[error("non-canonical modified UTF-7")]
    NotCanonical,
}

fn is_direct(ch: char) -> bool {
    matches!(ch, ' '..='~')
}

/// Encodes a UTF-8 name as modified UTF-7.
#[must_use]
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u8> = Vec::new();
    for ch in name.chars() {
        if is_direct(ch) {
            flush(&mut out, &mut pending);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                pending.extend_from_slice(&unit.to_be_bytes());
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}

fn flush(out: &mut String, pending: &mut Vec<u8>) {
    if pending.is_empty() {
        return;
    }
    out.push('&');
    ENGINE.encode_string(pending.as_slice(), out);
    out.push('-');
    pending.clear();
}

/// Decodes a modified UTF-7 name to UTF-8.
///
/// # Errors
///
/// Anything that is not the canonical modified UTF-7 of some text.
pub fn decode(wire: &str) -> Result<String, Utf7Error> {
    let mut out = String::with_capacity(wire.len());
    let mut rest = wire;
    // True right after a base64 section: another one must not follow.
    let mut after_shift = false;
    while let Some(ch) = rest.chars().next() {
        if !is_direct(ch) {
            return Err(Utf7Error::NotPrintable);
        }
        let after = rest.get(ch.len_utf8()..).unwrap_or_default();
        if ch != '&' {
            out.push(ch);
            rest = after;
            after_shift = false;
            continue;
        }
        let (section, tail) = after.split_once('-').ok_or(Utf7Error::Unterminated)?;
        rest = tail;
        if section.is_empty() {
            out.push('&');
            after_shift = false;
            continue;
        }
        if after_shift {
            return Err(Utf7Error::NotCanonical);
        }
        decode_section(section, &mut out)?;
        after_shift = true;
    }
    Ok(out)
}

fn decode_section(section: &str, out: &mut String) -> Result<(), Utf7Error> {
    let bytes = ENGINE.decode(section).map_err(|_| Utf7Error::Base64)?;
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(Utf7Error::Utf16);
    }
    let units = pairs.iter().map(|pair| u16::from_be_bytes(*pair));
    for ch in char::decode_utf16(units) {
        let ch = ch.map_err(|_| Utf7Error::Utf16)?;
        if is_direct(ch) {
            return Err(Utf7Error::NotCanonical);
        }
        out.push(ch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rfc_examples() {
        // RFC 3501 §5.1.3: "~peter/mail/台北/日本語"
        let name = "~peter/mail/\u{53f0}\u{5317}/\u{65e5}\u{672c}\u{8a9e}";
        assert_eq!(encode(name), "~peter/mail/&U,BTFw-/&ZeVnLIqe-");
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-").as_deref(),
            Ok(name)
        );
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(decode("Tom &- Jerry").as_deref(), Ok("Tom & Jerry"));
        assert_eq!(encode("Enviados é"), "Enviados &AOk-");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(decode("&Jjo!"), Err(Utf7Error::Unterminated));
        assert_eq!(decode("&U,BTFw-&ZeVnLIqe-"), Err(Utf7Error::NotCanonical));
        assert_eq!(decode("&AGE-"), Err(Utf7Error::NotCanonical));
        assert_eq!(decode("caf\u{e9}"), Err(Utf7Error::NotPrintable));
        assert!(decode("&2D3-").is_err());
        assert!(decode("&2D0-").is_err());
    }

    proptest! {
        #[test]
        fn round_trip(name in "\\PC{0,30}") {
            let wire = encode(&name);
            prop_assert!(wire.bytes().all(|b| (0x20..=0x7e).contains(&b)));
            prop_assert_eq!(decode(&wire), Ok(name));
        }

        #[test]
        fn decode_is_canonical(wire in "[ -~]{0,30}") {
            if let Ok(name) = decode(&wire) {
                prop_assert_eq!(encode(&name), wire);
            }
        }
    }
}
