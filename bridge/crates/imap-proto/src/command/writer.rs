// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Safe encoding of command arguments.
//!
//! Every byte that is not part of a literal goes through this writer, which
//! never lets CR, LF or NUL into the command line: strings that contain them
//! become literals, and NUL is refused.

use zeroize::Zeroizing;

use crate::EncodeError;
use crate::types::{Capabilities, Capability, NameEncoding, Tag, is_atom_char};

/// Which literal syntax we may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiteralMode {
    /// `{n}`: wait for the server's `+` before sending the data.
    Synchronizing,
    /// `{n+}` for any size (`LITERAL+`, RFC 7888).
    NonSyncAny,
    /// `{n+}` up to 4096 bytes (`LITERAL-`, RFC 7888).
    NonSyncSmall,
}

/// Largest non-synchronizing literal with `LITERAL-`.
const LITERAL_MINUS_MAX: usize = 4096;

/// Longer strings go as literals even if they could be quoted, to keep
/// command lines short.
const MAX_QUOTED_LEN: usize = 1024;

/// What the encoder may use on this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeOptions {
    /// Literal syntax.
    pub literals: LiteralMode,
    /// Mailbox name encoding (UTF-8 after `ENABLE UTF8=ACCEPT`).
    pub names: NameEncoding,
}

impl EncodeOptions {
    /// Plain `IMAP4rev1`: synchronizing literals and modified UTF-7.
    #[must_use]
    pub fn basic() -> Self {
        Self {
            literals: LiteralMode::Synchronizing,
            names: NameEncoding::ModifiedUtf7,
        }
    }

    /// The best options for these capabilities.
    #[must_use]
    pub fn for_capabilities(capabilities: &Capabilities, names: NameEncoding) -> Self {
        let literals = if capabilities.has(&Capability::LiteralPlus) {
            LiteralMode::NonSyncAny
        } else if capabilities.has(&Capability::LiteralMinus) {
            LiteralMode::NonSyncSmall
        } else {
            LiteralMode::Synchronizing
        };
        Self { literals, names }
    }

    fn utf8_strings(self) -> bool {
        self.names == NameEncoding::Utf8
    }
}

/// What the client must do after sending a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChunkEnd {
    /// Wait for a continuation request (`+`) before sending the next chunk.
    AwaitContinuation,
    /// The command is complete: wait for the tagged response.
    Complete,
}

/// A byte buffer for secrets that never grows in place.
///
/// `Vec::reserve` (and every `push` or `extend` that runs out of room) may
/// move the bytes to a new allocation and free the old one without wiping
/// it, leaving a copy of a password in freed memory. This buffer grows by
/// allocating a new wiped-on-drop `Vec`, copying, and dropping (wiping) the
/// old one. Nothing here calls `reserve` or lets the inner `Vec` grow.
pub(crate) struct SecretBuf(Zeroizing<Vec<u8>>);

