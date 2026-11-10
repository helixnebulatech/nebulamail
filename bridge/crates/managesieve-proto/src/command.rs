// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Client commands (RFC 5804 §2): encoding and, for fake servers and
//! fuzzing, decoding.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use zeroize::Zeroizing;

use crate::item::{Framing, Side, encode_literal, encode_string};
use crate::{DecodeError, Item, Limits, ScriptName};

/// A SASL mechanism name (RFC 4422 §3.1: 1–20 of `A-Z 0-9 - _`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mechanism(String);

impl Mechanism {
    /// `PLAIN` (RFC 4616).
    #[must_use]
    pub fn plain() -> Self {
        Self("PLAIN".to_owned())
    }

    /// Checks a name (upper-cased).
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let name = name.to_ascii_uppercase();
        let valid = (1..=20).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        valid.then_some(Self(name))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// SASL data (an initial response or an answer to a challenge), as raw
/// bytes: base64 happens when encoding. Wiped from memory on drop and never
/// printed: it holds passwords and tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct SaslData(Zeroizing<Vec<u8>>);

impl SaslData {
    /// Wraps raw bytes.
    #[must_use]
    pub fn new(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    fn base64(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(
            self.0
                .len()
                .saturating_mul(4)
                .saturating_div(3)
                .saturating_add(4),
        ));
        BASE64.encode_string(&*self.0, &mut out);
        out
    }
}

impl fmt::Debug for SaslData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SaslData(..)")
    }
}

/// A client command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// `AUTHENTICATE "mechanism" ["initial response"]`.
    Authenticate {
        /// The mechanism.
        mechanism: Mechanism,
        /// The initial response (sent with the command, RFC 4959 style).
        initial_response: Option<SaslData>,
    },
    /// The client's answer to a SASL challenge: a base64 string alone.
    SaslResponse(SaslData),
    /// Cancels the authentication: `"*"`.
    SaslCancel,
    /// `STARTTLS`.
    StartTls,
    /// `LOGOUT`.
    Logout,
    /// `CAPABILITY`.
    Capability,
    /// `UNAUTHENTICATE` (RFC 5804 §2.14.1).
    Unauthenticate,
    /// `NOOP ["tag"]`.
    Noop {
        /// Echoed back in a `TAG` response code.
        tag: Option<Vec<u8>>,
    },
    /// `HAVESPACE "name" size`.
    HaveSpace {
        /// The script name.
        name: ScriptName,
        /// Its size in bytes.
        size: u64,
    },
    /// `PUTSCRIPT "name" {n+}…`.
    PutScript {
        /// The script name.
        name: ScriptName,
        /// The script, opaque bytes.
        content: Vec<u8>,
    },
    /// `LISTSCRIPTS`.
    ListScripts,
    /// `SETACTIVE "name"`, or `SETACTIVE ""` to deactivate every script.
    SetActive {
        /// The script, or `None`.
        name: Option<ScriptName>,
    },
    /// `GETSCRIPT "name"`.
    GetScript {
        /// The script name.
        name: ScriptName,
    },
    /// `DELETESCRIPT "name"`.
    DeleteScript {
        /// The script name.
        name: ScriptName,
    },
    /// `RENAMESCRIPT "old" "new"`.
    RenameScript {
        /// The current name.
        from: ScriptName,
        /// The new name.
        to: ScriptName,
    },
    /// `CHECKSCRIPT {n+}…`.
    CheckScript {
        /// The script, opaque bytes.
        content: Vec<u8>,
    },
}

