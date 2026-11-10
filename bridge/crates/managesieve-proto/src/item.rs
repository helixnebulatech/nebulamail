// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The pieces of a line (RFC 5804 §4: atoms, quoted strings, literals,
//! parenthesized lists), the scanner that finds one complete line in a
//! buffer, and the encoder.

use std::fmt;

use crate::{DecodeError, Limits};

/// Longest quoted string we write (RFC 5804 §4: "quoted strings are
/// limited to 1024 octets"). Longer values go as literals.
const MAX_QUOTED: usize = 1024;

/// An atom: one or more printable ASCII characters other than space,
/// `(`, `)`, `"`, `{` and `\` (`OK`, `ACTIVE`, `QUOTA/MAXSIZE`, `1024`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Atom(String);

impl Atom {
    /// Checks an atom.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        (!text.is_empty() && text.bytes().all(is_atom_byte)).then(|| Self(text.to_owned()))
    }

    /// The atom.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-insensitive comparison (RFC 5804 keywords are).
    #[must_use]
    pub fn is(&self, keyword: &str) -> bool {
        self.0.eq_ignore_ascii_case(keyword)
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_atom_byte(byte: u8) -> bool {
    matches!(byte, 0x21..=0x7e) && !matches!(byte, b'(' | b')' | b'"' | b'{' | b'\\')
}

/// One piece of a line.
#[derive(Clone, PartialEq, Eq)]
pub enum Item {
    /// An atom.
    Atom(Atom),
    /// A quoted string, unescaped. Its bytes are not checked for UTF-8
    /// here: whoever reads it decides.
    Quoted(Vec<u8>),
    /// A literal (`{n}` + CRLF + n bytes).
    Literal(Vec<u8>),
    /// A parenthesized list (response codes).
    List(Vec<Item>),
}

impl fmt::Debug for Item {
    // Only sizes: a literal may be a whole script or a SASL challenge.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Atom(atom) => write!(f, "Atom({atom})"),
            Self::Quoted(bytes) => write!(f, "Quoted({} bytes)", bytes.len()),
            Self::Literal(bytes) => write!(f, "Literal({} bytes)", bytes.len()),
            Self::List(items) => f.debug_tuple("List").field(items).finish(),
        }
    }
}

impl Item {
    /// The bytes of a quoted string or literal.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Quoted(bytes) | Self::Literal(bytes) => Some(bytes),
            Self::Atom(_) | Self::List(_) => None,
        }
    }

    /// The text of a quoted string or literal, if it is UTF-8.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        self.as_bytes()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }

    /// The atom, if it is one.
    #[must_use]
    pub fn as_atom(&self) -> Option<&Atom> {
        match self {
            Self::Atom(atom) => Some(atom),
            _ => None,
        }
    }

    /// Takes the bytes of a quoted string or literal.
    pub(crate) fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Quoted(bytes) | Self::Literal(bytes) => Some(bytes),
            Self::Atom(_) | Self::List(_) => None,
        }
    }
}

/// Who wrote the line: the literal forms each side may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// A server: `{n}` only (RFC 5804 `literal-s2c`).
    Server,
    /// A client: `{n+}` (and `{n}`, which older clients send).
    Client,
}

/// The result of looking for one line.
#[derive(Debug)]
pub(crate) enum Scan {
    /// More bytes needed.
    Incomplete,
    /// A complete line and how many bytes it took.
    Line { items: Vec<Item>, consumed: usize },
}

/// Finds the first complete line of `buf`.
pub(crate) fn scan_line(buf: &[u8], limits: &Limits, side: Side) -> Result<Scan, DecodeError> {
    let mut scanner = Scanner {
        buf,
        pos: 0,
        limits,
        side,
        literal_bytes: 0,
        items: 0,
    };
    match scanner.line()? {
        Some(items) => Ok(Scan::Line {
            items,
            consumed: scanner.pos,
        }),
        None => Ok(Scan::Incomplete),
    }
}

