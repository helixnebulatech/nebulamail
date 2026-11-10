// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `APPEND`, with several messages (MULTIAPPEND, RFC 3502), with
//! `literal8` for messages with NUL bytes (BINARY, RFC 3516) and with the
//! `UTF8` data extension for UTF-8 header fields (RFC 6855 §4).

use super::store::write_flag_list;
use super::{Writer, command, write_mailbox};
use crate::EncodeError;
use crate::types::{Flag, InternalDate, MailboxName};

/// How message data may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppendData {
    binary: bool,
    utf8: bool,
}

impl AppendData {
    /// Normal literals only: messages with NUL bytes are refused.
    pub const TEXT: Self = Self {
        binary: false,
        utf8: false,
    };

    /// The server has BINARY: a message with NUL bytes goes as `~{n}`
    /// (`literal8`, RFC 3516 §4.4); the others still as normal literals.
    #[must_use]
    pub const fn with_binary(self) -> Self {
        Self {
            binary: true,
            ..self
        }
    }

    /// `UTF8=ACCEPT` is enabled: a message with 8-bit bytes in its header
    /// section goes as `UTF8 (~{n})`, as RFC 6855 §4 requires ("MUST send
    /// them using the UTF8 data extension").
    #[must_use]
    pub const fn with_utf8(self) -> Self {
        Self { utf8: true, ..self }
    }
}

/// `APPEND mailbox [(flags)] ["date-time"] literal`
pub struct Append<'a> {
    /// Destination.
    pub mailbox: &'a MailboxName,
    /// Initial flags.
    pub flags: &'a [Flag],
    /// Internal date; the server uses "now" if absent.
    pub internal_date: Option<InternalDate>,
    /// The RFC 5322 message (CRLF line endings; NUL only with
    /// [`AppendData::with_binary`]).
    pub message: &'a [u8],
    /// Whether `literal8` and `UTF8 (...)` may be used.
    pub data: AppendData,
}

/// One message of a [`MultiAppend`].
pub struct AppendMessage<'a> {
    /// Initial flags.
    pub flags: &'a [Flag],
    /// Internal date; the server uses "now" if absent.
    pub internal_date: Option<InternalDate>,
    /// The RFC 5322 message.
    pub message: &'a [u8],
}

impl std::fmt::Debug for AppendMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppendMessage")
            .field("flags", &self.flags)
            .field("message_len", &self.message.len())
            .finish_non_exhaustive()
    }
}

/// `APPEND mailbox [(flags)] ["date-time"] literal [(flags)] ["date-time"]
/// literal...` (needs `MULTIAPPEND`). The server adds all of them or none.
#[derive(Debug)]
pub struct MultiAppend<'a> {
    /// Destination.
    pub mailbox: &'a MailboxName,
    /// At least one message.
    pub messages: &'a [AppendMessage<'a>],
    /// Whether `literal8` and `UTF8 (...)` may be used.
    pub data: AppendData,
}

fn write_message(
    writer: &mut Writer,
    flags: &[Flag],
    internal_date: Option<InternalDate>,
    message: &[u8],
    data: AppendData,
) -> Result<(), EncodeError> {
    if message.is_empty() {
        return Err(EncodeError::Empty("message"));
    }
    if !flags.is_empty() {
        writer.space();
        write_flag_list(writer, flags)?;
    }
    if let Some(date) = internal_date {
        writer.raw(b" \"");
        writer.display(&date);
        writer.raw(b"\"");
    }
    writer.space();
    let nul = message.contains(&0);
    if nul && !data.binary {
        return Err(EncodeError::Nul("message"));
    }
    if data.utf8 && has_8bit_header(message) {
        // RFC 6855 §4: utf8-literal = "UTF8" SP "(" literal8 ")".
        writer.raw(b"UTF8 (");
        writer.literal8(message);
        writer.raw(b")");
        Ok(())
    } else if nul {
        writer.literal8(message);
        Ok(())
    } else {
        writer.literal(message, "message")
    }
}

/// Whether the header section (RFC 5322 §2.1: up to the first empty line)
/// has a byte outside ASCII, that is UTF-8 header fields (RFC 6532).
fn has_8bit_header(message: &[u8]) -> bool {
    let end = message
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(message.len());
    message.get(..end).is_some_and(|header| !header.is_ascii())
}

impl std::fmt::Debug for Append<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The message content is never printed.
        f.debug_struct("Append")
            .field("mailbox", &self.mailbox)
            .field("flags", &self.flags)
            .field("message_len", &self.message.len())
            .finish_non_exhaustive()
    }
}

command!(Append<'_>, "APPEND", |append, writer| {
    writer.raw(b"APPEND ");
    write_mailbox(writer, append.mailbox)?;
    write_message(
        writer,
        append.flags,
        append.internal_date,
        append.message,
        append.data,
    )
});

