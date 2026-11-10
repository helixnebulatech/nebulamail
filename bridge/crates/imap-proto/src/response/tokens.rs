// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A small, lenient tokenizer for one complete response.
//!
//! We use it where `imap-proto` has gaps (PREVIEW, NAMESPACE, SEARCH with
//! MODSEQ, LIST with extended data, `CAPABILITY` without `IMAP4rev1`). It
//! turns the response into atoms, strings and nested lists, with a depth
//! limit, and never panics.

use std::borrow::Cow;

/// One token of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    /// An atom, number or `NIL`. Brackets are part of the atom, so
    /// `BODY[HEADER.FIELDS (FROM)]<0>` is one token.
    Atom(&'a [u8]),
    /// A quoted string, unescaped.
    Quoted(Cow<'a, [u8]>),
    /// Literal data.
    Literal(&'a [u8]),
    /// A parenthesised list.
    List(Vec<Token<'a>>),
}

impl Token<'_> {
    /// The atom as text, if it is an atom.
    pub(crate) fn atom(&self) -> Option<&str> {
        match self {
            Self::Atom(bytes) => std::str::from_utf8(bytes).ok(),
            Self::Quoted(_) | Self::Literal(_) | Self::List(_) => None,
        }
    }

    /// Whether this is the atom `NIL`.
    pub(crate) fn is_nil(&self) -> bool {
        self.atom()
            .is_some_and(|atom| atom.eq_ignore_ascii_case("NIL"))
    }

    /// The bytes of a string (quoted, literal or atom).
    pub(crate) fn string_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Atom(bytes) | Self::Literal(bytes) => Some(bytes),
            Self::Quoted(bytes) => Some(bytes),
            Self::List(_) => None,
        }
    }

    /// The items of a list.
    pub(crate) fn list(&self) -> Option<&[Token<'_>]> {
        match self {
            Self::List(items) => Some(items),
            Self::Atom(_) | Self::Quoted(_) | Self::Literal(_) => None,
        }
    }
}

/// The response does not have the token structure we expect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenError {
    /// Unbalanced parentheses, bad quoting, truncated literal…
    Malformed,
    /// Nested deeper than the limit.
    TooDeep,
    /// More tokens than the limit (a small frame must not become a huge
    /// token tree in memory).
    TooMany,
}

/// Limits of one tokenization.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TokenLimits {
    /// Deepest list nesting.
    pub(crate) max_depth: usize,
    /// Most tokens (lists included) in the whole response.
    pub(crate) max_tokens: usize,
}

/// Item names whose `[` opens a section (`BODY[HEADER.FIELDS (FROM)]`).
/// In any other atom `[` is an ordinary character: otherwise `X [ BODYSTRUCTURE
/// (((... ]` would look like one atom here while `imap-proto` parses the
/// deep list inside.
const SECTION_ITEMS: [&[u8]; 5] = [
    b"BODY",
    b"BODY.PEEK",
    b"BINARY",
    b"BINARY.PEEK",
    b"BINARY.SIZE",
];