struct Scanner<'a> {
    buf: &'a [u8],
    pos: usize,
    limits: &'a Limits,
    side: Side,
    /// Literal payload bytes passed so far (not part of the line length).
    literal_bytes: usize,
    items: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    /// Moves past `n` bytes of the line itself (not a literal payload).
    fn advance(&mut self, n: usize) -> Result<(), DecodeError> {
        self.pos = self.pos.saturating_add(n);
        if self.pos.saturating_sub(self.literal_bytes) > self.limits.max_line {
            return Err(DecodeError::LineTooLong);
        }
        Ok(())
    }

    fn skip_spaces(&mut self) -> Result<(), DecodeError> {
        while self.peek() == Some(b' ') {
            self.advance(1)?;
        }
        Ok(())
    }

    fn count_item(&mut self) -> Result<(), DecodeError> {
        self.items = self.items.saturating_add(1);
        if self.items > self.limits.max_items {
            return Err(DecodeError::TooManyItems);
        }
        Ok(())
    }

    fn line(&mut self) -> Result<Option<Vec<Item>>, DecodeError> {
        let mut items = Vec::new();
        loop {
            self.skip_spaces()?;
            match self.peek() {
                None => return Ok(None),
                Some(b'\r') => {
                    let Some(next) = self.buf.get(self.pos.saturating_add(1)).copied() else {
                        return Ok(None);
                    };
                    if next != b'\n' {
                        return Err(DecodeError::Malformed("CR without LF"));
                    }
                    self.advance(2)?;
                    return Ok(Some(items));
                }
                Some(b'\n') => return Err(DecodeError::Malformed("LF without CR")),
                Some(b')') => return Err(DecodeError::Malformed("unbalanced parenthesis")),
                Some(_) => match self.item(0)? {
                    Some(item) => items.push(item),
                    None => return Ok(None),
                },
            }
        }
    }

    fn item(&mut self, depth: usize) -> Result<Option<Item>, DecodeError> {
        self.count_item()?;
        match self.peek() {
            None => Ok(None),
            Some(b'"') => self.quoted(),
            Some(b'{') => self.literal(),
            Some(b'(') => self.list(depth),
            Some(byte) if is_atom_byte(byte) => self.atom(),
            Some(_) => Err(DecodeError::Malformed("unexpected byte")),
        }
    }

    fn atom(&mut self) -> Result<Option<Item>, DecodeError> {
        let start = self.pos;
        loop {
            match self.peek() {
                None => return Ok(None),
                Some(byte) if is_atom_byte(byte) => self.advance(1)?,
                Some(b' ' | b'\r' | b'(' | b')' | b'"' | b'{') => break,
                Some(_) => return Err(DecodeError::Malformed("bad atom")),
            }
        }
        let text = self
            .buf
            .get(start..self.pos)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(Atom::new)
            .ok_or(DecodeError::Malformed("bad atom"))?;
        Ok(Some(Item::Atom(text)))
    }

    fn quoted(&mut self) -> Result<Option<Item>, DecodeError> {
        self.advance(1)?;
        let mut value = Vec::new();
        loop {
            match self.peek() {
                None => return Ok(None),
                Some(b'"') => {
                    self.advance(1)?;
                    return Ok(Some(Item::Quoted(value)));
                }
                Some(b'\\') => {
                    self.advance(1)?;
                    match self.peek() {
                        None => return Ok(None),
                        Some(byte @ (b'"' | b'\\')) => {
                            value.push(byte);
                            self.advance(1)?;
                        }
                        Some(_) => return Err(DecodeError::Malformed("bad escape")),
                    }
                }
                Some(b'\r' | b'\n' | 0) => {
                    return Err(DecodeError::Malformed(
                        "control character in a quoted string",
                    ));
                }
                Some(byte) => {
                    value.push(byte);
                    self.advance(1)?;
                }
            }
        }
    }

    fn literal(&mut self) -> Result<Option<Item>, DecodeError> {
        self.advance(1)?;
        let mut size: usize = 0;
        let mut digits = 0_u8;
        loop {
            match self.peek() {
                None => return Ok(None),
                Some(byte @ b'0'..=b'9') => {
                    digits = digits.saturating_add(1);
                    size = size
                        .checked_mul(10)
                        .and_then(|n| n.checked_add(usize::from(byte.saturating_sub(b'0'))))
                        .ok_or(DecodeError::LiteralTooLong)?;
                    if size > self.limits.max_literal {
                        return Err(DecodeError::LiteralTooLong);
                    }
                    self.advance(1)?;
                }
                Some(_) => break,
            }
        }
        if digits == 0 {
            return Err(DecodeError::Malformed("literal without a size"));
        }
        if self.peek() == Some(b'+') {
            if self.side == Side::Server {
                return Err(DecodeError::Malformed(
                    "non-synchronizing literal from a server",
                ));
            }
            self.advance(1)?;
        }
        for expected in *b"}\r\n" {
            match self.peek() {
                None => return Ok(None),
                Some(byte) if byte == expected => self.advance(1)?,
                Some(_) => return Err(DecodeError::Malformed("bad literal header")),
            }
        }
        self.literal_bytes = self
            .literal_bytes
            .checked_add(size)
            .ok_or(DecodeError::LiteralTooLong)?;
        if self.literal_bytes > self.limits.max_line_literals {
            return Err(DecodeError::LiteralTooLong);
        }
        let end = self
            .pos
            .checked_add(size)
            .ok_or(DecodeError::LiteralTooLong)?;
        let Some(bytes) = self.buf.get(self.pos..end) else {
            return Ok(None);
        };
        let value = bytes.to_vec();
        self.pos = end;
        Ok(Some(Item::Literal(value)))
    }

    fn list(&mut self, depth: usize) -> Result<Option<Item>, DecodeError> {
        let depth = depth.saturating_add(1);
        if depth > self.limits.max_depth {
            return Err(DecodeError::TooManyItems);
        }
        self.advance(1)?;
        let mut items = Vec::new();
        loop {
            self.skip_spaces()?;
            match self.peek() {
                None => return Ok(None),
                Some(b')') => {
                    self.advance(1)?;
                    return Ok(Some(Item::List(items)));
                }
                Some(b'\r' | b'\n') => return Err(DecodeError::Malformed("unclosed list")),
                Some(_) => match self.item(depth)? {
                    Some(item) => items.push(item),
                    None => return Ok(None),
                },
            }
        }
    }
}

