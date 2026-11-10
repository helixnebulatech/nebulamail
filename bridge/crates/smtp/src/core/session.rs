// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The client and its session states. The operations are in `op/`.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use lettre::transport::smtp::client::AsyncSmtpConnection;

use crate::core::activity::Activity;
use crate::core::config::{SmtpServer, Timeouts};
use crate::core::tls::TlsSetup;
use crate::dsn::ServerExtensions;
use crate::error::{ReplyCode, ServerText, SmtpError, Stage};

/// Sends mail through one configured server. Cheap to clone; build it once
/// (it loads the system trust store) and use it for every connection.
#[derive(Debug, Clone)]
pub struct SmtpClient {
    pub(crate) server: SmtpServer,
    pub(crate) tls: TlsSetup,
}

impl SmtpClient {
    /// Prepares the TLS settings for `server`.
    ///
    /// # Errors
    ///
    /// [`SmtpError::TlsSetup`] if they can't be built.
    pub fn new(server: SmtpServer) -> Result<Self, SmtpError> {
        let tls = TlsSetup::new(&server)?;
        Ok(Self { server, tls })
    }

    /// The server this client talks to.
    #[must_use]
    pub const fn server(&self) -> &SmtpServer {
        &self.server
    }
}

/// A connection with TLS up and `EHLO` done, not yet authenticated.
///
/// The only way to get one is [`SmtpClient::connect`], so anything done with
/// it (above all `AUTH`) is encrypted.
pub struct SecureSession {
    pub(crate) conn: AsyncSmtpConnection,
    pub(crate) timeouts: Timeouts,
    pub(crate) extensions: ServerExtensions,
    /// What the socket does, for the data timeouts.
    pub(crate) activity: Activity,
}

/// An authenticated connection that can send messages.
pub struct AuthenticatedSession {
    pub(crate) conn: AsyncSmtpConnection,
    pub(crate) timeouts: Timeouts,
    pub(crate) extensions: ServerExtensions,
    pub(crate) activity: Activity,
    /// Set after a network error or timeout: the dialogue is out of step.
    pub(crate) broken: bool,
}

impl SecureSession {
    /// The extensions the server announced over TLS.
    #[must_use]
    pub const fn extensions(&self) -> &ServerExtensions {
        &self.extensions
    }
}

impl AuthenticatedSession {
    /// The extensions the server announced over TLS.
    #[must_use]
    pub const fn extensions(&self) -> &ServerExtensions {
        &self.extensions
    }

    /// Whether a message with 8-bit content can be sent on this connection
    /// (8BITMIME): the same check [`send`](Self::send) makes before
    /// `MAIL FROM`, which fails with
    /// [`SmtpError::EightBitMimeUnsupported`](crate::SmtpError) without it.
    #[must_use]
    pub fn accepts_8bit(&self) -> bool {
        self.conn
            .server_info()
            .supports_feature(lettre::transport::smtp::extension::Extension::EightBitMime)
    }
}

impl fmt::Debug for SecureSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureSession").finish_non_exhaustive()
    }
}

impl fmt::Debug for AuthenticatedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthenticatedSession")
            .field("broken", &self.broken)
            .finish_non_exhaustive()
    }
}

/// How long `QUIT` may take (RFC 5321 gives it no time; its reply changes
/// nothing, so nobody waits long for it).
pub(crate) const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Ends a session with `QUIT` before dropping it: RFC 5321 §4.1.1.10
/// says the client MUST NOT close the channel on purpose without it (also
/// after a refused login or a missing extension). Not on a broken
/// connection, which has nothing to say any more.
pub(crate) async fn quit(conn: &mut AsyncSmtpConnection) {
    if !conn.has_broken() {
        let _ = timed(Stage::Data, QUIT_TIMEOUT, conn.quit()).await;
    }
}

impl SecureSession {
    /// Ends the session politely with `QUIT` (when it was only opened to
    /// read the server's extensions).
    pub async fn quit(mut self) {
        quit(&mut self.conn).await;
    }
}

/// Runs `future` with a timeout. The outer error is the timeout; the inner
/// result is lettre's, for the caller to classify.
pub(crate) async fn timed<T, E>(
    stage: Stage,
    after: Duration,
    future: impl Future<Output = Result<T, E>>,
) -> Result<Result<T, E>, SmtpError> {
    tokio::time::timeout(after, future)
        .await
        .map_err(|_| SmtpError::Timeout { stage, after })
}

/// A positive server reply, such as `250 2.0.0 Ok: queued as 1A2B3C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerReply {
    /// The reply code.
    pub code: ReplyCode,
    /// The reply text.
    pub text: ServerText,
}

impl ServerReply {
    pub(crate) fn from_lettre(response: &lettre::transport::smtp::response::Response) -> Self {
        let code = response
            .code()
            .to_string()
            .parse::<u16>()
            .ok()
            .and_then(ReplyCode::new)
            .unwrap_or(ReplyCode::OK);
        let text = response.message().collect::<Vec<_>>().join(" ");
        Self {
            code,
            text: ServerText::new(&text),
        }
    }
}
