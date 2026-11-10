// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Server lines (RFC 5804 §4): data lines and the `OK` / `NO` / `BYE`
//! that ends every response.

use std::fmt;

use crate::item::{Framing, Side, encode_items, encode_string};
use crate::{Atom, DecodeError, Item, Limits};

/// How a response ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Success.
    Ok,
    /// Failure (the connection stays usable).
    No,
    /// The server closes the connection.
    Bye,
}

impl Status {
    fn keyword(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::No => "NO",
            Self::Bye => "BYE",
        }
    }
}

/// A response code (RFC 5804 §1.3), the part in parentheses.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseCode {
    /// `AUTH-TOO-WEAK`.
    AuthTooWeak,
    /// `ENCRYPT-NEEDED`: TLS first.
    EncryptNeeded,
    /// `QUOTA` without detail.
    Quota,
    /// `QUOTA/MAXSCRIPTS`: too many scripts.
    QuotaMaxScripts,
    /// `QUOTA/MAXSIZE`: script too big.
    QuotaMaxSize,
    /// `REFERRAL sieveurl`: we never follow referrals (the admin chose the
    /// server).
    Referral(Vec<u8>),
    /// `SASL string`: data of a successful authentication.
    Sasl(Vec<u8>),
    /// `TRANSITION-NEEDED`.
    TransitionNeeded,
    /// `TRYLATER`: temporary failure.
    TryLater,
    /// `ACTIVE`: the script is active (DELETESCRIPT refused).
    Active,
    /// `NONEXISTENT`: no such script.
    Nonexistent,
    /// `ALREADYEXISTS`: the new name is taken.
    AlreadyExists,
    /// `WARNINGS`: the script was stored but has warnings (in the text).
    Warnings,
    /// `TAG string`: the tag of a NOOP.
    Tag(Vec<u8>),
    /// Another code, with whatever follows its name.
    Other {
        /// Its name.
        name: Atom,
        /// The rest.
        data: Vec<Item>,
    },
}

impl fmt::Debug for ResponseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Other { name, data } => return write!(f, "Other({name}, {} items)", data.len()),
            Self::Referral(_) => "REFERRAL",
            Self::Sasl(_) => "SASL",
            Self::Tag(_) => "TAG",
            other => other.simple_name().unwrap_or("?"),
        };
        f.write_str(name)
    }
}

const SIMPLE_CODES: [(&str, ResponseCode); 11] = [
    ("AUTH-TOO-WEAK", ResponseCode::AuthTooWeak),
    ("ENCRYPT-NEEDED", ResponseCode::EncryptNeeded),
    ("QUOTA", ResponseCode::Quota),
    ("QUOTA/MAXSCRIPTS", ResponseCode::QuotaMaxScripts),
    ("QUOTA/MAXSIZE", ResponseCode::QuotaMaxSize),
    ("TRANSITION-NEEDED", ResponseCode::TransitionNeeded),
    ("TRYLATER", ResponseCode::TryLater),
    ("ACTIVE", ResponseCode::Active),
    ("NONEXISTENT", ResponseCode::Nonexistent),
    ("ALREADYEXISTS", ResponseCode::AlreadyExists),
    ("WARNINGS", ResponseCode::Warnings),
];

