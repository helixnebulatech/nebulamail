// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Typed, classified errors.
//!
//! No error carries the message, credentials or anything the client sent:
//! only the stage, the server's reply code and the server's (cleaned up)
//! reply text.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use crate::address::{AddressError, RcptTo};

/// Where in the SMTP dialogue something failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Implicit TLS handshake, before the greeting.
    TlsHandshake,
    /// Server greeting and the first `EHLO`.
    Greeting,
    /// `STARTTLS` and the handshake after it.
    StartTls,
    /// `AUTH`.
    Auth,
    /// `MAIL FROM`.
    MailFrom,
    /// `RCPT TO`.
    RcptTo,
    /// The `DATA` command, before the server's 354: nothing of the message
    /// was sent yet.
    Data,
    /// The message was written; waiting for the final reply. A failure here
    /// leaves the outcome unknown: the server may have taken the message.
    DataEnd,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TlsHandshake => "TLS handshake",
            Self::Greeting => "greeting",
            Self::StartTls => "STARTTLS",
            Self::Auth => "AUTH",
            Self::MailFrom => "MAIL FROM",
            Self::RcptTo => "RCPT TO",
            Self::Data => "DATA",
            Self::DataEnd => "end of DATA",
        })
    }
}

/// A three-digit SMTP reply code (RFC 5321 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplyCode(u16);

impl ReplyCode {
    /// `250`: requested action okay, completed.
    pub const OK: Self = Self(250);
    /// `452`: too many recipients (RFC 5321 §4.5.3.1.10), temporary.
    pub const TOO_MANY_RECIPIENTS: Self = Self(452);

    /// Parses a reply code; it must be 200 to 599.
    #[must_use]
    pub const fn new(code: u16) -> Option<Self> {
        if code >= 200 && code <= 599 {
            Some(Self(code))
        } else {
            None
        }
    }

    /// The code as a number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// Whether it is a permanent failure (5xx): retrying won't help.
    #[must_use]
    pub const fn is_permanent(self) -> bool {
        self.0 >= 500
    }
}

impl fmt::Display for ReplyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The human-readable text of a server reply, cut to 300 characters and
/// with control characters removed, so it is safe to log and show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerText(String);

impl ServerText {
    const MAX_CHARS: usize = 300;