/// Where a [`LineFramer`] is inside a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FrameState {
    /// Between items, or in an atom or a list.
    #[default]
    Normal,
    /// In a quoted string.
    Quoted,
    /// After `\` in a quoted string.
    QuotedEscape,
    /// After `{`: the size so far and how many digits it had.
    LiteralSize { size: usize, digits: u8 },
    /// After `+` (clients only): `}` must follow.
    LiteralPlus { size: usize },
    /// After `}`; `lf` once the `\r` came.
    LiteralClose { size: usize, lf: bool },
    /// In the payload of a literal: bytes still to skip.
    LiteralBody { left: usize },
    /// After `\r` outside a string: `\n` must follow.
    Cr,
}

/// Finds where a line ends without parsing it, resuming where it stopped
/// on the last call: each received byte is looked at once (literal
/// payloads are skipped whole), and nothing is copied. [`scan_line`] then
/// parses the complete line, once.
///
/// It applies the size limits as the bytes come (line length, literal
/// sizes), so a peer can't make the buffer grow past them while it waits
/// for the end of a line. Other grammar errors show up when the line is
/// parsed.
#[derive(Debug, Clone, Default)]
pub(crate) struct LineFramer {
    /// Bytes of the buffer already looked at.
    pos: usize,
    /// Literal payload bytes in `..pos` (not part of the line length).
    literal_bytes: usize,
    state: FrameState,
}

