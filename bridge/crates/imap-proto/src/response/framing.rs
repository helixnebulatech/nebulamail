// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Finds where one complete response ends: a line, plus any literals it
//! announces (`{n}` at the end of a line), plus the lines after them.
//!
//! The scan is incremental (bytes are looked at once even if they arrive in
//! many reads) and enforces the limits before the data is buffered, so a
//! hostile server cannot make us hold more than the limits allow.
//!
//! A response that breaks a limit does not end the connection: the scanner
//! switches to *skipping*. It keeps only a short prefix (to tell a tagged
//! completion from untagged data) and the end of the current line (to see
//! literal headers), lets the caller drop every byte it has looked at, and
//! reports a [`Scan::Skipped`] at the end of the response. The connection
//! stays in sync, so one huge message (a BODYSTRUCTURE with thousands of
//! parts) cannot block a whole account.

use std::ops::Range;

use crate::DecodeError;

/// Limits for incoming responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    /// Longest line, without literals.
    pub max_line: usize,
    /// Largest literal (for example a message body).
    pub max_literal: usize,
    /// Largest response: lines and literals together.
    pub max_response: usize,
    /// Most literals in one response.
    pub max_literals: usize,
    /// Deepest nesting of parentheses (BODYSTRUCTURE of nested multiparts).
    /// The parser of `imap-proto` is recursive and uses about 35 KiB of
    /// stack per level in debug builds, so the default (32) keeps it well
    /// inside a 2 MiB tokio worker stack. Deeper responses become `Unknown`.
    pub max_depth: usize,
    /// Longest `* SEARCH` or `* ESEARCH` line. A plain SEARCH lists every
    /// UID (1,000,000 UIDs of 10 digits are about 11 MB), so it gets more
    /// room than [`Self::max_line`]. The total is still bounded by the
    /// result limits of the client.
    pub max_search_line: usize,
    /// Most tokens (atoms, strings, lists) our own parsers build from one
    /// response, so a frame of `max_response` bytes cannot become a token
    /// tree many times its size.
    pub max_tokens: usize,
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self {
            // A `* SEARCH` without ESEARCH takes up to 11 bytes per UID: one
            // million UIDs of 10 digits are about 11 MB.
            max_line: 16 * 1024 * 1024,
            max_literal: 64 * 1024 * 1024,
            max_response: 80 * 1024 * 1024,
            max_literals: 4096,
            max_depth: 32,
            max_search_line: 16 * 1024 * 1024,
            max_tokens: 131_072,
        }
    }
}

/// Whether `buffer` starts with `* SEARCH` or `* ESEARCH` followed by a
/// space or the line end (any case).
fn is_search_line(buffer: &[u8]) -> bool {
    let Some(rest) = buffer.strip_prefix(b"* ") else {
        return false;
    };
    [&b"SEARCH"[..], b"ESEARCH"].iter().any(|word| {
        rest.get(..word.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(word))
            && matches!(rest.get(word.len()), Some(b' ' | b'\r' | b'\n'))
    })
}

/// Bytes of a skipped response kept to classify it (tag, `*` or `+`).
const SKIP_PREFIX: usize = 1024;

/// Bytes of the current line kept while skipping, to find a literal header
/// (`{18446744073709551615+}` plus CRLF is 25 bytes).
const SKIP_TAIL: usize = 64;

/// Incremental scanner state for the response at the start of the buffer.
#[derive(Debug, Clone, Default)]
pub(crate) struct FrameScanner {
    /// Next byte to look at.
    position: usize,
    /// Start of the current line.
    line_start: usize,
    /// End of the literal we are inside, if any.
    literal_end: Option<usize>,
    /// Literal data ranges found so far.
    literals: Vec<Range<usize>>,
    /// Set once the response broke a limit: it is being skipped.
    skip: Option<Skip>,
}