impl Command {
    /// The command's keyword, for logs (never its arguments).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Authenticate { .. } => "AUTHENTICATE",
            Self::SaslResponse(_) | Self::SaslCancel => "AUTHENTICATE (continued)",
            Self::StartTls => "STARTTLS",
            Self::Logout => "LOGOUT",
            Self::Capability => "CAPABILITY",
            Self::Unauthenticate => "UNAUTHENTICATE",
            Self::Noop { .. } => "NOOP",
            Self::HaveSpace { .. } => "HAVESPACE",
            Self::PutScript { .. } => "PUTSCRIPT",
            Self::ListScripts => "LISTSCRIPTS",
            Self::SetActive { .. } => "SETACTIVE",
            Self::GetScript { .. } => "GETSCRIPT",
            Self::DeleteScript { .. } => "DELETESCRIPT",
            Self::RenameScript { .. } => "RENAMESCRIPT",
            Self::CheckScript { .. } => "CHECKSCRIPT",
        }
    }

    /// The bytes to send, CRLF included. Zeroized on drop (it may hold a
    /// password).
    #[must_use]
    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let size = match self {
            Self::PutScript { content, .. } | Self::CheckScript { content } => content.len(),
            _ => 0,
        };
        let mut out = Zeroizing::new(Vec::with_capacity(size.saturating_add(2048)));
        let client = Side::Client;
        let name = |out: &mut Vec<u8>, name: &ScriptName| {
            out.push(b' ');
            encode_string(out, name.as_str().as_bytes(), client);
        };
        match self {
            Self::Authenticate {
                mechanism,
                initial_response,
            } => {
                out.extend_from_slice(b"AUTHENTICATE ");
                encode_string(&mut out, mechanism.as_str().as_bytes(), client);
                if let Some(data) = initial_response {
                    out.push(b' ');
                    encode_string(&mut out, data.base64().as_bytes(), client);
                }
            }
            Self::SaslResponse(data) => encode_string(&mut out, data.base64().as_bytes(), client),
            Self::SaslCancel => out.extend_from_slice(b"\"*\""),
            Self::StartTls
            | Self::Logout
            | Self::Capability
            | Self::Unauthenticate
            | Self::ListScripts => out.extend_from_slice(self.name().as_bytes()),
            Self::Noop { tag } => {
                out.extend_from_slice(b"NOOP");
                if let Some(tag) = tag {
                    out.push(b' ');
                    encode_string(&mut out, tag, client);
                }
            }
            Self::HaveSpace { name: script, size } => {
                out.extend_from_slice(b"HAVESPACE");
                name(&mut out, script);
                out.push(b' ');
                out.extend_from_slice(size.to_string().as_bytes());
            }
            Self::PutScript {
                name: script,
                content,
            } => {
                out.extend_from_slice(b"PUTSCRIPT");
                name(&mut out, script);
                out.push(b' ');
                encode_literal(&mut out, content, client);
            }
            Self::SetActive { name: script } => {
                out.extend_from_slice(b"SETACTIVE");
                match script {
                    Some(script) => name(&mut out, script),
                    None => out.extend_from_slice(b" \"\""),
                }
            }
            Self::GetScript { name: script } => {
                out.extend_from_slice(b"GETSCRIPT");
                name(&mut out, script);
            }
            Self::DeleteScript { name: script } => {
                out.extend_from_slice(b"DELETESCRIPT");
                name(&mut out, script);
            }
            Self::RenameScript { from, to } => {
                out.extend_from_slice(b"RENAMESCRIPT");
                name(&mut out, from);
                name(&mut out, to);
            }
            Self::CheckScript { content } => {
                out.extend_from_slice(b"CHECKSCRIPT ");
                encode_literal(&mut out, content, client);
            }
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

/// Reads client commands from bytes (fake servers in tests, fuzzing).
#[derive(Debug)]
pub struct CommandDecoder {
    buffer: Zeroizing<Vec<u8>>,
    limits: Limits,
    /// Where the next line ends, or the error that broke the stream.
    framing: Framing,
}

impl CommandDecoder {
    /// A decoder with these limits.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            buffer: Zeroizing::new(Vec::new()),
            limits,
            framing: Framing::default(),
        }
    }

    /// Adds received bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes received but not decoded yet.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// The next complete line. After a framing error (the stream is out of
    /// sync) every call returns it again; a complete line that is not a
    /// valid command leaves the decoder in sync.
    fn line(&mut self) -> Result<Option<Vec<Item>>, DecodeError> {
        let Some((items, consumed)) =
            self.framing
                .next_line(&self.buffer, &self.limits, Side::Client)?
        else {
            return Ok(None);
        };
        let consumed = consumed.min(self.buffer.len());
        self.buffer.drain(..consumed);
        Ok(Some(items))
    }

    /// The next command.
    ///
    /// # Errors
    ///
    /// Not a valid command.
    pub fn decode_command(&mut self) -> Result<Option<Command>, DecodeError> {
        self.line()?.map(parse_command).transpose()
    }

    /// The next line as the answer to a SASL challenge
    /// ([`Command::SaslResponse`] or [`Command::SaslCancel`]).
    ///
    /// # Errors
    ///
    /// Not one base64 string.
    pub fn decode_sasl(&mut self) -> Result<Option<Command>, DecodeError> {
        let Some(items) = self.line()? else {
            return Ok(None);
        };
        match items.as_slice() {
            [item] if item.as_bytes() == Some(b"*") => Ok(Some(Command::SaslCancel)),
            [item] => {
                let bytes = item
                    .as_bytes()
                    .ok_or(DecodeError::Malformed("SASL response"))?;
                Ok(Some(Command::SaslResponse(sasl_data(bytes)?)))
            }
            _ => Err(DecodeError::Malformed("SASL response")),
        }
    }
}