impl LineFramer {
    /// Goes on through `buf` (the same buffer, maybe longer, as on the last
    /// call): the length of the first line once it is complete.
    pub(crate) fn frame(
        &mut self,
        buf: &[u8],
        limits: &Limits,
        side: Side,
    ) -> Result<Option<usize>, DecodeError> {
        while let Some(&byte) = buf.get(self.pos) {
            if let FrameState::LiteralBody { left } = self.state {
                let skip = left.min(buf.len().saturating_sub(self.pos));
                self.pos = self.pos.saturating_add(skip);
                let left = left.saturating_sub(skip);
                self.state = if left == 0 {
                    FrameState::Normal
                } else {
                    FrameState::LiteralBody { left }
                };
                continue;
            }
            self.pos = self.pos.saturating_add(1);
            if self.pos.saturating_sub(self.literal_bytes) > limits.max_line {
                return Err(DecodeError::LineTooLong);
            }
            self.state = match (self.state, byte) {
                (FrameState::Normal, b'"') => FrameState::Quoted,
                (FrameState::Normal, b'{') => FrameState::LiteralSize { size: 0, digits: 0 },
                (FrameState::Normal, b'\r') => FrameState::Cr,
                (FrameState::Normal, b'\n') => {
                    return Err(DecodeError::Malformed("LF without CR"));
                }
                (FrameState::Normal, _) | (FrameState::Quoted, b'"') => FrameState::Normal,
                (FrameState::Quoted, b'\\') => FrameState::QuotedEscape,
                (FrameState::Quoted, b'\r' | b'\n' | 0) => {
                    return Err(DecodeError::Malformed(
                        "control character in a quoted string",
                    ));
                }
                (FrameState::Quoted, _) | (FrameState::QuotedEscape, b'"' | b'\\') => {
                    FrameState::Quoted
                }
                (FrameState::QuotedEscape, _) => {
                    return Err(DecodeError::Malformed("bad escape"));
                }
                (FrameState::LiteralSize { size, digits }, b'0'..=b'9') => {
                    let longer = size
                        .checked_mul(10)
                        .and_then(|n| n.checked_add(usize::from(byte.saturating_sub(b'0'))))
                        .ok_or(DecodeError::LiteralTooLong)?;
                    if longer > limits.max_literal {
                        return Err(DecodeError::LiteralTooLong);
                    }
                    FrameState::LiteralSize {
                        size: longer,
                        digits: digits.saturating_add(1),
                    }
                }
                (FrameState::LiteralSize { digits: 0, .. }, _) => {
                    return Err(DecodeError::Malformed("literal without a size"));
                }
                (FrameState::LiteralSize { size, .. }, b'+') => {
                    if side == Side::Server {
                        return Err(DecodeError::Malformed(
                            "non-synchronizing literal from a server",
                        ));
                    }
                    FrameState::LiteralPlus { size }
                }
                (FrameState::LiteralSize { size, .. } | FrameState::LiteralPlus { size }, b'}') => {
                    FrameState::LiteralClose { size, lf: false }
                }
                (FrameState::LiteralClose { size, lf: false }, b'\r') => {
                    FrameState::LiteralClose { size, lf: true }
                }
                (FrameState::LiteralClose { size, lf: true }, b'\n') => {
                    self.literal_bytes = self
                        .literal_bytes
                        .checked_add(size)
                        .ok_or(DecodeError::LiteralTooLong)?;
                    if self.literal_bytes > limits.max_line_literals {
                        return Err(DecodeError::LiteralTooLong);
                    }
                    if size == 0 {
                        FrameState::Normal
                    } else {
                        FrameState::LiteralBody { left: size }
                    }
                }
                (
                    FrameState::LiteralSize { .. }
                    | FrameState::LiteralPlus { .. }
                    | FrameState::LiteralClose { .. },
                    _,
                ) => return Err(DecodeError::Malformed("bad literal header")),
                (FrameState::Cr, b'\n') => return Ok(Some(self.pos)),
                (FrameState::Cr, _) => return Err(DecodeError::Malformed("CR without LF")),
                // Handled before the match.
                (FrameState::LiteralBody { .. }, _) => self.state,
            };
        }
        Ok(None)
    }
}