impl ResponseCode {
    fn simple_name(&self) -> Option<&'static str> {
        SIMPLE_CODES
            .iter()
            .find(|(_, code)| code == self)
            .map(|(name, _)| *name)
    }

    /// Any `QUOTA` code.
    #[must_use]
    pub fn is_quota(&self) -> bool {
        matches!(
            self,
            Self::Quota | Self::QuotaMaxScripts | Self::QuotaMaxSize
        )
    }

    fn parse(items: Vec<Item>) -> Result<Self, DecodeError> {
        let mut items = items.into_iter();
        let Some(Item::Atom(name)) = items.next() else {
            return Err(DecodeError::Malformed("response code without a name"));
        };
        let rest: Vec<Item> = items.collect();
        if rest.is_empty()
            && let Some((_, code)) = SIMPLE_CODES.iter().find(|(known, _)| name.is(known))
        {
            return Ok(code.clone());
        }
        let make: fn(Vec<u8>) -> Self = if name.is("SASL") {
            Self::Sasl
        } else if name.is("TAG") {
            Self::Tag
        } else if name.is("REFERRAL") {
            Self::Referral
        } else {
            return Ok(Self::Other { name, data: rest });
        };
        let mut rest = rest.into_iter();
        let argument = match (rest.next(), rest.next()) {
            // REFERRAL's URL may come as an atom.
            (Some(Item::Atom(url)), None) if name.is("REFERRAL") => {
                Some(url.as_str().as_bytes().to_vec())
            }
            (Some(item), None) => item.into_bytes(),
            _ => None,
        };
        argument
            .map(make)
            .ok_or(DecodeError::Malformed("bad response code argument"))
    }

    fn items(&self) -> Vec<Item> {
        let atom = |name: &str| Atom::new(name).map(Item::Atom);
        let named = |name: &str, bytes: &[u8]| {
            atom(name)
                .into_iter()
                .chain([Item::Quoted(bytes.to_vec())])
                .collect()
        };
        match self {
            Self::Referral(url) => named("REFERRAL", url),
            Self::Sasl(data) => named("SASL", data),
            Self::Tag(tag) => named("TAG", tag),
            Self::Other { name, data } => [Item::Atom(name.clone())]
                .into_iter()
                .chain(data.iter().cloned())
                .collect(),
            simple => simple.simple_name().and_then(atom).into_iter().collect(),
        }
    }
}

/// The line that ends a response.
#[derive(Clone, PartialEq, Eq)]
pub struct Completion {
    /// OK, NO or BYE.
    pub status: Status,
    /// The response code, if any.
    pub code: Option<ResponseCode>,
    /// The human-readable text, if any (bytes: servers may send anything).
    pub text: Option<Vec<u8>>,
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("status", &self.status)
            .field("code", &self.code)
            .field("text", &self.text_one_line())
            .finish()
    }
}

/// Longest text kept for errors and logs (it may be shown to the user).
const MAX_TEXT_SHOWN: usize = 2048;

/// `text` on one line: every control character (line breaks, `U+0085`,
/// escapes) and `U+2028` / `U+2029` become a space, so a server can't
/// forge log lines.
#[must_use]
pub fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

impl Completion {
    /// The text as UTF-8 (invalid bytes replaced), without control
    /// characters other than `\n`, at most 2048 characters. For showing
    /// to the user (script errors span lines); it keeps line breaks, so
    /// not for logs or error messages: use [`Completion::text_one_line`].
    #[must_use]
    pub fn text_lossy(&self) -> String {
        let text = String::from_utf8_lossy(self.text.as_deref().unwrap_or_default());
        text.chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .take(MAX_TEXT_SHOWN)
            .collect()
    }

    /// [`Completion::text_lossy`] on one line ([`one_line`]): safe to log.
    #[must_use]
    pub fn text_one_line(&self) -> String {
        one_line(&self.text_lossy())
    }

    fn parse(status: Status, items: Vec<Item>) -> Result<Self, DecodeError> {
        let mut completion = Self {
            status,
            code: None,
            text: None,
        };
        let mut items = items.into_iter().peekable();
        if let Some(Item::List(_)) = items.peek()
            && let Some(Item::List(list)) = items.next()
        {
            completion.code = Some(ResponseCode::parse(list)?);
        }
        match (items.next(), items.next()) {
            (None, _) => {}
            (Some(item), None) => {
                completion.text = Some(
                    item.into_bytes()
                        .ok_or(DecodeError::Malformed("status text is not a string"))?,
                );
            }
            (Some(_), Some(_)) => return Err(DecodeError::Malformed("extra items after the text")),
        }
        Ok(completion)
    }
}

/// One line from a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerLine {
    /// A data line: strings and atoms. What it means depends on the
    /// command (a capability, a listed script, a script's content, a SASL
    /// challenge).
    Data(Vec<Item>),
    /// The end of a response.
    Done(Completion),
}