    pub(crate) fn new(text: &str) -> Self {
        let clean = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(Self::MAX_CHARS)
            .collect::<String>();
        Self(clean.trim().to_owned())
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The enhanced status code at the start of the text, if any
    /// (RFC 3463 §2: `class.subject.detail`, class 2, 4 or 5, subject and
    /// detail of 1 to 3 digits; RFC 2034 puts it first in the text).
    #[must_use]
    pub fn enhanced_status(&self) -> Option<EnhancedStatus> {
        let first = self.0.split_ascii_whitespace().next()?;
        let mut parts = first.split('.');
        let number = |part: Option<&str>, max_len: usize| {
            part.filter(|p| {
                !p.is_empty() && p.len() <= max_len && p.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|p| p.parse::<u16>().ok())
        };
        let class = number(parts.next(), 1)?;
        let subject = number(parts.next(), 3)?;
        let detail = number(parts.next(), 3)?;
        if parts.next().is_some() || !matches!(class, 2 | 4 | 5) {
            return None;
        }
        Some(EnhancedStatus {
            class,
            subject,
            detail,
        })
    }
}

/// An enhanced mail system status code (RFC 3463), such as `5.3.4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnhancedStatus {
    /// 2 success, 4 persistent transient failure, 5 permanent failure.
    pub class: u16,
    /// What it is about (RFC 3463 §3: 1 address, 2 mailbox, 3 mail
    /// system, 4 network, 5 protocol, 6 content, 7 security).
    pub subject: u16,
    /// The detail within the subject.
    pub detail: u16,
}

impl EnhancedStatus {
    /// "Message length exceeds administrative limit" (X.2.3) or "message
    /// too big for system" (X.3.4), RFC 3463 §3.3 and §3.4.
    #[must_use]
    pub const fn is_too_large(self) -> bool {
        matches!((self.subject, self.detail), (2, 3) | (3, 4))
    }
}

impl fmt::Display for ServerText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A recipient the server refused at `RCPT TO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientRejection {
    /// The refused recipient.
    pub recipient: RcptTo,
    /// The reply code (4xx: try later, 5xx: permanent).
    pub code: ReplyCode,
    /// The server's explanation.
    pub text: ServerText,
}

/// Everything that can go wrong while submitting mail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SmtpError {
    // Configuration and input.
    /// The host name can't be used for TLS (SNI and certificate check).
    #[error("invalid TLS host name")]
    InvalidHostName,
    /// The `EHLO` name is not a valid domain name.
    #[error("invalid EHLO name")]
    InvalidHelloName,
    /// The user name or password is empty or has characters SASL can't
    /// carry (NUL, CR, LF).
    #[error("invalid credentials")]
    InvalidCredentials,
    /// A mailbox address was rejected.
    #[error(transparent)]
    Address(#[from] AddressError),
    /// The envelope has no recipients, or too many.
    #[error("an envelope needs 1 to {max} recipients")]
    RecipientCount {
        /// The maximum.
        max: usize,
    },
    /// The message is empty.
    #[error("message is empty")]
    EmptyMessage,
    /// The TLS settings could not be built (no system certificates, say).
    #[error("could not set up TLS")]
    TlsSetup,

    // TLS.
    /// Certificate checks are off, which is only allowed for a loopback
    /// peer, and the peer is not one.
    #[error("certificate checks can only be skipped for loopback servers, not {peer}")]
    InsecureTlsNotLoopback {
        /// The peer address.
        peer: IpAddr,
    },
    /// The server doesn't offer STARTTLS: we never continue in clear.
    #[error("the server does not offer STARTTLS")]
    TlsUnavailable,
    /// The TLS handshake failed (bad certificate, wrong host name...).
    #[error("TLS failed during {stage}")]
    Tls {
        /// Implicit TLS or STARTTLS.
        stage: Stage,
    },

    // Connection.
    /// Reading or writing the connection failed, or the server closed it.
    #[error("network error during {stage}")]
    Network {
        /// Where it happened.
        stage: Stage,
    },
    /// The server took too long to answer.
    #[error("timed out after {after:?} during {stage}")]
    Timeout {
        /// Where it happened.
        stage: Stage,
        /// The timeout that ran out.
        after: Duration,
    },
    /// The server sent something that isn't a valid SMTP reply.
    #[error("invalid server reply during {stage}")]
    Protocol {
        /// Where it happened.
        stage: Stage,
    },

    // Server answers.
    /// The server offers neither `AUTH PLAIN` nor `AUTH LOGIN`.
    #[error("the server offers no supported AUTH mechanism")]
    AuthUnavailable,
    /// The server refused the credentials (or failed to check them, for 4xx).
    #[error("authentication failed ({code}): {text}")]
    Auth {
        /// The reply code; 535 is "bad credentials".
        code: ReplyCode,
        /// The server's explanation.
        text: ServerText,
    },
    /// The envelope has non-ASCII addresses and the server lacks SMTPUTF8.
    #[error("the server does not support SMTPUTF8")]
    SmtpUtf8Unsupported,
    /// The message has UTF-8 header fields (RFC 6532) and the server lacks
    /// SMTPUTF8 (RFC 6531 §3.2: it can't be sent there).
    #[error("the server does not support SMTPUTF8, needed by the header fields")]
    SmtpUtf8HeadersUnsupported,
    /// Bigger than the fixed maximum the server announced with `SIZE`
    /// (RFC 1870 §6): nothing was sent.
    #[error("the message is bigger than the server's maximum of {maximum} bytes")]
    TooLargeForServer {
        /// The server's maximum, in bytes.
        maximum: usize,
    },
    /// The message has 8-bit bytes and the server lacks 8BITMIME.
    #[error("the server does not support 8BITMIME")]
    EightBitMimeUnsupported,
    /// DSN parameters were asked for and the server lacks DSN (RFC 3461).
    #[error("the server does not support DSN")]
    DsnUnsupported,
    /// The server refused one or more recipients. Nothing was sent: the
    /// transaction was cancelled so no one gets a partial send.
    #[error("{} recipient(s) rejected", rejected.len())]
    RecipientsRejected {
        /// Every refused recipient.
        rejected: Vec<RecipientRejection>,
    },
    /// The server refused a command.
    #[error("rejected during {stage} ({code}): {text}")]
    Rejected {
        /// Where it happened.
        stage: Stage,
        /// 5xx: permanent; 4xx: try again later.
        permanent: bool,
        /// The reply code.
        code: ReplyCode,
        /// The server's explanation.
        text: ServerText,
    },
}

impl SmtpError {
    /// Whether trying again later might work (timeouts, network errors and
    /// 4xx replies). Never for an [unknown outcome](Self::outcome_unknown):
    /// trying again could send the message twice.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Network { stage } | Self::Timeout { stage, .. } => *stage != Stage::DataEnd,
            Self::Auth { code, .. } => !code.is_permanent(),
            Self::Rejected { permanent, .. } => !permanent,
            Self::RecipientsRejected { rejected } => rejected
                .iter()
                .all(|rejection| !rejection.code.is_permanent()),
            _ => false,
        }
    }

    /// Whether the server refused the message for its size: the enhanced
    /// code X.2.3 or X.3.4 (RFC 3463), or, without one, a `552` to
    /// `MAIL FROM` (RFC 1870 §6.1, over a fixed maximum) or after the
    /// message (RFC 5321 §4.2.3: exceeded storage allocation). Also our own
    /// check against the announced `SIZE`. Never `534` (RFC 4954: that is
    /// an authentication mechanism too weak).
    #[must_use]
    pub fn is_too_large(&self) -> bool {
        match self {
            Self::TooLargeForServer { .. } => true,
            Self::Rejected {
                stage, code, text, ..
            } => match text.enhanced_status() {
                Some(status) => status.is_too_large(),
                None => code.get() == 552 && matches!(stage, Stage::MailFrom | Stage::DataEnd),
            },
            _ => false,
        }
    }

    /// Whether the message may have been sent: the connection failed, timed
    /// out or got garbage after the message was written and before the
    /// server's final reply ([`Stage::DataEnd`]). Don't send it again
    /// without checking.
    #[must_use]
    pub fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            Self::Network {
                stage: Stage::DataEnd
            } | Self::Timeout {
                stage: Stage::DataEnd,
                ..
            } | Self::Protocol {
                stage: Stage::DataEnd
            }
        )
    }

    /// Classifies an error from lettre at `stage`.
    pub(crate) fn from_lettre(stage: Stage, error: &lettre::transport::smtp::Error) -> Self {
        if let Some((code, text)) = reply(error) {
            return Self::Rejected {
                stage,
                permanent: code.is_permanent(),
                code,
                text,
            };
        }
        if error.is_tls() {
            return Self::Tls { stage };
        }
        // The server sent more than a session ever needs (`ReadBudget`):
        // garbage, not a network failure.
        if error.is_response() || crate::core::budget::received_too_much(error) {
            return Self::Protocol { stage };
        }
        // Network, connection, client and shutdown errors: the connection is
        // no longer usable.
        Self::Network { stage }
    }
}

/// The reply code and text of a negative server reply, if `error` is one.
pub(crate) fn reply(error: &lettre::transport::smtp::Error) -> Option<(ReplyCode, ServerText)> {
    let code = error.status()?;
    let code = code
        .to_string()
        .parse::<u16>()
        .ok()
        .and_then(ReplyCode::new)?;
    let text = std::error::Error::source(error)
        .map(|source| ServerText::new(&source.to_string()))
        .unwrap_or_default();
    Some((code, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_codes() {
        assert!(ReplyCode::new(199).is_none());
        assert!(ReplyCode::new(600).is_none());
        assert!(!ReplyCode::new(451).unwrap().is_permanent());
        assert!(ReplyCode::new(550).unwrap().is_permanent());
    }

    #[test]
    fn server_text_is_cleaned() {
        let text = ServerText::new("5.1.1 no\r\nsuch\u{1b}[31m user ");
        assert_eq!(text.as_str(), "5.1.1 no  such [31m user");
        assert_eq!(ServerText::new(&"x".repeat(1000)).as_str().len(), 300);
    }

    #[test]
    fn transient_classification() {
        let code = |n| ReplyCode::new(n).unwrap();
        let rejected = |n| SmtpError::Rejected {
            stage: Stage::MailFrom,
            permanent: code(n).is_permanent(),
            code: code(n),
            text: ServerText::default(),
        };
        assert!(rejected(451).is_transient());
        assert!(!rejected(550).is_transient());
        assert!(
            SmtpError::Timeout {
                stage: Stage::Data,
                after: Duration::from_secs(1)
            }
            .is_transient()
        );
        assert!(!SmtpError::TlsUnavailable.is_transient());
        assert!(SmtpError::Network { stage: Stage::Data }.is_transient());
    }

    #[test]
    fn data_end_failures_have_an_unknown_outcome() {
        let after = Duration::from_secs(1);
        for error in [
            SmtpError::Timeout {
                stage: Stage::DataEnd,
                after,
            },
            SmtpError::Network {
                stage: Stage::DataEnd,
            },
            SmtpError::Protocol {
                stage: Stage::DataEnd,
            },
        ] {
            assert!(!error.is_transient(), "{error:?}");
            assert!(error.outcome_unknown(), "{error:?}");
        }
        let before = SmtpError::Timeout {
            stage: Stage::Data,
            after,
        };
        assert!(before.is_transient());
        assert!(!before.outcome_unknown());
        // A clear reply is not unknown.
        let code = ReplyCode::new(451).unwrap();
        let rejected = SmtpError::Rejected {
            stage: Stage::DataEnd,
            permanent: false,
            code,
            text: ServerText::default(),
        };
        assert!(rejected.is_transient());
        assert!(!rejected.outcome_unknown());
    }

    fn rejected(stage: Stage, code: u16, text: &str) -> SmtpError {
        SmtpError::Rejected {
            stage,
            permanent: code >= 500,
            code: ReplyCode::new(code).unwrap(),
            text: ServerText::new(text),
        }
    }

    #[test]
    fn enhanced_status_codes() {
        let status = ServerText::new("5.3.4 Message too big")
            .enhanced_status()
            .unwrap();
        assert_eq!((status.class, status.subject, status.detail), (5, 3, 4));
        assert!(status.is_too_large());
        assert!(
            ServerText::new("4.2.2 Mailbox full")
                .enhanced_status()
                .is_some()
        );
        // Not one: wrong class, too many digits, no detail, words.
        for text in [
            "3.1.1 x",
            "5.1234.1 x",
            "5.1 x",
            "Message too big",
            "",
            "5.1.1.1 x",
        ] {
            assert!(ServerText::new(text).enhanced_status().is_none(), "{text}");
        }
    }

    #[test]
    fn too_large_by_the_standards() {
        // RFC 3463: X.3.4 and X.2.3, whatever the basic code.
        assert!(rejected(Stage::DataEnd, 554, "5.3.4 Message too big").is_too_large());
        assert!(rejected(Stage::DataEnd, 552, "5.2.3 Too long").is_too_large());
        // A full mailbox is not the message's size.
        assert!(!rejected(Stage::DataEnd, 552, "5.2.2 Mailbox full").is_too_large());
        // Without an enhanced code: 552 to MAIL FROM (RFC 1870) or after
        // the message (RFC 5321 §4.2.3).
        assert!(rejected(Stage::MailFrom, 552, "Too big").is_too_large());
        assert!(rejected(Stage::DataEnd, 552, "Too big").is_too_large());
        assert!(!rejected(Stage::RcptTo, 552, "Too many").is_too_large());
        // 534 is an authentication mechanism too weak (RFC 4954).
        assert!(!rejected(Stage::DataEnd, 534, "Too weak").is_too_large());
        assert!(SmtpError::TooLargeForServer { maximum: 10 }.is_too_large());
    }
}