/// Where a decoder is: framing a line, or broken by an error it keeps
/// returning (the stream is out of sync after one).
#[derive(Debug, Clone)]
pub(crate) enum Framing {
    /// Looking for the end of the next line.
    Ok(LineFramer),
    /// A line broke the grammar or the limits.
    Failed(DecodeError),
}

impl Default for Framing {
    fn default() -> Self {
        Self::Ok(LineFramer::default())
    }
}

impl Framing {
    /// The items of the next complete line of `buf` and its length, which
    /// the caller removes from `buf`; `None` if more bytes are needed.
    /// Each line is parsed (and its literals copied) once.
    pub(crate) fn next_line(
        &mut self,
        buf: &[u8],
        limits: &Limits,
        side: Side,
    ) -> Result<Option<(Vec<Item>, usize)>, DecodeError> {
        let result = match self {
            Self::Failed(error) => return Err(error.clone()),
            Self::Ok(framer) => framer.frame(buf, limits, side).and_then(|end| {
                let Some(end) = end else {
                    return Ok(None);
                };
                #[cfg(test)]
                SCANS.with(|scans| scans.set(scans.get().saturating_add(1)));
                match scan_line(buf.get(..end).unwrap_or_default(), limits, side)? {
                    Scan::Line { items, consumed } if consumed == end => Ok(Some((items, end))),
                    _ => Err(DecodeError::Malformed("line framing mismatch")),
                }
            }),
        };
        match &result {
            Err(error) => *self = Self::Failed(error.clone()),
            Ok(Some(_)) => *self = Self::default(),
            Ok(None) => {}
        }
        result
    }
}

#[cfg(test)]
thread_local! {
    /// Calls to [`scan_line`] from [`Framing::next_line`] on this thread
    /// (each test runs on its own thread).
    static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times [`Framing::next_line`] has called [`scan_line`] on this
/// thread: one per complete line, none while waiting for more bytes.
#[cfg(test)]
pub(crate) fn scans() -> usize {
    SCANS.with(std::cell::Cell::get)
}

/// Whether `bytes` can go in a quoted string.
pub(crate) fn quotable(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_QUOTED
        && !bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0))
        && std::str::from_utf8(bytes).is_ok()
}

/// Writes a string: quoted if it can be, else a literal.
pub(crate) fn encode_string(out: &mut Vec<u8>, bytes: &[u8], side: Side) {
    if quotable(bytes) {
        write_quoted(out, bytes);
    } else {
        encode_literal(out, bytes, side);
    }
}

fn write_quoted(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    for &byte in bytes {
        if matches!(byte, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b'"');
}

/// Writes a literal: `{n}` from a server, `{n+}` from a client.
pub(crate) fn encode_literal(out: &mut Vec<u8>, bytes: &[u8], side: Side) {
    out.push(b'{');
    out.extend_from_slice(bytes.len().to_string().as_bytes());
    if side == Side::Client {
        out.push(b'+');
    }
    out.extend_from_slice(b"}\r\n");
    out.extend_from_slice(bytes);
}

/// Writes items separated by spaces.
pub(crate) fn encode_items(out: &mut Vec<u8>, items: &[Item], side: Side) {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        encode_item(out, item, side);
    }
}