/// State of a response that broke a limit and is being dropped. Nothing is
/// buffered but the two short pieces below.
#[derive(Debug, Clone, Default)]
struct Skip {
    /// The first bytes of the response, up to the end of its first line.
    prefix: Vec<u8>,
    /// Whether `prefix` already reached the end of the first line.
    prefix_done: bool,
    /// The last bytes of the current line.
    tail: Vec<u8>,
    /// Bytes of the current literal still to drop.
    literal_left: u64,
    /// Bytes dropped so far.
    dropped: u64,
}

/// What [`FrameScanner::scan`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scan {
    /// More bytes are needed. The caller drops the first `drop` bytes of
    /// the buffer (only while skipping: they are not needed any more).
    NeedMore {
        /// Bytes to remove from the start of the buffer.
        drop: usize,
    },
    /// A complete response.
    Frame(FrameEnd),
    /// The end of a response that broke a limit.
    Skipped(Skipped),
}

/// A response that broke a limit, now fully dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skipped {
    /// Bytes of the buffer that still belong to it (the caller drops them).
    pub(crate) consumed: usize,
    /// Its first line, cut at [`SKIP_PREFIX`] bytes.
    pub(crate) prefix: Vec<u8>,
    /// Its whole size in bytes.
    pub(crate) len: u64,
}

/// A complete response at the start of the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameEnd {
    /// Length of the response in bytes.
    pub(crate) len: usize,
    /// Where the literal data is, relative to the start.
    pub(crate) literals: Vec<Range<usize>>,
}

impl FrameScanner {
    /// Looks for the end of the response that starts at `buffer[0]`.
    ///
    /// After [`Scan::Frame`] or [`Scan::Skipped`] the caller removes the
    /// response from the buffer, and after [`Scan::NeedMore`] the bytes it
    /// says; the scanner is then ready for the rest.
    pub(crate) fn scan(&mut self, buffer: &[u8], limits: &FrameLimits) -> Scan {
        if self.skip.is_none() {
            match self.scan_frame(buffer, limits) {
                Ok(Some(frame)) => return Scan::Frame(frame),
                Ok(None) => return Scan::NeedMore { drop: 0 },
                Err(_over_a_limit) => {
                    // Start over in skipping mode from the first byte: what
                    // was buffered is bounded by the limit just broken.
                    *self = Self {
                        skip: Some(Skip::default()),
                        ..Self::default()
                    };
                }
            }
        }
        let Some(skip) = self.skip.as_mut() else {
            return Scan::NeedMore { drop: 0 };
        };
        match skip.feed(buffer) {
            Some(consumed) => {
                let skip = std::mem::take(skip);
                *self = Self::default();
                Scan::Skipped(Skipped {
                    consumed,
                    prefix: skip.prefix,
                    len: skip.dropped,
                })
            }
            None => Scan::NeedMore { drop: buffer.len() },
        }
    }

    /// The scan within the limits: `Err` once the response breaks one.
    fn scan_frame(
        &mut self,
        buffer: &[u8],
        limits: &FrameLimits,
    ) -> Result<Option<FrameEnd>, DecodeError> {
        loop {
            // Only the first line of a response can be a SEARCH line.
            let max_line = if self.line_start == 0 && is_search_line(buffer) {
                limits.max_search_line
            } else {
                limits.max_line
            };
            if let Some(end) = self.literal_end {
                if buffer.len() < end {
                    return Ok(None);
                }
                self.position = end;
                self.line_start = end;
                self.literal_end = None;
            }
            let unread = buffer.get(self.position..).unwrap_or_default();
            let Some(offset) = unread.iter().position(|&byte| byte == b'\n') else {
                self.position = buffer.len();
                let line_len = buffer.len().saturating_sub(self.line_start);
                if line_len > max_line {
                    return Err(DecodeError::LineTooLong { limit: max_line });
                }
                if buffer.len() > limits.max_response {
                    return Err(DecodeError::ResponseTooLarge {
                        limit: limits.max_response,
                    });
                }
                return Ok(None);
            };
            let newline = self.position.saturating_add(offset);
            let line_end = newline.saturating_add(1);
            let line = buffer.get(self.line_start..line_end).unwrap_or_default();
            if line.len() > max_line {
                return Err(DecodeError::LineTooLong { limit: max_line });
            }
            if line_end > limits.max_response {
                return Err(DecodeError::ResponseTooLarge {
                    limit: limits.max_response,
                });
            }
            let Some(size) = literal_size(line, limits)? else {
                let literals = std::mem::take(&mut self.literals);
                *self = Self::default();
                return Ok(Some(FrameEnd {
                    len: line_end,
                    literals,
                }));
            };
            self.start_literal(line_end, size, limits)?;
        }
    }