impl SecretBuf {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(Zeroizing::new(Vec::with_capacity(capacity)))
    }

    /// Makes room for `extra` more bytes plus a CRLF (most writes are
    /// followed by one), so that ending the line does not move the data.
    fn make_room(&mut self, extra: usize) {
        let needed = self.0.len().saturating_add(extra);
        if needed <= self.0.capacity() {
            return;
        }
        let capacity = self
            .0
            .capacity()
            .saturating_mul(2)
            .max(needed.saturating_add(2));
        let mut grown = Zeroizing::new(Vec::with_capacity(capacity));
        grown.extend_from_slice(&self.0);
        // The old allocation is wiped when dropped here.
        self.0 = grown;
    }

    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.make_room(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    pub(crate) fn push(&mut self, byte: u8) {
        self.make_room(1);
        self.0.push(byte);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn into_inner(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}

/// A part of a command that is sent in one go.
///
/// The bytes are wiped from memory when dropped, because LOGIN and
/// AUTHENTICATE put credentials in them.
pub struct Chunk {
    bytes: Zeroizing<Vec<u8>>,
    end: ChunkEnd,
}

impl Chunk {
    /// The bytes to send. They may contain credentials or message content:
    /// never log them.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// What to do after sending this chunk.
    #[must_use]
    pub fn end(&self) -> ChunkEnd {
        self.end
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the bytes: they may hold a password or a message.
        f.debug_struct("Chunk")
            .field("len", &self.bytes.len())
            .field("end", &self.end)
            .finish()
    }
}

/// A command ready to send: its tag, its name (for logs) and its chunks.
#[derive(Debug)]
pub struct EncodedCommand {
    tag: Tag,
    name: &'static str,
    chunks: Vec<Chunk>,
}

impl EncodedCommand {
    /// The tag of the command.
    #[must_use]
    pub fn tag(&self) -> &Tag {
        &self.tag
    }

    /// The command name, safe to log (`LOGIN`, `UID FETCH`…).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The chunks, in order.
    #[must_use]
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Takes the chunks.
    #[must_use]
    pub fn into_chunks(self) -> Vec<Chunk> {
        self.chunks
    }
}

/// Builds the bytes of one command.
pub struct Writer {
    options: EncodeOptions,
    chunks: Vec<Chunk>,
    current: SecretBuf,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("chunks", &self.chunks.len())
            .finish_non_exhaustive()
    }
}

impl Writer {
    pub(crate) fn new(options: EncodeOptions) -> Self {
        Self {
            options,
            chunks: Vec::new(),
            current: SecretBuf::with_capacity(64),
        }
    }

    pub(crate) fn options(&self) -> EncodeOptions {
        self.options
    }

    /// Bytes we wrote ourselves (command names, numbers, punctuation).
    /// Only used with constants of this crate, which never contain CR/LF.
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        debug_assert!(!bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0)));
        self.current.extend(bytes);
    }

    pub(crate) fn space(&mut self) {
        self.current.push(b' ');
    }

    pub(crate) fn number(&mut self, value: impl Into<u64>) {
        self.current.extend(value.into().to_string().as_bytes());
    }

    /// Something whose `Display` is made of safe characters only (sequence
    /// sets, flags, dates…).
    pub(crate) fn display(&mut self, value: &impl std::fmt::Display) {
        let text = value.to_string();
        debug_assert!(!text.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)));
        self.current.extend(text.as_bytes());
    }

    /// An `astring`: atom if possible, else quoted, else literal.
    pub(crate) fn astring(&mut self, bytes: &[u8], what: &'static str) -> Result<(), EncodeError> {
        if !bytes.is_empty() && bytes.iter().copied().all(is_atom_char) {
            self.current.extend(bytes);
            Ok(())
        } else {
            self.string(bytes, what)
        }
    }

    /// A `string`: quoted if possible, else literal.
    pub(crate) fn string(&mut self, bytes: &[u8], what: &'static str) -> Result<(), EncodeError> {
        if bytes.contains(&0) {
            return Err(EncodeError::Nul(what));
        }
        if self.can_quote(bytes) {
            self.quoted(bytes);
            Ok(())
        } else {
            self.literal(bytes, what)
        }
    }

    fn can_quote(&self, bytes: &[u8]) -> bool {
        if bytes.len() > MAX_QUOTED_LEN {
            return false;
        }
        let seven_bit = bytes
            .iter()
            .all(|b| matches!(b, 0x01..=0x7f) && !matches!(b, b'\r' | b'\n'));
        if seven_bit {
            return true;
        }
        // RFC 6855: after UTF8=ACCEPT, quoted strings may hold UTF-8.
        self.options.utf8_strings()
            && std::str::from_utf8(bytes).is_ok()
            && !bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0))
    }

    fn quoted(&mut self, bytes: &[u8]) {
        // Room for the worst case (every byte escaped) at once.
        self.current
            .make_room(bytes.len().saturating_mul(2).saturating_add(2));
        self.current.push(b'"');
        for &byte in bytes {
            if matches!(byte, b'"' | b'\\') {
                self.current.push(b'\\');
            }
            self.current.push(byte);
        }
        self.current.push(b'"');
    }

    /// A literal, always (APPEND message data).
    pub(crate) fn literal(&mut self, bytes: &[u8], what: &'static str) -> Result<(), EncodeError> {
        if bytes.contains(&0) {
            return Err(EncodeError::Nul(what));
        }
        let non_sync = match self.options.literals {
            LiteralMode::NonSyncAny => true,
            LiteralMode::NonSyncSmall => bytes.len() <= LITERAL_MINUS_MAX,
            LiteralMode::Synchronizing => false,
        };
        self.current.push(b'{');
        self.current.extend(bytes.len().to_string().as_bytes());
        if non_sync {
            self.current.extend(b"+}\r\n");
        } else {
            self.current.extend(b"}\r\n");
            self.end_chunk(ChunkEnd::AwaitContinuation);
        }
        self.current.extend(bytes);
        Ok(())
    }

    /// A `literal8` (`~{n}`, BINARY): may hold NUL bytes. Always
    /// synchronizing, so the server can refuse it before the data is sent.
    pub(crate) fn literal8(&mut self, bytes: &[u8]) {
        self.current.extend(b"~{");
        self.current.extend(bytes.len().to_string().as_bytes());
        self.current.extend(b"}\r\n");
        self.end_chunk(ChunkEnd::AwaitContinuation);
        self.current.extend(bytes);
    }

    /// Ends the current chunk: the client sends it and waits for `+`.
    pub(crate) fn await_continuation(&mut self) {
        self.current.extend(b"\r\n");
        self.end_chunk(ChunkEnd::AwaitContinuation);
    }

    fn end_chunk(&mut self, end: ChunkEnd) {
        let bytes = std::mem::replace(&mut self.current, SecretBuf::with_capacity(0));
        self.chunks.push(Chunk {
            bytes: bytes.into_inner(),
            end,
        });
    }

    pub(crate) fn finish(mut self, tag: Tag, name: &'static str) -> EncodedCommand {
        self.current.extend(b"\r\n");
        self.end_chunk(ChunkEnd::Complete);
        EncodedCommand {
            tag,
            name,
            chunks: self.chunks,
        }
    }
}

