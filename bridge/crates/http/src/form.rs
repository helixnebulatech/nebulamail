// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `application/x-www-form-urlencoded` parameters (query strings and form
//! bodies), with the OAuth rule that a parameter must not repeat
//! (RFC 6749 §3.1, §3.2).

use std::collections::HashMap;

/// A parameter appeared more than once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DuplicateParameter;

/// Decoded parameters. Empty values count as absent (RFC 6749 §3.1).
#[derive(Debug, Default)]
pub(crate) struct Form(HashMap<String, String>);

impl Form {
    /// Parses `a=1&b=2`.
    pub(crate) fn parse(input: &[u8]) -> Result<Self, DuplicateParameter> {
        let mut map = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for (name, value) in url::form_urlencoded::parse(input) {
            if !seen.insert(name.clone().into_owned()) {
                return Err(DuplicateParameter);
            }
            if !value.is_empty() {
                map.insert(name.into_owned(), value.into_owned());
            }
        }
        Ok(Self(map))
    }

    /// The value of `name`, if present and not empty.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// Decodes one form-encoded component (`+` is a space, `%XX` a byte), as
/// the parts of `client_secret_basic` are (RFC 6749 §2.3.1). `None` if a
/// `%` escape is malformed or the result isn't UTF-8.
pub(crate) fn decode_component(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = hex(input.next()?)?;
                let low = hex(input.next()?)?;
                bytes.push(high << 4 | low);
            }
            other => bytes.push(other),
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects_duplicates() {
        let form = Form::parse(b"a=1&b=x+y%21&c=").unwrap();
        assert_eq!(form.get("a"), Some("1"));
        assert_eq!(form.get("b"), Some("x y!"));
        assert_eq!(form.get("c"), None);
        assert_eq!(form.get("d"), None);
        assert_eq!(Form::parse(b"a=1&a=2").unwrap_err(), DuplicateParameter);
        assert_eq!(Form::parse(b"a=&a=2").unwrap_err(), DuplicateParameter);
    }

    #[test]
    fn decodes_components() {
        assert_eq!(
            decode_component("s3cret+value").as_deref(),
            Some("s3cret value")
        );
        assert_eq!(decode_component("a%3Ab%25").as_deref(), Some("a:b%"));
        assert_eq!(decode_component("%C3%B1").as_deref(), Some("ñ"));
        assert_eq!(decode_component("%"), None);
        assert_eq!(decode_component("%zz"), None);
        assert_eq!(decode_component("%FF"), None);
    }
}
