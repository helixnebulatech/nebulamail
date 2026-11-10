// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Greeting, STARTTLS and the first CAPABILITY.

use nebula_imap_proto::command::{self, EncodeOptions};
use nebula_imap_proto::response::{Condition, Response, ResponseCode, StatusResponse, Untagged};
use nebula_imap_proto::types::{Capabilities, Capability};
use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::core::client::{Authenticated, Client, Connected, NotAuthenticated, Session};
use crate::core::connection::{
    Connection, Deadline, SecureStream, StrayContinuation, after, expect_ok,
};
use crate::{Error, Limits, Tls, TlsConnector};

impl Client<NotAuthenticated> {
    /// Starts an IMAP session over a TCP stream the caller opened (through
    /// `net-guard`, so it is pinned to a validated IP).
    ///
    /// With [`Tls::StartTls`], STARTTLS is mandatory: if the server does not
    /// offer it, or greets with PREAUTH before TLS, this fails. Capabilities
    /// seen before TLS are discarded and asked again over TLS.
    ///
    /// # Errors
    ///
    /// Network, TLS or protocol errors, a BYE greeting, or a server that is
    /// not IMAP4rev1/rev2.
    pub async fn connect(
        stream: TcpStream,
        server_name: ServerName<'static>,
        tls: Tls,
        connector: &TlsConnector,
        limits: Limits,
    ) -> Result<Connected, Error> {
        let deadline = Deadline::At(after(limits.greeting_timeout));
        let (connection, greeting) = match tls {
            Tls::Implicit => {
                let secure = tls_handshake(connector, stream, server_name, deadline).await?;
                let mut connection = Connection::new(secure, limits);
                let greeting = read_greeting(&mut connection, deadline).await?;
                (connection, greeting)
            }
            Tls::StartTls => start_tls(stream, server_name, connector, limits, deadline).await?,
        };
        finish_greeting(connection, greeting).await
    }
}

async fn tls_handshake(
    connector: &TlsConnector,
    stream: TcpStream,
    server_name: ServerName<'static>,
    deadline: Deadline,
) -> Result<SecureStream, Error> {
    let (Deadline::At(until) | Deadline::Command(until)) = deadline;
    let handshake = connector.handshake(stream, server_name);
    let stream = tokio::time::timeout_at(until, handshake)
        .await
        .map_err(|_| Error::Timeout("TLS handshake"))??;
    Ok(SecureStream::Tls(Box::new(stream)))
}

/// Plain greeting, STARTTLS, TLS handshake. Returns the TLS connection and
/// a synthetic OK greeting (the real one was before TLS).
async fn start_tls(
    stream: TcpStream,
    server_name: ServerName<'static>,
    connector: &TlsConnector,
    limits: Limits,
    deadline: Deadline,
) -> Result<(Connection<SecureStream>, StatusResponse), Error> {
    let mut plain = Connection::new(stream, limits);
    let greeting = read_greeting(&mut plain, deadline).await?;
    match greeting.condition {
        Condition::Ok => {}
        Condition::PreAuth => return Err(Error::PreAuthOnPlaintext),
        _ => return Err(Error::Protocol("unexpected greeting")),
    }
    let capabilities = match greeting.code {
        Some(ResponseCode::Capabilities(capabilities)) => capabilities,
        _ => fetch_capabilities(&mut plain).await?,
    };
    if !capabilities.has(&Capability::StartTls) {
        return Err(Error::StartTlsUnavailable);
    }
    let tagged = plain
        .run(
            &command::StartTls,
            EncodeOptions::basic(),
            StrayContinuation::Reject,
            &mut |_| {},
        )
        .await?;
    expect_ok("STARTTLS", tagged).map_err(|_| Error::StartTlsUnavailable)?;
    // Anything already buffered was sent before TLS and could come from a
    // man in the middle (CVE-2011-0411 class): refuse it.
    if plain.decoder().buffered() > 0 {
        return Err(Error::InjectedBeforeTls);
    }
    let secure = tls_handshake(connector, plain.into_stream(), server_name, deadline).await?;
    let connection = Connection::new(secure, limits);
    // Pre-TLS capabilities are not trusted: `finish_greeting` asks again.
    let greeting = StatusResponse {
        condition: Condition::Ok,
        code: None,
        text: String::new(),
    };
    Ok((connection, greeting))
}

/// Reads the greeting: `* OK`, `* PREAUTH` or `* BYE`.
async fn read_greeting<S: AsyncRead + AsyncWrite + Unpin>(
    connection: &mut Connection<S>,
    deadline: Deadline,
) -> Result<StatusResponse, Error> {
    match connection.read_response(deadline, "greeting").await? {
        Response::Untagged(Untagged::Status(status)) => match status.condition {
            Condition::Ok | Condition::PreAuth => Ok(status),
            Condition::Bye => Err(Error::Bye(status.text)),
            _ => Err(Error::Protocol("unexpected greeting")),
        },
        _ => Err(Error::Protocol("unexpected greeting")),
    }
}

/// `CAPABILITY` on any connection.
pub(crate) async fn fetch_capabilities<S: AsyncRead + AsyncWrite + Unpin>(
    connection: &mut Connection<S>,
) -> Result<Capabilities, Error> {
    let mut found: Option<Capabilities> = None;
    let tagged = connection
        .run(
            &command::Capability,
            EncodeOptions::basic(),
            StrayContinuation::Reject,
            &mut |data| {
                if let Untagged::Capabilities(capabilities) = data {
                    found = Some(capabilities);
                }
            },
        )
        .await?;
    let tagged = expect_ok("CAPABILITY", tagged)?;
    match (found, tagged.code) {
        (Some(capabilities), _) | (None, Some(ResponseCode::Capabilities(capabilities))) => {
            Ok(capabilities)
        }
        _ => Err(Error::Protocol("CAPABILITY without capabilities")),
    }
}

async fn finish_greeting(
    connection: Connection<SecureStream>,
    greeting: StatusResponse,
) -> Result<Connected, Error> {
    let max_events = connection.limits.max_events;
    let mut client = Client {
        connection: Box::new(connection),
        session: Session::new(Capabilities::default(), max_events),
        state: NotAuthenticated,
    };
    // An `[ALERT]` in the greeting (only after TLS: `start_tls` replaces
    // the plaintext one, which anyone on the path could have written).
    if greeting.code == Some(ResponseCode::Alert) {
        client.session.alerts.push(&greeting.text);
    }
    match greeting.code {
        Some(ResponseCode::Capabilities(capabilities)) => {
            client.session.capabilities = capabilities;
        }
        _ => {
            client.refresh_capabilities().await?;
        }
    }
    if !client.session.capabilities.is_imap4() {
        return Err(Error::NotImap4);
    }
    if greeting.condition == Condition::PreAuth {
        let mut client: Client<Authenticated> = client.into_state(Authenticated);
        client.after_authentication(None).await?;
        return Ok(Connected::PreAuthenticated(client));
    }
    Ok(Connected::NotAuthenticated(client))
}
