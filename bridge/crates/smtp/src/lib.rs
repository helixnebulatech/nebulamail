// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Mail submission.
//!
//! lettre is used only as the SMTP transport: the message is built elsewhere
//! (mail-builder) and sent here as raw bytes. The caller opens the TCP
//! connection (later through `net-guard`, pinned to a validated IP) and
//! passes the connected [`tokio::net::TcpStream`]; nothing here resolves DNS.
//!
//! The types make the unsafe paths impossible:
//!
//! - [`SmtpClient::connect`] returns a [`SecureSession`] only after TLS is up,
//!   either implicit TLS (port 465) or STARTTLS, which is **required**: if the
//!   server doesn't offer it, [`SmtpError::TlsUnavailable`].
//! - Only a [`SecureSession`] can authenticate, so `AUTH` never goes in clear,
//!   and only an [`AuthenticatedSession`] can send.
//! - Skipping certificate checks ([`CertificateCheck::InsecureLoopbackOnly`])
//!   works only when the peer is a loopback address.
//!
//! ```no_run
//! # async fn example(stream: tokio::net::TcpStream) -> Result<(), nebula_smtp::SmtpError> {
//! use secrecy::SecretString;
//! use nebula_smtp::*;
//!
//! let server = SmtpServer::new("smtp.example.com".parse()?, Security::ImplicitTls);
//! let client = SmtpClient::new(server)?;
//! let credentials = Credentials::new("alice", SecretString::from("password"))?;
//! let envelope = Envelope::new("alice@example.com".parse()?, vec!["bob@example.org".parse()?])?;
//! let message = OutgoingMessage::new(b"From: alice@example.com\r\nBcc: carol@example.net\r\n\r\nHi\r\n")?;
//!
//! let mut session = client.connect(stream).await?.authenticate(&credentials).await?;
//! session.send(&envelope, &message).await?;
//! session.quit().await;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod address;
mod core;
mod dsn;
mod error;
mod message;
mod op;

pub use self::core::config::{
    CertificateCheck, Credentials, HelloName, Security, SmtpServer, Timeouts, TlsHost,
};
pub use self::core::session::{AuthenticatedSession, SecureSession, ServerReply, SmtpClient};
pub use address::{AddressError, Envelope, MailFrom, RcptTo};
pub use dsn::{
    DsnError, DsnNotify, DsnReturn, EnvelopeId, MailDsn, OriginalRecipient, RandomError, RcptDsn,
    ServerExtensions,
};
pub use error::{EnhancedStatus, RecipientRejection, ReplyCode, ServerText, SmtpError, Stage};
pub use message::{HeaderField, OutgoingMessage, normalize_line_endings, remove_bcc};
pub use op::Sent;