/// A line sent outside any command: `DONE` after IDLE or a SASL response.
pub struct RawLine(Zeroizing<Vec<u8>>);

impl RawLine {
    pub(crate) fn new(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self(bytes)
    }

    /// The bytes to send, CRLF included. Never log them.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for RawLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawLine")
            .field("len", &self.0.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(options: EncodeOptions, f: impl FnOnce(&mut Writer)) -> Vec<(Vec<u8>, ChunkEnd)> {
        let mut writer = Writer::new(options);
        f(&mut writer);
        writer
            .finish(Tag::new("A1").unwrap(), "TEST")
            .into_chunks()
            .into_iter()
            .map(|chunk| (chunk.bytes().to_vec(), chunk.end()))
            .collect()
    }

    #[test]
    fn chooses_atom_quoted_literal() {
        let chunks = written(EncodeOptions::basic(), |w| {
            w.astring(b"alice", "x").unwrap();
            w.space();
            w.astring(b"two words \"q\" \\", "x").unwrap();
            w.space();
            w.astring(b"", "x").unwrap();
        });
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, b"alice \"two words \\\"q\\\" \\\\\" \"\"\r\n");

        let chunks = written(EncodeOptions::basic(), |w| {
            w.astring(b"a\r\nb", "x").unwrap();
        });
        assert_eq!(
            chunks[0],
            (b"{4}\r\n".to_vec(), ChunkEnd::AwaitContinuation)
        );
        assert_eq!(chunks[1], (b"a\r\nb\r\n".to_vec(), ChunkEnd::Complete));
    }

