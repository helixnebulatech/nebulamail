// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Percent-encoding of one path segment (RFC 3986 §2.1), strict.

use std::fmt::Write as _;

use crate::HrefError;

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes every `%XX` of a raw segment. The result must be UTF-8.
pub(crate) fn decode_segment(raw: &str) -> Result<String, HrefError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let high = bytes.get(i + 1).copied().and_then(hex_value);
            let low = bytes.get(i + 2).copied().and_then(hex_value);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(HrefError::PercentEncoding);
            };
            out.push(high * 16 + low);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| HrefError::PercentEncoding)
}

/// Characters written as they are in a segment: RFC 3986 `pchar` minus
/// `%` (unreserved, sub-delims, `:` and `@`). Everything else, including
/// every non-ASCII byte, is `%XX` with uppercase hex. So the encoding of a
/// decoded segment is unique, and two hrefs that differ only in how they
/// were encoded compare equal (libdav normalises the same way).
const fn is_plain(byte: u8) -> bool {
    matches!(byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
        | b'-' | b'.' | b'_' | b'~'
        | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
        | b':' | b'@')
}

pub(crate) fn encode_segment(segment: &str, out: &mut String) {
    for &byte in segment.as_bytes() {
        if is_plain(byte) {
            out.push(char::from(byte));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_segment, encode_segment};
    use crate::HrefError;

    fn encode(segment: &str) -> String {
        let mut out = String::new();
        encode_segment(segment, &mut out);
        out
    }

    #[test]
    fn decodes_and_reencodes_canonically() {
        assert_eq!(
            decode_segment("alice%40example.com").unwrap(),
            "alice@example.com"
        );
        assert_eq!(decode_segment("caf%C3%A9").unwrap(), "café");
        assert_eq!(decode_segment("caf%c3%a9").unwrap(), "café");
        assert_eq!(encode("café"), "caf%C3%A9");
        assert_eq!(encode("a b?#%"), "a%20b%3F%23%25");
        assert_eq!(encode("alice@example.com"), "alice@example.com");
    }

    #[test]
    fn rejects_bad_escapes() {
        for bad in ["%", "%4", "%zz", "a%g0", "%C3", "%FF"] {
            assert_eq!(
                decode_segment(bad),
                Err(HrefError::PercentEncoding),
                "{bad}"
            );
        }
    }
}