    fn start_literal(
        &mut self,
        start: usize,
        size: usize,
        limits: &FrameLimits,
    ) -> Result<(), DecodeError> {
        let too_large = DecodeError::ResponseTooLarge {
            limit: limits.max_response,
        };
        let end = start.checked_add(size).ok_or(too_large.clone())?;
        if end > limits.max_response {
            return Err(too_large);
        }
        if self.literals.len() >= limits.max_literals {
            return Err(DecodeError::TooManyLiterals {
                limit: limits.max_literals,
            });
        }
        self.literals.push(start..end);
        self.literal_end = Some(end);
        Ok(())
    }
}

/// If `line` ends with a literal header (`{123}` or `~{123}` before CRLF),
/// returns its size.
fn literal_size(line: &[u8], limits: &FrameLimits) -> Result<Option<usize>, DecodeError> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let Some(inner) = line.strip_suffix(b"}") else {
        return Ok(None);
    };
    let Some(open) = inner.iter().rposition(|&byte| byte == b'{') else {
        return Ok(None);
    };
    let digits = inner.get(open.saturating_add(1)..).unwrap_or_default();
    // A server never sends `{n+}`, but accept it: it is still a literal.
    let digits = digits.strip_suffix(b"+").unwrap_or(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Ok(None);
    }
    // More than 20 digits cannot fit in u64: certainly over the limit.
    let size = std::str::from_utf8(digits)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(u64::MAX);
    let too_large = DecodeError::LiteralTooLarge {
        size,
        limit: limits.max_literal,
    };
    let size = usize::try_from(size).map_err(|_| too_large.clone())?;
    if size > limits.max_literal {
        return Err(too_large);
    }
    Ok(Some(size))
}

impl Skip {
    /// Drops `bytes` (all of them belong to the skipped response, up to its
    /// end). Returns how many belong to it once its end is found.
    fn feed(&mut self, bytes: &[u8]) -> Option<usize> {
        let mut position = 0;
        loop {
            let rest = bytes.get(position..).unwrap_or_default();
            if self.literal_left > 0 {
                let available = u64::try_from(rest.len()).unwrap_or(u64::MAX);
                let take = self.literal_left.min(available);
                self.literal_left = self.literal_left.saturating_sub(take);
                self.dropped = self.dropped.saturating_add(take);
                // `take` is at most `rest.len()`, so it fits.
                position = position.saturating_add(usize::try_from(take).unwrap_or(rest.len()));
                if self.literal_left > 0 {
                    return None;
                }
                continue;
            }
            let Some(offset) = rest.iter().position(|&byte| byte == b'\n') else {
                self.keep(rest);
                return None;
            };
            let line = rest.get(..=offset).unwrap_or_default();
            self.keep(line);
            position = position.saturating_add(line.len());
            match skipped_literal_size(&self.tail) {
                Some(size) => {
                    // The line goes on after the literal.
                    self.tail.clear();
                    self.literal_left = size;
                }
                None => return Some(position),
            }
        }
    }