    #[test]
    fn non_sync_literals() {
        let plus = EncodeOptions {
            literals: LiteralMode::NonSyncAny,
            names: NameEncoding::ModifiedUtf7,
        };
        let chunks = written(plus, |w| w.literal(&[b'x'; 5000], "x").unwrap());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].0.starts_with(b"{5000+}\r\n"));

        let minus = EncodeOptions {
            literals: LiteralMode::NonSyncSmall,
            ..plus
        };
        let chunks = written(minus, |w| w.literal(&[b'x'; 5000], "x").unwrap());
        assert_eq!(chunks.len(), 2);
        let chunks = written(minus, |w| w.literal(b"small", "x").unwrap());
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn utf8_quoting_depends_on_mode() {
        let chunks = written(EncodeOptions::basic(), |w| {
            w.astring("é".as_bytes(), "x").unwrap();
        });
        assert_eq!(chunks.len(), 2);
        let utf8 = EncodeOptions {
            names: NameEncoding::Utf8,
            ..EncodeOptions::basic()
        };
        let chunks = written(utf8, |w| w.astring("é".as_bytes(), "x").unwrap());
        assert_eq!(chunks[0].0, "\"é\"\r\n".as_bytes());
    }

    #[test]
    fn literal_reserves_crlf() {
        // A synchronizing literal starts a new chunk: the data and the
        // final CRLF must fit in one allocation, so `finish` does not move
        // (and leave behind) the secret.
        let secret = [b'x'; 300];
        let mut writer = Writer::new(EncodeOptions::basic());
        writer.literal(&secret, "x").unwrap();
        assert_eq!(writer.chunks.len(), 1);
        assert!(writer.current.capacity() >= writer.current.len() + 2);
        let before = writer.current.as_slice().as_ptr();
        let chunks = writer.finish(Tag::new("A1").unwrap(), "TEST").into_chunks();
        assert_eq!(chunks[1].bytes().as_ptr(), before);
        assert_eq!(chunks[1].bytes().len(), 302);
    }

    #[test]
    fn quoted_reserves_once() {
        let mut writer = Writer::new(EncodeOptions::basic());
        writer.raw(b"LOGIN ");
        let before = writer.current.as_slice().as_ptr();
        let capacity = writer.current.capacity();
        // Grows once (the start of the command is copied, then wiped).
        writer.astring(&[b'"'; 200], "x").unwrap();
        let after = writer.current.as_slice().as_ptr();
        assert_ne!(before, after);
        assert!(writer.current.capacity() > capacity);
        let grown = writer.current.capacity();
        writer.current.extend(b"\r\n");
        assert_eq!(writer.current.capacity(), grown);
        assert_eq!(writer.current.as_slice().as_ptr(), after);
    }

    proptest::proptest! {
        /// `SecretBuf` holds the same bytes as a plain `Vec`.
        #[test]
        fn secret_buf_matches_vec(
            steps in proptest::collection::vec(
                proptest::option::of(proptest::collection::vec(proptest::num::u8::ANY, 0..300)),
                0..40,
            ),
        ) {
            let mut secret = SecretBuf::with_capacity(0);
            let mut plain = Vec::new();
            for step in steps {
                if let Some(bytes) = step {
                    secret.extend(&bytes);
                    plain.extend_from_slice(&bytes);
                } else {
                    secret.push(7);
                    plain.push(7);
                }
                proptest::prop_assert!(secret.capacity() >= secret.len());
            }
            proptest::prop_assert_eq!(secret.as_slice(), &plain[..]);
        }
    }

    #[test]
    fn nul_is_refused() {
        let mut writer = Writer::new(EncodeOptions::basic());
        assert_eq!(writer.astring(b"a\0", "x"), Err(EncodeError::Nul("x")));
        assert_eq!(writer.literal(b"a\0", "x"), Err(EncodeError::Nul("x")));
    }
}