fn sasl_data(base64: &[u8]) -> Result<SaslData, DecodeError> {
    let bytes = BASE64
        .decode(base64)
        .map_err(|_| DecodeError::Malformed("bad base64"))?;
    Ok(SaslData(Zeroizing::new(bytes)))
}

fn script_name(item: &Item) -> Result<ScriptName, DecodeError> {
    let bytes = item
        .as_bytes()
        .ok_or(DecodeError::Malformed("script name"))?;
    ScriptName::from_bytes(bytes).map_err(|_| DecodeError::Malformed("script name"))
}

fn content(item: Item) -> Result<Vec<u8>, DecodeError> {
    item.into_bytes()
        .ok_or(DecodeError::Malformed("script content"))
}

fn parse_command(items: Vec<Item>) -> Result<Command, DecodeError> {
    let mut items = items.into_iter();
    let Some(Item::Atom(keyword)) = items.next() else {
        return Err(DecodeError::Malformed("command without a keyword"));
    };
    let args: Vec<Item> = items.collect();
    let keyword = keyword.as_str().to_ascii_uppercase();
    let bad = || DecodeError::Malformed("bad arguments");
    Ok(match (keyword.as_str(), args.as_slice()) {
        ("STARTTLS", []) => Command::StartTls,
        ("LOGOUT", []) => Command::Logout,
        ("CAPABILITY", []) => Command::Capability,
        ("UNAUTHENTICATE", []) => Command::Unauthenticate,
        ("LISTSCRIPTS", []) => Command::ListScripts,
        ("NOOP", []) => Command::Noop { tag: None },
        ("NOOP", [tag]) => Command::Noop {
            tag: Some(tag.as_bytes().ok_or_else(bad)?.to_vec()),
        },
        ("AUTHENTICATE", [mechanism, rest @ ..]) if rest.len() <= 1 => Command::Authenticate {
            mechanism: mechanism
                .as_text()
                .and_then(Mechanism::new)
                .ok_or_else(bad)?,
            initial_response: rest
                .first()
                .map(|item| item.as_bytes().ok_or_else(bad).and_then(sasl_data))
                .transpose()?,
        },
        ("HAVESPACE", [name, size]) => Command::HaveSpace {
            name: script_name(name)?,
            size: size
                .as_atom()
                .and_then(|atom| atom.as_str().parse::<u64>().ok())
                .ok_or_else(bad)?,
        },
        ("PUTSCRIPT", [name, _]) => {
            let name = script_name(name)?;
            let body = args.into_iter().nth(1).ok_or_else(bad)?;
            Command::PutScript {
                name,
                content: content(body)?,
            }
        }
        ("CHECKSCRIPT", [_]) => Command::CheckScript {
            content: content(args.into_iter().next().ok_or_else(bad)?)?,
        },
        ("SETACTIVE", [name]) => Command::SetActive {
            name: match name.as_bytes() {
                Some([]) => None,
                _ => Some(script_name(name)?),
            },
        },
        ("GETSCRIPT", [name]) => Command::GetScript {
            name: script_name(name)?,
        },
        ("DELETESCRIPT", [name]) => Command::DeleteScript {
            name: script_name(name)?,
        },
        ("RENAMESCRIPT", [from, to]) => Command::RenameScript {
            from: script_name(from)?,
            to: script_name(to)?,
        },
        _ => return Err(DecodeError::Malformed("unknown command or arguments")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> ScriptName {
        ScriptName::new(text).unwrap()
    }

    #[test]
    fn encodes_commands() {
        let plain = Command::Authenticate {
            mechanism: Mechanism::plain(),
            initial_response: Some(SaslData::new(Zeroizing::new(b"\0alice\0pw".to_vec()))),
        };
        assert_eq!(
            &*plain.encode(),
            b"AUTHENTICATE \"PLAIN\" \"AGFsaWNlAHB3\"\r\n"
        );
        assert!(!format!("{plain:?}").contains("pw"));
        let put = Command::PutScript {
            name: name("a \"b\""),
            content: b"keep;\r\n".to_vec(),
        };
        assert_eq!(
            &*put.encode(),
            b"PUTSCRIPT \"a \\\"b\\\"\" {7+}\r\nkeep;\r\n\r\n"
        );
        assert_eq!(
            &*Command::SetActive { name: None }.encode(),
            b"SETACTIVE \"\"\r\n"
        );
        assert_eq!(
            &*Command::HaveSpace {
                name: name("x"),
                size: 10
            }
            .encode(),
            b"HAVESPACE \"x\" 10\r\n"
        );
        assert_eq!(&*Command::SaslCancel.encode(), b"\"*\"\r\n");
    }

    #[test]
    fn decodes_what_it_encodes() {
        let commands = [
            Command::StartTls,
            Command::Capability,
            Command::Noop {
                tag: Some(b"t".to_vec()),
            },
            Command::RenameScript {
                from: name("a"),
                to: name("ü"),
            },
            Command::CheckScript {
                content: b"\0\xff\r\n".to_vec(),
            },
            Command::SetActive { name: None },
            Command::SetActive {
                name: Some(name("v")),
            },
            Command::Authenticate {
                mechanism: Mechanism::new("oauthbearer").unwrap(),
                initial_response: None,
            },
        ];
        let mut decoder = CommandDecoder::new(Limits::default());
        for command in &commands {
            decoder.feed(&command.encode());
        }
        for command in commands {
            assert_eq!(decoder.decode_command().unwrap(), Some(command));
        }
        assert_eq!(decoder.decode_command().unwrap(), None);
        decoder.feed(b"\"*\"\r\n\"AQ==\"\r\n");
        assert_eq!(decoder.decode_sasl().unwrap(), Some(Command::SaslCancel));
        assert!(matches!(
            decoder.decode_sasl().unwrap(),
            Some(Command::SaslResponse(data)) if data.expose() == [1]
        ));
    }

    #[test]
    fn rejects_bad_commands() {
        for bad in [
            &b"FOO\r\n"[..],
            b"GETSCRIPT\r\n",
            b"GETSCRIPT \"a\x01\"\r\n",
            b"HAVESPACE \"a\" -1\r\n",
            b"AUTHENTICATE \"PL AIN\"\r\n",
            b"AUTHENTICATE \"PLAIN\" \"!!\"\r\n",
            b"\"x\"\r\n",
        ] {
            let mut decoder = CommandDecoder::new(Limits::default());
            decoder.feed(bad);
            assert!(
                decoder.decode_command().is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