command!(MultiAppend<'_>, "APPEND", |append, writer| {
    if append.messages.is_empty() {
        return Err(EncodeError::Empty("messages"));
    }
    writer.raw(b"APPEND ");
    write_mailbox(writer, append.mailbox)?;
    for message in append.messages {
        write_message(
            writer,
            message.flags,
            message.internal_date,
            message.message,
            append.data,
        )?;
    }
    Ok(())
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{
        EncodeOptions, LiteralMode,
        test_util::{basic, wire},
    };
    use crate::types::NameEncoding;

    #[test]
    fn encodes() {
        let drafts = MailboxName::new("Drafts").unwrap();
        let date: InternalDate = "17-Jul-1996 02:44:25 -0700".parse().unwrap();
        let append = Append {
            mailbox: &drafts,
            flags: &[Flag::Draft],
            internal_date: Some(date),
            message: b"Subject: hi\r\n\r\nbody\r\n",
            data: AppendData::TEXT,
        };
        assert_eq!(
            basic(&append),
            "A1 APPEND Drafts (\\Draft) \"17-Jul-1996 02:44:25 -0700\" {21}\r\n<WAIT>Subject: hi\r\n\r\nbody\r\n\r\n"
        );
        let plus = EncodeOptions {
            literals: LiteralMode::NonSyncAny,
            names: NameEncoding::ModifiedUtf7,
        };
        let append = Append {
            mailbox: &drafts,
            flags: &[],
            internal_date: None,
            message: b"x",
            data: AppendData::TEXT,
        };
        assert_eq!(wire(&append, plus), "A1 APPEND Drafts {1+}\r\nx\r\n");
    }

    #[test]
    fn nul_needs_binary() {
        let drafts = MailboxName::new("Drafts").unwrap();
        let mut append = Append {
            mailbox: &drafts,
            flags: &[],
            internal_date: None,
            message: b"a\0b",
            data: AppendData::TEXT,
        };
        let encoded = crate::command::encode(
            &append,
            crate::types::Tag::new("A1").unwrap(),
            EncodeOptions::basic(),
        );
        assert_eq!(encoded.err(), Some(EncodeError::Nul("message")));
        append.data = AppendData::TEXT.with_binary();
        assert_eq!(basic(&append), "A1 APPEND Drafts ~{3}\r\n<WAIT>a\0b\r\n");
    }

    #[test]
    fn several_messages() {
        let inbox = MailboxName::inbox();
        let messages = [
            AppendMessage {
                flags: &[Flag::Seen],
                internal_date: None,
                message: b"one",
            },
            AppendMessage {
                flags: &[],
                internal_date: None,
                message: b"two",
            },
        ];
        let append = MultiAppend {
            mailbox: &inbox,
            messages: &messages,
            data: AppendData::TEXT,
        };
        assert_eq!(
            basic(&append),
            "A1 APPEND INBOX (\\Seen) {3}\r\n<WAIT>one {3}\r\n<WAIT>two\r\n"
        );
        let none = MultiAppend {
            mailbox: &inbox,
            messages: &[],
            data: AppendData::TEXT,
        };
        assert!(
            crate::command::encode(
                &none,
                crate::types::Tag::new("A1").unwrap(),
                EncodeOptions::basic()
            )
            .is_err()
        );
    }

    #[test]
    fn utf8_headers_use_the_utf8_extension() {
        let inbox = MailboxName::inbox();
        let message = "Subject: Año nuevo\r\n\r\ncafé\r\n".as_bytes();
        let mut append = Append {
            mailbox: &inbox,
            flags: &[],
            internal_date: None,
            message,
            data: AppendData::TEXT.with_utf8(),
        };
        let len = message.len();
        assert_eq!(
            basic(&append),
            format!(
                "A1 APPEND INBOX UTF8 (~{{{len}}}\r\n<WAIT>{})\r\n",
                String::from_utf8_lossy(message)
            )
        );
        // 8-bit only in the body: a normal literal.
        let body_only = "Subject: hi\r\n\r\ncafé\r\n".as_bytes();
        append.message = body_only;
        assert!(basic(&append).starts_with("A1 APPEND INBOX {"));
        // Without UTF8=ACCEPT enabled: a normal literal too.
        append.message = message;
        append.data = AppendData::TEXT;
        assert!(basic(&append).starts_with("A1 APPEND INBOX {"));
        // A message without a body counts as all header.
        assert!(has_8bit_header("Subject: é".as_bytes()));
        assert!(!has_8bit_header(b"Subject: e\r\n\r\n\xc3\xa9"));
    }

    #[test]
    fn nul_still_needs_binary_with_utf8() {
        let inbox = MailboxName::inbox();
        let append = Append {
            mailbox: &inbox,
            flags: &[],
            internal_date: None,
            message: "Subject: é\r\n\r\na\0b".as_bytes(),
            data: AppendData::TEXT.with_utf8(),
        };
        let encoded = crate::command::encode(
            &append,
            crate::types::Tag::new("A1").unwrap(),
            EncodeOptions::basic(),
        );
        assert_eq!(encoded.err(), Some(EncodeError::Nul("message")));
    }
}