/// Splits a complete response (ending with CRLF) into top-level tokens.
pub(crate) fn tokenize(input: &[u8], limits: TokenLimits) -> Result<Vec<Token<'_>>, TokenError> {
    let mut parser = Parser {
        input,
        position: 0,
        tokens: 0,
        max_tokens: limits.max_tokens,
    };
    let tokens = parser.sequence(0, limits.max_depth)?;
    match parser.rest() {
        b"\r\n" | b"\n" => Ok(tokens),
        _ => Err(TokenError::Malformed),
    }
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
    /// Tokens made so far.
    tokens: usize,
    max_tokens: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a [u8] {
        self.input.get(self.position..).unwrap_or_default()
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn advance(&mut self, count: usize) {
        self.position = self.position.saturating_add(count).min(self.input.len());
    }

    fn count_token(&mut self) -> Result<(), TokenError> {
        self.tokens = self.tokens.saturating_add(1);
        if self.tokens > self.max_tokens {
            return Err(TokenError::TooMany);
        }
        Ok(())
    }

    /// After an atom, a string or a literal comes a space, `)` or the line
    /// end. A token glued to `"`, `{`, `~{` or `(` is read differently by
    /// `imap-proto` (its catch-all for unknown FETCH items takes `a"b` or
    /// `a{5}` whole), so the two parsers would not agree on where literals
    /// and lists are: refuse it.
    fn check_separator(&self) -> Result<(), TokenError> {
        match self.peek() {
            Some(b'"' | b'{' | b'(') => Err(TokenError::Malformed),
            Some(b'~') if self.input.get(self.position.saturating_add(1)) == Some(&b'{') => {
                Err(TokenError::Malformed)
            }
            _ => Ok(()),
        }
    }

    /// Tokens until `)` (inside a list) or the line end.
    fn sequence(&mut self, depth: usize, max_depth: usize) -> Result<Vec<Token<'a>>, TokenError> {
        let mut tokens = Vec::new();
        loop {
            while self.peek() == Some(b' ') {
                self.advance(1);
            }
            match self.peek() {
                None => return Err(TokenError::Malformed),
                Some(b'\r' | b'\n' | b')') => return Ok(tokens),
                Some(_) => {}
            }
            self.count_token()?;
            match self.peek() {
                Some(b'(') => {
                    if depth >= max_depth {
                        return Err(TokenError::TooDeep);
                    }
                    self.advance(1);
                    let items = self.sequence(depth.saturating_add(1), max_depth)?;
                    if self.peek() != Some(b')') {
                        return Err(TokenError::Malformed);
                    }
                    self.advance(1);
                    tokens.push(Token::List(items));
                }
                Some(b'"') => {
                    tokens.push(self.quoted()?);
                    self.check_separator()?;
                }
                Some(b'{') => {
                    tokens.push(self.literal()?);
                    self.check_separator()?;
                }
                // `~{n}`: a literal8 (BINARY, RFC 3516).
                Some(b'~') if self.input.get(self.position.saturating_add(1)) == Some(&b'{') => {
                    self.advance(1);
                    tokens.push(self.literal()?);
                    self.check_separator()?;
                }
                _ => {
                    tokens.push(self.atom()?);
                    self.check_separator()?;
                }
            }
            if depth == 0 && matches!(self.peek(), Some(b'\r' | b'\n')) {
                return Ok(tokens);
            }
            if depth == 0 && self.peek() == Some(b')') {
                return Err(TokenError::Malformed);
            }
        }
    }

    fn quoted(&mut self) -> Result<Token<'a>, TokenError> {
        self.advance(1);
        let start = self.position;
        let mut unescaped: Option<Vec<u8>> = None;
        loop {
            let byte = self.peek().ok_or(TokenError::Malformed)?;
            match byte {
                b'"' => {
                    let raw = self.input.get(start..self.position).unwrap_or_default();
                    self.advance(1);
                    return Ok(Token::Quoted(
                        unescaped.map_or(Cow::Borrowed(raw), Cow::Owned),
                    ));
                }
                b'\\' => {
                    let escaped = self.input.get(self.position.saturating_add(1)).copied();
                    let Some(escaped @ (b'"' | b'\\')) = escaped else {
                        return Err(TokenError::Malformed);
                    };
                    let buffer = unescaped.get_or_insert_with(|| {
                        self.input
                            .get(start..self.position)
                            .unwrap_or_default()
                            .to_vec()
                    });
                    buffer.push(escaped);
                    self.advance(2);
                }
                b'\r' | b'\n' | 0 => return Err(TokenError::Malformed),
                other => {
                    if let Some(buffer) = unescaped.as_mut() {
                        buffer.push(other);
                    }
                    self.advance(1);
                }
            }
        }
    }

    fn literal(&mut self) -> Result<Token<'a>, TokenError> {
        let rest = self.rest();
        let close = rest
            .iter()
            .position(|&b| b == b'}')
            .ok_or(TokenError::Malformed)?;
        let digits = rest.get(1..close).ok_or(TokenError::Malformed)?;
        let digits = digits.strip_suffix(b"+").unwrap_or(digits);
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(TokenError::Malformed);
        }
        let size: usize = std::str::from_utf8(digits)
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or(TokenError::Malformed)?;
        let after_brace = close.saturating_add(1);
        if rest.get(after_brace..after_brace.saturating_add(2)) != Some(b"\r\n") {
            return Err(TokenError::Malformed);
        }
        let data_start = self.position.saturating_add(after_brace).saturating_add(2);
        let data_end = data_start.checked_add(size).ok_or(TokenError::Malformed)?;
        let data = self
            .input
            .get(data_start..data_end)
            .ok_or(TokenError::Malformed)?;
        self.position = data_end;
        Ok(Token::Literal(data))
    }

    fn atom(&mut self) -> Result<Token<'a>, TokenError> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            match byte {
                b' ' | b'(' | b')' | b'"' | b'\r' | b'\n' | b'{' | 0 => break,
                b'[' if self.opens_section(start) => self.bracket()?,
                _ => self.advance(1),
            }
        }
        let atom = self.input.get(start..self.position).unwrap_or_default();
        if atom.is_empty() {
            return Err(TokenError::Malformed);
        }
        Ok(Token::Atom(atom))
    }

    /// Whether the atom read so far (from `start`) names an item with a
    /// section, so the `[` here opens it.
    fn opens_section(&self, start: usize) -> bool {
        let name = self.input.get(start..self.position).unwrap_or_default();
        SECTION_ITEMS
            .iter()
            .any(|item| item.eq_ignore_ascii_case(name))
    }

    /// Skips `[...]` inside an atom (sections may hold spaces, lists and
    /// quoted header names).
    fn bracket(&mut self) -> Result<(), TokenError> {
        self.advance(1);
        loop {
            match self.peek().ok_or(TokenError::Malformed)? {
                b']' => {
                    self.advance(1);
                    return Ok(());
                }
                b'"' => {
                    self.quoted()?;
                }
                b'\r' | b'\n' | 0 => return Err(TokenError::Malformed),
                _ => self.advance(1),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn limits(max_depth: usize) -> TokenLimits {
        TokenLimits {
            max_depth,
            max_tokens: 1000,
        }
    }

    #[test]
    fn tokens() {
        let input = b"* 1 FETCH (UID 5 BODY[HEADER.FIELDS (FROM \"X]\")]<0> {3}\r\nabc PREVIEW \"a \\\"b\\\"\" X NIL)\r\n";
        let tokens = tokenize(input, limits(8)).unwrap();
        assert_eq!(tokens.len(), 4);
        let items = tokens[3].list().unwrap();
        assert_eq!(
            items[2],
            Token::Atom(b"BODY[HEADER.FIELDS (FROM \"X]\")]<0>")
        );
        assert_eq!(items[3], Token::Literal(b"abc"));
        assert_eq!(items[5].string_bytes(), Some(&b"a \"b\""[..]));
        assert!(items[7].is_nil());
    }

    #[test]
    fn literal8() {
        let tokens = tokenize(b"* 1 FETCH (BINARY[1] ~{3}\r\na\0b)\r\n", limits(8)).unwrap();
        let items = tokens[3].list().unwrap();
        assert_eq!(items[1], Token::Literal(b"a\0b"));
    }

    #[test]
    fn errors() {
        assert_eq!(tokenize(b"* (a\r\n", limits(8)), Err(TokenError::Malformed));
        assert_eq!(tokenize(b"* a)\r\n", limits(8)), Err(TokenError::Malformed));
        assert_eq!(
            tokenize(b"* ((a))\r\n", limits(1)),
            Err(TokenError::TooDeep)
        );
        assert_eq!(
            tokenize(b"* {5}\r\nab\r\n", limits(8)),
            Err(TokenError::Malformed)
        );
        assert_eq!(
            tokenize(b"* \"a\\x\"\r\n", limits(8)),
            Err(TokenError::Malformed)
        );
        assert_eq!(tokenize(b"* a", limits(8)), Err(TokenError::Malformed));
    }

    #[test]
    fn brackets_open_sections_only_after_section_items() {
        let input = b"* 1 FETCH (BODY[HEADER.FIELDS (FROM \"X]\")]<0> NIL BINARY.SIZE[1] 3 body.peek[TEXT] NIL)\r\n";
        let tokens = tokenize(input, limits(2)).unwrap();
        let items = tokens[3].list().unwrap();
        assert_eq!(items.len(), 6);
        assert_eq!(items[2].atom(), Some("BINARY.SIZE[1]"));
        // Elsewhere `[` is a plain character: the list after it is a list.
        let tokens = tokenize(b"* 1 FETCH (X [ (a) ])\r\n", limits(8)).unwrap();
        let items = tokens[3].list().unwrap();
        assert_eq!(items.len(), 4);
        assert!(items[2].list().is_some());
        assert_eq!(
            tokenize(b"* 1 FETCH (X [ ((((a)))) ])\r\n", limits(3)),
            Err(TokenError::TooDeep)
        );
        let tokens = tokenize(b"* 1 FETCH (FLAGS (\\Seen [x a[b))\r\n", limits(8)).unwrap();
        assert_eq!(tokens[3].list().unwrap()[1].list().unwrap().len(), 3);
    }

    #[test]
    fn glued_tokens_are_malformed() {
        for input in [
            &b"* 1 FETCH (X a\"b\" UID 1)\r\n"[..],
            b"* 1 FETCH (X a{1}\r\nb UID 1)\r\n",
            b"* 1 FETCH (X a(b) UID 1)\r\n",
            b"* 1 FETCH (X \"a\"\"b\")\r\n",
            b"* 1 FETCH (X \"a\"(b))\r\n",
            b"* 1 FETCH (X {1}\r\na\"b\")\r\n",
            b"* 1 FETCH (X {1}\r\na~{1}\r\nb)\r\n",
        ] {
            assert_eq!(
                tokenize(input, limits(8)),
                Err(TokenError::Malformed),
                "{}",
                String::from_utf8_lossy(input)
            );
        }
        // A list may be followed by another (multipart BODYSTRUCTURE).
        assert!(tokenize(b"* 1 FETCH (BODYSTRUCTURE ((a)(b) c))\r\n", limits(8)).is_ok());
    }

    #[test]
    fn token_limit() {
        let few = TokenLimits {
            max_depth: 8,
            max_tokens: 6,
        };
        assert!(tokenize(b"* 1 FETCH (a b)\r\n", few).is_ok());
        assert_eq!(
            tokenize(b"* 1 FETCH (a b c)\r\n", few),
            Err(TokenError::TooMany)
        );
        assert_eq!(
            tokenize(b"* 1 FETCH (((())))\r\n", few),
            Err(TokenError::TooMany)
        );
    }

    proptest! {
        #[test]
        fn never_panics(input in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = tokenize(&input, limits(16));
        }

        #[test]
        fn never_panics_structured(input in "[* ()\"\\\\{}0-9a-zA-Z\\[\\]\r\n+]{0,80}") {
            let _ = tokenize(input.as_bytes(), limits(16));
        }
    }
}