fn encode_item(out: &mut Vec<u8>, item: &Item, side: Side) {
    match item {
        Item::Atom(atom) => out.extend_from_slice(atom.as_str().as_bytes()),
        // As it came: any bytes but CR, LF and NUL fit in a quoted string.
        Item::Quoted(bytes) if !bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0)) => {
            write_quoted(out, bytes);
        }
        Item::Quoted(bytes) | Item::Literal(bytes) => encode_literal(out, bytes, side),
        Item::List(items) => {
            out.push(b'(');
            encode_items(out, items, side);
            out.push(b')');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &[u8]) -> Result<Scan, DecodeError> {
        scan_line(text, &Limits::default(), Side::Server)
    }

    fn items(text: &[u8]) -> Vec<Item> {
        match line(text) {
            Ok(Scan::Line { items, consumed }) => {
                assert_eq!(consumed, text.len());
                items
            }
            other => panic!("{other:?}"),
        }
    }

    fn atom(text: &str) -> Item {
        Item::Atom(Atom::new(text).unwrap())
    }

    #[test]
    fn pieces() {
        assert_eq!(
            items(b"OK (QUOTA/MAXSIZE) \"a \\\"b\\\\\"\r\n"),
            vec![
                atom("OK"),
                Item::List(vec![atom("QUOTA/MAXSIZE")]),
                Item::Quoted(b"a \"b\\".to_vec())
            ]
        );
        assert_eq!(
            items(b"{5}\r\nab\r\nc ACTIVE\r\n"),
            vec![Item::Literal(b"ab\r\nc".to_vec()), atom("ACTIVE")]
        );
        assert_eq!(items(b"\"\"\r\n"), vec![Item::Quoted(Vec::new())]);
        assert_eq!(
            items(b"NO (TAG {3}\r\nx\"y) \"t\"\r\n"),
            vec![
                atom("NO"),
                Item::List(vec![atom("TAG"), Item::Literal(b"x\"y".to_vec())]),
                Item::Quoted(b"t".to_vec())
            ]
        );
    }

    #[test]
    fn incomplete_lines() {
        assert!(matches!(line(b"OK"), Ok(Scan::Incomplete)));
        assert!(matches!(line(b"OK\r"), Ok(Scan::Incomplete)));
        assert!(matches!(line(b"{10}\r\nabc"), Ok(Scan::Incomplete)));
        assert!(matches!(line(b"\"abc"), Ok(Scan::Incomplete)));
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            &b"OK\n"[..],
            b"OK\rX",
            b"\"a\rb\"\r\n",
            b"\"a\\nb\"\r\n",
            b"{5+}\r\nhello\r\n",
            b"{}\r\n",
            b"{5\r\n",
            b"(a\r\n",
            b")\r\n",
            b"\x01\r\n",
            b"caf\xc3\xa9\r\n",
        ] {
            assert!(line(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        // Clients may use `{n+}`.
        assert!(matches!(
            scan_line(b"{1+}\r\nx\r\n", &Limits::default(), Side::Client),
            Ok(Scan::Line { .. })
        ));
    }

    #[test]
    fn limits() {
        let limits = Limits {
            max_line: 16,
            max_literal: 8,
            max_line_literals: 12,
            max_items: 4,
            max_depth: 2,
        };
        let scan = |text: &[u8]| scan_line(text, &limits, Side::Server);
        assert_eq!(scan(&[b' '; 32]).unwrap_err(), DecodeError::LineTooLong);
        assert_eq!(scan(b"{9}\r\n").unwrap_err(), DecodeError::LiteralTooLong);
        assert_eq!(
            scan(b"{99999999999999999999999}\r\n").unwrap_err(),
            DecodeError::LiteralTooLong
        );
        assert_eq!(
            scan(b"{8}\r\n12345678 {8}\r\n").unwrap_err(),
            DecodeError::LiteralTooLong
        );
        assert_eq!(
            scan(b"a b c d e\r\n").unwrap_err(),
            DecodeError::TooManyItems
        );
        assert_eq!(scan(b"(((a)))\r\n").unwrap_err(), DecodeError::TooManyItems);
        // Literal payloads don't count as line length.
        assert!(matches!(
            scan(b"{8}\r\n12345678 x\r\n"),
            Ok(Scan::Line { .. })
        ));
    }

    /// Frames `chunks` fed one after the other: the line ends found.
    fn frame_chunks(chunks: &[&[u8]], limits: &Limits) -> Result<Vec<usize>, DecodeError> {
        let mut buf = Vec::new();
        let mut framing = Framing::default();
        let mut ends = Vec::new();
        for chunk in chunks {
            buf.extend_from_slice(chunk);
            while let Some((_, end)) = framing.next_line(&buf, limits, Side::Server)? {
                buf.drain(..end);
                ends.push(end);
            }
        }
        Ok(ends)
    }

    #[test]
    fn framer_resumes_across_chunks() {
        let limits = Limits::default();
        let ends = frame_chunks(
            &[
                b"\"a\\",
                b"\"b\" {3",
                b"}\r",
                b"\nx\r",
                b"\ny\r\n",
                b"OK\r\nNO\r\n",
            ],
            &limits,
        )
        .unwrap();
        assert_eq!(ends, vec![18, 4, 4]);
    }

    #[test]
    fn framer_errors() {
        let limits = Limits::default();
        let error = |chunks: &[&[u8]]| frame_chunks(chunks, &limits).unwrap_err();
        // An escape that is not `"` or `\` (here the CR) is an error at
        // once: it does not eat the end of the line.
        assert_eq!(
            error(&[b"\"a\\", b"\r\n"]),
            DecodeError::Malformed("bad escape")
        );
        assert_eq!(error(&[b"OK\n"]), DecodeError::Malformed("LF without CR"));
        assert_eq!(error(&[b"OK\rx"]), DecodeError::Malformed("CR without LF"));
        assert_eq!(
            error(&[b"{5+}\r\nhello\r\n"]),
            DecodeError::Malformed("non-synchronizing literal from a server")
        );
        assert_eq!(
            error(&[b"{}"]),
            DecodeError::Malformed("literal without a size")
        );
        assert_eq!(
            error(&[b"{5\r"]),
            DecodeError::Malformed("bad literal header")
        );
        // A grammar error the framer does not look for comes with the line.
        assert_eq!(
            error(&[b"\x01", b"\r\n"]),
            DecodeError::Malformed("unexpected byte")
        );
    }

    #[test]
    fn framer_applies_the_limits_before_the_end_of_the_line() {
        let limits = Limits {
            max_line: 16,
            max_literal: 8,
            max_line_literals: 12,
            max_items: 4,
            max_depth: 2,
        };
        let frame = |bytes: &[u8]| LineFramer::default().frame(bytes, &limits, Side::Server);
        assert_eq!(frame(&[b'a'; 17]).unwrap_err(), DecodeError::LineTooLong);
        assert_eq!(frame(&[b'a'; 16]), Ok(None));
        assert_eq!(frame(b"{9}").unwrap_err(), DecodeError::LiteralTooLong);
        assert_eq!(
            frame(b"{8}\r\n12345678 {8}\r\n").unwrap_err(),
            DecodeError::LiteralTooLong
        );
        // Literal payloads don't count as line length.
        assert_eq!(frame(b"{8}\r\n12345678 {4}\r\n1234 x\r\n"), Ok(Some(27)));
    }

    #[test]
    fn a_broken_decoder_stays_broken() {
        let limits = Limits::default();
        let mut framing = Framing::default();
        assert_eq!(
            framing.next_line(b"OK\n", &limits, Side::Server),
            Err(DecodeError::Malformed("LF without CR"))
        );
        // Even with a good line after it.
        assert_eq!(
            framing.next_line(b"OK\r\n", &limits, Side::Server),
            Err(DecodeError::Malformed("LF without CR"))
        );
    }

    #[test]
    fn encoding() {
        let mut out = Vec::new();
        encode_items(
            &mut out,
            &[
                atom("OK"),
                Item::Quoted(b"a\"b".to_vec()),
                Item::Quoted(b"x\r\ny".to_vec()),
                Item::List(vec![atom("TAG"), Item::Literal(b"z".to_vec())]),
            ],
            Side::Server,
        );
        assert_eq!(out, b"OK \"a\\\"b\" {4}\r\nx\r\ny (TAG {1}\r\nz)");
        let mut out = Vec::new();
        encode_string(&mut out, &[b'a'; 1025], Side::Client);
        assert!(out.starts_with(b"{1025+}\r\n"));
    }
}