    /// Accounts for bytes of a line: the prefix, the tail and the count.
    fn keep(&mut self, line: &[u8]) {
        self.dropped = self
            .dropped
            .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
        if !self.prefix_done {
            let newline = line.iter().position(|&byte| byte == b'\n');
            let end = newline.map_or(line.len(), |index| index.saturating_add(1));
            let room = SKIP_PREFIX.saturating_sub(self.prefix.len());
            self.prefix
                .extend_from_slice(line.get(..end.min(room)).unwrap_or_default());
            self.prefix_done = newline.is_some() || self.prefix.len() >= SKIP_PREFIX;
        }
        let start = line.len().saturating_sub(SKIP_TAIL);
        self.tail
            .extend_from_slice(line.get(start..).unwrap_or_default());
        let excess = self.tail.len().saturating_sub(SKIP_TAIL);
        self.tail.drain(..excess);
    }
}

/// The literal size announced at the end of a skipped line (whose last
/// [`SKIP_TAIL`] bytes are `tail`), without limits: it is dropped, not
/// kept. A header longer than the tail can only be a huge size.
fn skipped_literal_size(tail: &[u8]) -> Option<u64> {
    let line = tail.strip_suffix(b"\n")?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let inner = line.strip_suffix(b"}")?;
    let inner = inner.strip_suffix(b"+").unwrap_or(inner);
    let digits_start = inner
        .iter()
        .rposition(|byte| !byte.is_ascii_digit())
        .map_or(0, |index| index.saturating_add(1));
    let digits = inner.get(digits_start..).unwrap_or_default();
    if digits.is_empty() {
        return None;
    }
    match digits_start.checked_sub(1).and_then(|open| inner.get(open)) {
        Some(b'{') => Some(
            std::str::from_utf8(digits)
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(u64::MAX),
        ),
        // The digits fill the whole tail: more than 20 of them.
        None if tail.len() >= SKIP_TAIL => Some(u64::MAX),
        Some(_) | None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_all(input: &[u8], limits: &FrameLimits) -> Scan {
        FrameScanner::default().scan(input, limits)
    }

    fn frame_len(scan: Scan) -> usize {
        match scan {
            Scan::Frame(frame) => frame.len,
            other => panic!("not a frame: {other:?}"),
        }
    }

    fn skipped(scan: Scan) -> Skipped {
        match scan {
            Scan::Skipped(skipped) => skipped,
            other => panic!("not skipped: {other:?}"),
        }
    }

    /// Feeds `input` in pieces of `chunk` bytes, dropping what the scanner
    /// says, like the decoder. Returns what came out and the largest buffer.
    fn scan_chunks(input: &[u8], chunk: usize, limits: &FrameLimits) -> (Vec<Scan>, usize) {
        let mut scanner = FrameScanner::default();
        let mut buffer = Vec::new();
        let mut found = Vec::new();
        let mut largest = 0;
        for piece in input.chunks(chunk) {
            buffer.extend_from_slice(piece);
            largest = largest.max(buffer.len());
            loop {
                match scanner.scan(&buffer, limits) {
                    Scan::NeedMore { drop } => {
                        buffer.drain(..drop);
                        break;
                    }
                    Scan::Frame(frame) => {
                        buffer.drain(..frame.len);
                        found.push(Scan::Frame(frame));
                    }
                    Scan::Skipped(skipped) => {
                        buffer.drain(..skipped.consumed);
                        found.push(Scan::Skipped(skipped));
                    }
                }
            }
        }
        (found, largest)
    }

    #[test]
    fn simple_line() {
        let limits = FrameLimits::default();
        assert_eq!(
            frame_len(scan_all(b"* OK hi\r\n* 1 EXISTS\r\n", &limits)),
            9
        );
        assert_eq!(scan_all(b"* OK hi", &limits), Scan::NeedMore { drop: 0 });
    }

    #[test]
    fn literal_spans_lines() {
        let limits = FrameLimits::default();
        let input = b"* 1 FETCH (BODY[] {5}\r\na\r\nb\n UID 1)\r\nrest";
        let Scan::Frame(frame) = scan_all(input, &limits) else {
            panic!()
        };
        assert_eq!(&input[..frame.len], &input[..input.len() - 4]);
        assert_eq!(frame.literals, vec![23..28]);
        assert_eq!(&input[23..28], b"a\r\nb\n");
    }

    #[test]
    fn incremental_scan() {
        let limits = FrameLimits::default();
        let input = b"* 1 FETCH (BODY[] {3}\r\nabc)\r\n";
        let mut scanner = FrameScanner::default();
        for end in 1..input.len() {
            assert_eq!(
                scanner.scan(&input[..end], &limits),
                Scan::NeedMore { drop: 0 },
                "at {end}"
            );
        }
        assert_eq!(frame_len(scanner.scan(input, &limits)), input.len());
    }

    #[test]
    fn limits_skip_the_response() {
        let limits = FrameLimits {
            max_line: 10,
            max_literal: 4,
            max_response: 30,
            max_literals: 1,
            max_depth: 8,
            max_search_line: 20,
            max_tokens: 100,
        };
        // A long line: dropped while it arrives, then skipped at its end.
        assert_eq!(
            scan_all(b"* 0123456789ABC", &limits),
            Scan::NeedMore { drop: 15 }
        );
        let found = skipped(scan_all(b"* 0123456789ABC\r\nA1 OK\r\n", &limits));
        assert_eq!(found.consumed, 17);
        assert_eq!(found.prefix, b"* 0123456789ABC\r\n");
        assert_eq!(found.len, 17);
        // SEARCH lines have their own limit.
        assert_eq!(
            scan_all(b"* SEARCH 1 2 3 4", &limits),
            Scan::NeedMore { drop: 0 }
        );
        assert_eq!(frame_len(scan_all(b"* search 1 2 3 4\r\n", &limits)), 18);
        let found = skipped(scan_all(b"* ESEARCH UID ALL 1:200000\r\n", &limits));
        assert_eq!(found.consumed, 28);
        let found = skipped(scan_all(b"* SEARCHX 1 2 3 4\r\n", &limits));
        assert_eq!(found.consumed, 19);
        // A literal over the limit is dropped whole, `\n` inside included.
        let found = skipped(scan_all(b"* {5}\r\na\nb\nc)\r\nA1 OK\r\n", &limits));
        assert_eq!(found.consumed, 15);
        // A size that does not fit in u64: dropped until the data ends.
        let mut scanner = FrameScanner::default();
        let huge = b"* {99999999999999999999999}\r\n";
        assert_eq!(
            scanner.scan(huge, &limits),
            Scan::NeedMore { drop: huge.len() }
        );
        assert_eq!(
            scanner.scan(&[b'\n'; 1000], &limits),
            Scan::NeedMore { drop: 1000 }
        );
        // Too many literals.
        let found = skipped(scan_all(b"* {1}\r\na {1}\r\nb)\r\nA1 OK\r\n", &limits));
        assert_eq!(found.consumed, 18);
        // A whole response over the limit.
        let limits = FrameLimits {
            max_line: 100,
            max_literal: 100,
            max_response: 20,
            ..limits
        };
        let mut input = b"* {50}\r\n".to_vec();
        input.extend_from_slice(&[b'x'; 50]);
        input.extend_from_slice(b")\r\nA1 OK\r\n");
        let found = skipped(scan_all(&input, &limits));
        assert_eq!(found.consumed, input.len() - 7);
    }

    #[test]
    fn skipping_keeps_the_buffer_small_and_resyncs() {
        let limits = FrameLimits {
            max_line: 1024,
            ..FrameLimits::default()
        };
        let mut input = b"* 1 FETCH (UID 7 BODYSTRUCTURE (".to_vec();
        for _ in 0..2000 {
            input.extend_from_slice(b"(\"text\" \"plain\" NIL NIL NIL \"7bit\" 0 0)");
        }
        input.extend_from_slice(b" \"mixed\"))\r\nA1 OK done\r\n");
        assert!(input.len() > 64 * 1024);
        let (found, largest) = scan_chunks(&input, 100, &limits);
        assert!(largest <= 1024 + 100 + 100, "buffer grew to {largest}");
        let [Scan::Skipped(first), Scan::Frame(second)] = found.as_slice() else {
            panic!("{found:?}")
        };
        assert!(first.prefix.starts_with(b"* 1 FETCH"));
        assert_eq!(first.prefix.len(), SKIP_PREFIX);
        assert_eq!(second.len, b"A1 OK done\r\n".len());
    }

    #[test]
    fn literal_after_a_long_line_is_skipped_whole() {
        let limits = FrameLimits {
            max_line: 16,
            ..FrameLimits::default()
        };
        let mut input = b"* 1 FETCH (".to_vec();
        input.extend_from_slice(&[b'a'; 100]);
        input.extend_from_slice(b" {5}\r\nx\ny\nz)\r\nA1 OK\r\n");
        for chunk in [1, 3, 7, 1000] {
            let (found, _) = scan_chunks(&input, chunk, &limits);
            let [Scan::Skipped(first), Scan::Frame(second)] = found.as_slice() else {
                panic!("{chunk}: {found:?}")
            };
            assert_eq!(first.len, u64::try_from(input.len() - 7).unwrap());
            assert_eq!(second.len, 7);
        }
    }

    #[test]
    fn literal_headers_while_skipping() {
        assert_eq!(skipped_literal_size(b"x {5}\r\n"), Some(5));
        assert_eq!(skipped_literal_size(b"x ~{5}\r\n"), Some(5));
        assert_eq!(skipped_literal_size(b"x {5+}\n"), Some(5));
        assert_eq!(skipped_literal_size(b"x 5}\r\n"), None);
        assert_eq!(skipped_literal_size(b"x {}\r\n"), None);
        assert_eq!(skipped_literal_size(b"x {5}"), None);
        assert_eq!(
            skipped_literal_size(b"x {99999999999999999999}\r\n"),
            Some(u64::MAX)
        );
        let mut tail = vec![b'9'; SKIP_TAIL - 3];
        tail.extend_from_slice(b"}\r\n");
        assert_eq!(skipped_literal_size(&tail), Some(u64::MAX));
    }

    proptest::proptest! {
        /// Cutting the input at random gives the same result as one read.
        #[test]
        fn chunking_does_not_change_the_result(
            sizes in proptest::collection::vec(0usize..200, 1..6),
            chunk in 1usize..64,
        ) {
            let limits = FrameLimits {
                max_line: 64,
                max_literal: 32,
                max_literals: 2,
                ..FrameLimits::default()
            };
            let mut input = Vec::new();
            for (index, size) in sizes.iter().enumerate() {
                input.extend_from_slice(format!("* {index} FETCH (").as_bytes());
                if index % 2 == 0 {
                    input.extend(std::iter::repeat_n(b'a', *size));
                } else {
                    input.extend_from_slice(format!("{{{size}}}\r\n").as_bytes());
                    input.extend(std::iter::repeat_n(b'\n', *size));
                }
                input.extend_from_slice(b")\r\n");
            }
            let describe = |found: Vec<Scan>| -> Vec<(bool, u64)> {
                found
                    .into_iter()
                    .map(|scan| match scan {
                        Scan::Frame(frame) => (true, u64::try_from(frame.len).unwrap()),
                        Scan::Skipped(skipped) => (false, skipped.len),
                        Scan::NeedMore { .. } => unreachable!(),
                    })
                    .collect()
            };
            let (whole, _) = scan_chunks(&input, input.len(), &limits);
            let (pieces, _) = scan_chunks(&input, chunk, &limits);
            let whole = describe(whole);
            proptest::prop_assert_eq!(whole.len(), sizes.len());
            let total: u64 = whole.iter().map(|(_, len)| len).sum();
            proptest::prop_assert_eq!(total, u64::try_from(input.len()).unwrap());
            proptest::prop_assert_eq!(whole, describe(pieces));
        }
    }
}