impl ServerLine {
    fn parse(items: Vec<Item>) -> Result<Self, DecodeError> {
        let mut items = items.into_iter();
        match items.next() {
            Some(Item::Atom(atom)) => {
                let status = [Status::Ok, Status::No, Status::Bye]
                    .into_iter()
                    .find(|status| atom.is(status.keyword()))
                    .ok_or(DecodeError::Malformed("unknown status"))?;
                Completion::parse(status, items.collect()).map(Self::Done)
            }
            Some(first @ (Item::Quoted(_) | Item::Literal(_))) => {
                let data: Vec<Item> = std::iter::once(first).chain(items).collect();
                if data.iter().any(|item| matches!(item, Item::List(_))) {
                    return Err(DecodeError::Malformed("list in a data line"));
                }
                Ok(Self::Data(data))
            }
            Some(Item::List(_)) => Err(DecodeError::Malformed("line starts with a list")),
            None => Err(DecodeError::Malformed("empty line")),
        }
    }

    /// The line as a server writes it (fake servers, round-trip tests).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Data(items) => encode_items(&mut out, items, Side::Server),
            Self::Done(completion) => {
                out.extend_from_slice(completion.status.keyword().as_bytes());
                if let Some(code) = &completion.code {
                    out.push(b' ');
                    encode_items(&mut out, &[Item::List(code.items())], Side::Server);
                }
                if let Some(text) = &completion.text {
                    out.push(b' ');
                    encode_string(&mut out, text, Side::Server);
                }
            }
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

/// Reads server lines from bytes as they arrive.
///
/// Feed it with [`ResponseDecoder::feed`] and take lines with
/// [`ResponseDecoder::decode`] until it says it needs more. After an error
/// the stream is out of sync: drop the connection.
#[derive(Debug)]
pub struct ResponseDecoder {
    buffer: Vec<u8>,
    limits: Limits,
    /// Where the next line ends (each byte looked at once), or the error
    /// that broke the stream.
    framing: Framing,
}

impl ResponseDecoder {
    /// A decoder with these limits.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            buffer: Vec::new(),
            limits,
            framing: Framing::default(),
        }
    }

    /// Adds received bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes received but not decoded yet. After STARTTLS's `OK` this must
    /// be 0: anything else came before TLS and may be injected.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// The next complete line, or `None` if more bytes are needed.
    ///
    /// # Errors
    ///
    /// The bytes break the grammar or the limits. After a framing error
    /// every call returns it again.
    pub fn decode(&mut self) -> Result<Option<ServerLine>, DecodeError> {
        let Some((items, consumed)) =
            self.framing
                .next_line(&self.buffer, &self.limits, Side::Server)?
        else {
            return Ok(None);
        };
        self.buffer.drain(..consumed.min(self.buffer.len()));
        ServerLine::parse(items).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(bytes: &[u8]) -> Result<Vec<ServerLine>, DecodeError> {
        let mut decoder = ResponseDecoder::new(Limits::default());
        decoder.feed(bytes);
        let mut lines = Vec::new();
        while let Some(line) = decoder.decode()? {
            lines.push(line);
        }
        assert_eq!(decoder.buffered(), 0);
        Ok(lines)
    }

    fn done(status: Status, code: Option<ResponseCode>, text: Option<&str>) -> ServerLine {
        ServerLine::Done(Completion {
            status,
            code,
            text: text.map(|t| t.as_bytes().to_vec()),
        })
    }

    #[test]
    fn completions() {
        let lines = decode_all(
            b"OK\r\nok \"Logged in.\"\r\nNO (NONEXISTENT) \"no\"\r\n\
              NO (QUOTA/MAXSIZE) {5}\r\nbig\r\n\r\n\
              BYE (TRYLATER)\r\nOK (TAG \"t1\") \"Done\"\r\n\
              NO (SASL \"AQ==\")\r\nOK (WARNINGS) \"line 2: warning\"\r\n\
              NO (REFERRAL sieve://other.example) \"go\"\r\n\
              NO (X-THING 1 \"a\") \"x\"\r\nNO (activE)\r\n",
        )
        .unwrap();
        assert_eq!(
            lines,
            vec![
                done(Status::Ok, None, None),
                done(Status::Ok, None, Some("Logged in.")),
                done(Status::No, Some(ResponseCode::Nonexistent), Some("no")),
                done(
                    Status::No,
                    Some(ResponseCode::QuotaMaxSize),
                    Some("big\r\n")
                ),
                done(Status::Bye, Some(ResponseCode::TryLater), None),
                done(
                    Status::Ok,
                    Some(ResponseCode::Tag(b"t1".to_vec())),
                    Some("Done")
                ),
                done(Status::No, Some(ResponseCode::Sasl(b"AQ==".to_vec())), None),
                done(
                    Status::Ok,
                    Some(ResponseCode::Warnings),
                    Some("line 2: warning")
                ),
                done(
                    Status::No,
                    Some(ResponseCode::Referral(b"sieve://other.example".to_vec())),
                    Some("go")
                ),
                done(
                    Status::No,
                    Some(ResponseCode::Other {
                        name: Atom::new("X-THING").unwrap(),
                        data: vec![
                            Item::Atom(Atom::new("1").unwrap()),
                            Item::Quoted(b"a".to_vec())
                        ]
                    }),
                    Some("x")
                ),
                done(Status::No, Some(ResponseCode::Active), None),
            ]
        );
    }

    #[test]
    fn data_lines() {
        let lines =
            decode_all(b"\"IMPLEMENTATION\" \"Dovecot\"\r\n\"a\" ACTIVE\r\n{3}\r\nabc\r\n\"\"\r\n")
                .unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|line| matches!(line, ServerLine::Data(_))));
    }

    #[test]
    fn rejects_bad_lines() {
        for bad in [
            &b"MAYBE\r\n"[..],
            b"\r\n",
            b"(OK)\r\n",
            b"OK \"a\" \"b\"\r\n",
            b"OK ACTIVE\r\n",
            b"OK (\"x\")\r\n",
            b"NO (TAG)\r\n",
            b"NO (SASL a b)\r\n",
            b"\"a\" (b)\r\n",
        ] {
            assert!(decode_all(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn byte_by_byte_is_the_same() {
        let input: &[u8] =
            b"\"SIEVE\" \"fileinto\"\r\n{4}\r\na\r\nb\r\nOK (TAG {2}\r\nxy) \"z\"\r\n";
        let whole = decode_all(input).unwrap();
        let mut decoder = ResponseDecoder::new(Limits::default());
        let mut lines = Vec::new();
        for byte in input {
            decoder.feed(&[*byte]);
            while let Some(line) = decoder.decode().unwrap() {
                lines.push(line);
            }
        }
        assert_eq!(lines, whole);
    }

    #[test]
    fn each_line_is_parsed_once() {
        // A 1 MiB literal, then 10 000 spaces one at a time: the buffer is
        // never scanned again from the start (nor the literal copied again).
        // Counts the real calls to `scan_line`, not the lines returned.
        let before = crate::item::scans();
        let mut decoder = ResponseDecoder::new(Limits::default());
        decoder.feed(b"{1048576}\r\n");
        decoder.feed(&vec![b'x'; 1024 * 1024]);
        assert_eq!(decoder.decode(), Ok(None));
        for _ in 0..10_000 {
            decoder.feed(b" ");
            assert_eq!(decoder.decode(), Ok(None));
        }
        assert_eq!(crate::item::scans() - before, 0, "Ok(None) never scans");
        decoder.feed(b"\r\n");
        let line = decoder.decode().unwrap().unwrap();
        assert!(
            matches!(&line, ServerLine::Data(items) if items.len() == 1),
            "{line:?}"
        );
        assert_eq!(crate::item::scans() - before, 1);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn text_is_cleaned() {
        let completion = Completion {
            status: Status::No,
            code: None,
            text: Some(b"bad\x1b[31m \xff line\n2".to_vec()),
        };
        assert_eq!(completion.text_lossy(), "bad[31m \u{fffd} line\n2");
    }

    #[test]
    fn one_line_text() {
        let completion = Completion {
            status: Status::No,
            code: None,
            text: Some("a\nb\u{2028}c\u{2029}d\r\u{85}e".as_bytes().to_vec()),
        };
        // `text_lossy` already drops `\r` and `U+0085`.
        assert_eq!(completion.text_one_line(), "a b c de");
        assert_eq!(
            one_line("x\u{0b}y\u{0c}z\u{1b}[31m\r\n\t\u{85}\u{2028}\u{2029}w"),
            "x y z [31m      w"
        );
        assert!(!format!("{completion:?}").contains('\n'));
    }

    #[test]
    fn encodes_what_it_decodes() {
        let input: &[u8] = b"NO (QUOTA/MAXSCRIPTS) \"too many\"\r\n";
        let lines = decode_all(input).unwrap();
        assert_eq!(lines.first().unwrap().encode(), input);
    }
}
