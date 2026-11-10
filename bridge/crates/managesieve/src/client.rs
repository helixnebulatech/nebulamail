// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The typestate client: greeting and STARTTLS, authentication, and the
//! script commands.

use nebula_managesieve_proto::{
    Capabilities, Command, ListedScript, Mechanism, ResponseCode, SaslData, ScriptName, ServerLine,
    Status,
};
use nebula_tls::TlsConnector;
use rustls_pki_types::ServerName;
use secrecy::{ExposeSecret as _, SecretString};
use tokio::net::TcpStream;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::connection::{Connection, Reply, SecureStream, after};
use crate::{Error, Limits, Refusal, Tls};

/// Not logged in yet: only authentication is possible.
#[derive(Debug)]
pub struct NotAuthenticated;

/// Logged in: script commands are available.
#[derive(Debug)]
pub struct Authenticated;

/// An OAuth SASL mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OAuthMechanism {
    /// `XOAUTH2` (Gmail, Outlook).
    XOAuth2,
    /// `OAUTHBEARER` (RFC 7628).
    OAuthBearer,
}

/// What `CHECKSCRIPT` said about a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Valid, maybe with warnings (the server's text).
    Valid {
        /// The warnings, if any.
        warnings: Option<String>,
    },
    /// Invalid: the server's error text (usually with a line number).
    Invalid(String),
}

/// A ManageSieve connection in state `S`.
pub struct Client<S> {
    connection: Box<Connection<SecureStream>>,
    capabilities: Capabilities,
    #[expect(dead_code, reason = "typestate marker")]
    state: S,
}

impl<S> std::fmt::Debug for Client<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &std::any::type_name::<S>())
            .field("connection", &self.connection)
            .finish_non_exhaustive()
    }
}

impl<S> Client<S> {
    /// What the server announced (re-read after STARTTLS and login).
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// An earlier error left the connection unusable.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.connection.is_broken()
    }

    /// Marks the connection broken if `result` is a fatal error.
    fn track<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.connection.mark_broken();
        }
        result
    }

    fn into_state<T>(self, state: T) -> Client<T> {
        Client {
            connection: self.connection,
            capabilities: self.capabilities,
            state,
        }
    }

    async fn run(&mut self, command: &Command) -> Result<Reply, Error> {
        self.connection.run(command).await
    }

    async fn run_ok(&mut self, command: &Command) -> Result<Reply, Error> {
        let reply = self.run(command).await?;
        expect_ok(command.name(), reply)
    }

    async fn refresh_capabilities(&mut self) -> Result<(), Error> {
        let reply = self.run_ok(&Command::Capability).await?;
        let parsed = Capabilities::parse(&reply.data).map_err(Error::from);
        self.capabilities = self.track(parsed)?;
        Ok(())
    }

    /// Ends the session politely (errors are ignored: the connection is
    /// dropped anyway).
    pub async fn logout(mut self) {
        if !self.connection.is_broken() {
            let _ = self.run(&Command::Logout).await;
        }
    }
}

fn refusal(command: &'static str, reply: Reply) -> Error {
    Error::Refused(Refusal {
        command,
        text: reply.done.text_lossy(),
        code: reply.done.code,
    })
}

fn expect_ok(command: &'static str, reply: Reply) -> Result<Reply, Error> {
    if reply.done.status == Status::Ok {
        Ok(reply)
    } else {
        Err(refusal(command, reply))
    }
}

/// Reads the capability response that starts a session (the greeting, or
/// the one re-sent after STARTTLS).
async fn read_capabilities<S>(
    connection: &mut Connection<S>,
    deadline: Instant,
    during: &'static str,
) -> Result<Capabilities, Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let reply = connection.read_reply(deadline, during).await?;
    if reply.done.status != Status::Ok {
        return Err(Error::Protocol("the server refused the session"));
    }
    Ok(Capabilities::parse(&reply.data)?)
}

async fn tls_handshake(
    connector: &TlsConnector,
    stream: TcpStream,
    server_name: ServerName<'static>,
    deadline: Instant,
) -> Result<SecureStream, Error> {
    let handshake = connector.handshake(stream, server_name);
    let stream = tokio::time::timeout_at(deadline, handshake)
        .await
        .map_err(|_| Error::Timeout("TLS handshake"))?
        .map_err(|error| match error {
            nebula_tls::TlsError::InsecureNotLoopback => Error::InsecureTlsNotLoopback,
            nebula_tls::TlsError::InvalidServerName => Error::InvalidServerName,
            nebula_tls::TlsError::Config(error) => Error::TlsConfig(error),
            nebula_tls::TlsError::Io(error) => Error::Io(error),
            other => Error::Io(std::io::Error::other(other)),
        })?;
    Ok(SecureStream::Tls(Box::new(stream)))
}

impl Client<NotAuthenticated> {
    /// Starts a session over a TCP stream the caller opened (through
    /// `net-guard`, so it is pinned to a validated IP).
    ///
    /// With [`Tls::StartTls`], STARTTLS is mandatory: if the server does
    /// not offer it, this fails. Capabilities seen before TLS are thrown
    /// away; the server re-sends them over TLS (RFC 5804 §2.2).
    ///
    /// # Errors
    ///
    /// Network, TLS or protocol errors, a `BYE` greeting, STARTTLS missing.
    pub async fn connect(
        stream: TcpStream,
        server_name: ServerName<'static>,
        tls: Tls,
        connector: &TlsConnector,
        limits: Limits,
    ) -> Result<Self, Error> {
        let deadline = after(limits.greeting_timeout);
        let secure = match tls {
            Tls::Implicit => tls_handshake(connector, stream, server_name, deadline).await?,
            Tls::StartTls => {
                // In plaintext, anyone in the middle writes: small limits.
                let mut plain = Connection::new(stream, limits.before_tls());
                let before = read_capabilities(&mut plain, deadline, "greeting").await?;
                if !before.starttls {
                    return Err(Error::StartTlsUnavailable);
                }
                let reply = plain.run(&Command::StartTls).await?;
                if reply.done.status != Status::Ok || !reply.data.is_empty() {
                    return Err(Error::StartTlsUnavailable);
                }
                // Anything already buffered was sent before TLS and could
                // come from a man in the middle (CVE-2011-0411 class).
                if plain.buffered() > 0 {
                    return Err(Error::InjectedBeforeTls);
                }
                tls_handshake(connector, plain.into_stream(), server_name, deadline).await?
            }
        };
        let mut connection = Connection::new(secure, limits);
        let during = match tls {
            Tls::Implicit => "greeting",
            Tls::StartTls => "capabilities after STARTTLS",
        };
        let capabilities = read_capabilities(&mut connection, deadline, during).await?;
        Ok(Self {
            connection: Box::new(connection),
            capabilities,
            state: NotAuthenticated,
        })
    }

    /// Logs in with a password (`AUTHENTICATE "PLAIN"` with the initial
    /// response), then reads the capabilities again.
    ///
    /// # Errors
    ///
    /// [`Error::NoAuthMechanism`] without PLAIN, [`Error::AuthFailed`] for
    /// wrong credentials, other errors otherwise.
    pub async fn login(
        self,
        username: &str,
        password: &SecretString,
    ) -> Result<Client<Authenticated>, Error> {
        if !self.capabilities.has_sasl("PLAIN") {
            return Err(Error::NoAuthMechanism);
        }
        if username.contains('\0') || password.expose_secret().contains('\0') {
            return Err(Error::AuthFailed(
                "invalid user name or password".to_owned(),
            ));
        }
        // RFC 4616: authzid NUL authcid NUL passwd (no authzid).
        let password = password.expose_secret().as_bytes();
        let mut data = Zeroizing::new(Vec::with_capacity(
            username
                .len()
                .saturating_add(password.len())
                .saturating_add(2),
        ));
        data.push(0);
        data.extend_from_slice(username.as_bytes());
        data.push(0);
        data.extend_from_slice(password);
        self.authenticate(Mechanism::plain(), SaslData::new(data))
            .await
    }

    /// Logs in with an OAuth access token.
    ///
    /// # Errors
    ///
    /// As [`Self::login`]; [`Error::NoAuthMechanism`] if the server does not
    /// offer the mechanism.
    pub async fn login_oauth(
        self,
        username: &str,
        token: &SecretString,
        mechanism: OAuthMechanism,
    ) -> Result<Client<Authenticated>, Error> {
        let name = match mechanism {
            OAuthMechanism::XOAuth2 => "XOAUTH2",
            OAuthMechanism::OAuthBearer => "OAUTHBEARER",
        };
        if !self.capabilities.has_sasl(name) {
            return Err(Error::NoAuthMechanism);
        }
        if username.contains(['\u{1}', ',']) || token.expose_secret().contains('\u{1}') {
            return Err(Error::AuthFailed("invalid user name or token".to_owned()));
        }
        let token = token.expose_secret();
        let text = Zeroizing::new(match mechanism {
            // https://developers.google.com/gmail/imap/xoauth2-protocol
            OAuthMechanism::XOAuth2 => format!("user={username}\u{1}auth=Bearer {token}\u{1}\u{1}"),
            // RFC 7628 §3.1 (`=` and `,` in the name are escaped as =3D, =2C;
            // a comma was refused above).
            OAuthMechanism::OAuthBearer => format!(
                "n,a={},\u{1}auth=Bearer {token}\u{1}\u{1}",
                username.replace('=', "=3D")
            ),
        });
        let data = SaslData::new(Zeroizing::new(text.as_bytes().to_vec()));
        let mechanism = Mechanism::new(name).ok_or(Error::NoAuthMechanism)?;
        self.authenticate(mechanism, data).await
    }

    async fn authenticate(
        mut self,
        mechanism: Mechanism,
        initial: SaslData,
    ) -> Result<Client<Authenticated>, Error> {
        let mechanism_name = mechanism.as_str().to_owned();
        let command = Command::Authenticate {
            mechanism,
            initial_response: Some(initial),
        };
        let deadline = self.connection.command_deadline();
        tracing::debug!(command = command.name(), "ManageSieve command");
        let bytes = command.encode();
        drop(command);
        self.connection
            .write(&bytes, deadline, "AUTHENTICATE")
            .await?;
        drop(bytes);
        self.connection.begin_reply();
        // OAUTHBEARER's error challenge is answered with a single %x01 so
        // the server finishes the exchange (RFC 7628 §3.2.3); for other
        // mechanisms the client cancels with "*" (RFC 5804 §2.1). Either way
        // the server then answers NO.
        let oauthbearer = mechanism_name.eq_ignore_ascii_case("OAUTHBEARER");
        let mut cancelled = false;
        let done = loop {
            match self.connection.read_line(deadline, "AUTHENTICATE").await? {
                // A challenge: we sent everything up front, so this is an
                // error report (OAUTHBEARER) or a server that wants more.
                ServerLine::Data(items) if items.len() == 1 && !cancelled => {
                    cancelled = true;
                    let answer = if oauthbearer {
                        Command::SaslResponse(SaslData::new(Zeroizing::new(vec![1])))
                    } else {
                        Command::SaslCancel
                    };
                    self.connection
                        .write(&answer.encode(), deadline, "AUTHENTICATE")
                        .await?;
                }
                ServerLine::Data(_) => {
                    return Err(Error::Protocol("unexpected SASL challenge"));
                }
                ServerLine::Done(done) => break done,
            }
        };
        match (done.status, &done.code) {
            (Status::Ok, _) => {}
            (Status::Bye, _) => return Err(Error::Bye(done.text_one_line())),
            (
                Status::No,
                Some(
                    ResponseCode::EncryptNeeded
                    | ResponseCode::TransitionNeeded
                    | ResponseCode::TryLater
                    | ResponseCode::AuthTooWeak,
                ),
            ) => {
                return Err(Error::Refused(Refusal {
                    command: "AUTHENTICATE",
                    text: done.text_lossy(),
                    code: done.code,
                }));
            }
            (Status::No, _) => return Err(Error::AuthFailed(done.text_one_line())),
        }
        let mut client = self.into_state(Authenticated);
        // Capabilities may change once logged in (RFC 5804 §2.1).
        client.refresh_capabilities().await?;
        tracing::debug!("ManageSieve logged in");
        Ok(client)
    }
}

/// A random name for a script that only lives during a check.
fn scratch_name() -> Result<ScriptName, Error> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| Error::Unsupported("system random generator"))?;
    let hex = bytes.iter().fold(String::new(), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    });
    ScriptName::new(format!("nebula-mail-check-{hex}"))
        .map_err(|_| Error::Protocol("scratch script name"))
}

impl Client<Authenticated> {
    /// Every script, with the active one marked. Names the RFC forbids
    /// (only a broken server would have them) are skipped.
    ///
    /// # Errors
    ///
    /// Network or protocol errors, or a refusal.
    pub async fn list_scripts(&mut self) -> Result<Vec<ListedScript>, Error> {
        let result = self.list_scripts_inner().await;
        self.track(result)
    }

    async fn list_scripts_inner(&mut self) -> Result<Vec<ListedScript>, Error> {
        let reply = self.run_ok(&Command::ListScripts).await?;
        let mut scripts = Vec::with_capacity(reply.data.len());
        let mut active = 0_usize;
        for line in &reply.data {
            match ListedScript::parse(line)? {
                Ok(script) => {
                    active = active.saturating_add(usize::from(script.active));
                    scripts.push(script);
                }
                Err(error) => tracing::warn!(%error, "ManageSieve: skipped a script name"),
            }
        }
        if active > 1 {
            return Err(Error::Protocol("more than one active script"));
        }
        Ok(scripts)
    }

    /// The content of a script, as opaque bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] with `NONEXISTENT` if there is no such script.
    pub async fn get_script(&mut self, name: &ScriptName) -> Result<Vec<u8>, Error> {
        let result = self.get_script_inner(name).await;
        self.track(result)
    }

    async fn get_script_inner(&mut self, name: &ScriptName) -> Result<Vec<u8>, Error> {
        let reply = self
            .run_ok(&Command::GetScript { name: name.clone() })
            .await?;
        let mut lines = reply.data.into_iter();
        match (lines.next(), lines.next()) {
            (Some(line), None) => {
                let mut items = line.into_iter();
                match (items.next(), items.next()) {
                    (Some(item), None) => item
                        .as_bytes()
                        .map(<[u8]>::to_vec)
                        .ok_or(Error::Protocol("GETSCRIPT without a string")),
                    _ => Err(Error::Protocol("GETSCRIPT with extra data")),
                }
            }
            _ => Err(Error::Protocol("GETSCRIPT without exactly one script")),
        }
    }

    /// Stores a script (creating or replacing it). Returns the server's
    /// warnings, if any.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`]: an invalid script (the text says why), or a
    /// `QUOTA` code.
    pub async fn put_script(
        &mut self,
        name: &ScriptName,
        content: &[u8],
    ) -> Result<Option<String>, Error> {
        let reply = self
            .run_ok(&Command::PutScript {
                name: name.clone(),
                content: content.to_vec(),
            })
            .await?;
        Ok(warnings(&reply))
    }

    /// Checks a script without storing it: `CHECKSCRIPT`, or on servers
    /// older than RFC 5804, `PUTSCRIPT` under a random name and
    /// `DELETESCRIPT`.
    ///
    /// # Errors
    ///
    /// Network or protocol errors; a refusal that isn't about the script
    /// (`QUOTA`, `TRYLATER`).
    pub async fn check_script(&mut self, content: &[u8]) -> Result<CheckOutcome, Error> {
        let (command, scratch) = if self.capabilities.is_rfc5804() {
            (
                Command::CheckScript {
                    content: content.to_vec(),
                },
                None,
            )
        } else {
            let name = scratch_name()?;
            (
                Command::PutScript {
                    name: name.clone(),
                    content: content.to_vec(),
                },
                Some(name),
            )
        };
        let reply = self.run(&command).await?;
        if reply.done.status == Status::Ok {
            if let Some(name) = scratch {
                self.delete_script(&name).await?;
            }
            return Ok(CheckOutcome::Valid {
                warnings: warnings(&reply),
            });
        }
        match &reply.done.code {
            None | Some(ResponseCode::Warnings) => {
                Ok(CheckOutcome::Invalid(reply.done.text_lossy()))
            }
            Some(_) => Err(refusal(command.name(), reply)),
        }
    }

    /// Makes `name` the active script, or deactivates every script.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] (`NONEXISTENT`…).
    pub async fn set_active(&mut self, name: Option<&ScriptName>) -> Result<(), Error> {
        self.run_ok(&Command::SetActive {
            name: name.cloned(),
        })
        .await?;
        Ok(())
    }

    /// Deletes a script (servers refuse the active one: `ACTIVE`).
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] (`NONEXISTENT`, `ACTIVE`).
    pub async fn delete_script(&mut self, name: &ScriptName) -> Result<(), Error> {
        self.run_ok(&Command::DeleteScript { name: name.clone() })
            .await?;
        Ok(())
    }

    /// Renames a script (it stays active if it was). On servers older than
    /// RFC 5804: get, put under the new name, activate, delete the old one.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] (`NONEXISTENT`, `ALREADYEXISTS`).
    pub async fn rename_script(&mut self, from: &ScriptName, to: &ScriptName) -> Result<(), Error> {
        if self.capabilities.is_rfc5804() {
            self.run_ok(&Command::RenameScript {
                from: from.clone(),
                to: to.clone(),
            })
            .await?;
            return Ok(());
        }
        let scripts = self.list_scripts().await?;
        let refused = |code: ResponseCode, text: &str| {
            Error::Refused(Refusal {
                command: "RENAMESCRIPT",
                code: Some(code),
                text: text.to_owned(),
            })
        };
        let Some(old) = scripts.iter().find(|script| &script.name == from) else {
            return Err(refused(ResponseCode::Nonexistent, "no such script"));
        };
        if scripts.iter().any(|script| &script.name == to) {
            return Err(refused(ResponseCode::AlreadyExists, "the name is taken"));
        }
        let active = old.active;
        let content = self.get_script(from).await?;
        self.put_script(to, &content).await?;
        if active {
            self.set_active(Some(to)).await?;
        }
        self.delete_script(from).await
    }

    /// Whether a script of `size` bytes named `name` would fit.
    ///
    /// # Errors
    ///
    /// A refusal other than `QUOTA`.
    pub async fn have_space(&mut self, name: &ScriptName, size: u64) -> Result<bool, Error> {
        let command = Command::HaveSpace {
            name: name.clone(),
            size,
        };
        let reply = self.run(&command).await?;
        match (reply.done.status, &reply.done.code) {
            (Status::Ok, _) => Ok(true),
            (_, Some(code)) if code.is_quota() => Ok(false),
            _ => Err(refusal(command.name(), reply)),
        }
    }

    /// Keeps the connection alive (`NOOP`, or `CAPABILITY` on older
    /// servers).
    ///
    /// # Errors
    ///
    /// Network or protocol errors.
    pub async fn noop(&mut self) -> Result<(), Error> {
        let command = if self.capabilities.is_rfc5804() {
            Command::Noop { tag: None }
        } else {
            Command::Capability
        };
        self.run_ok(&command).await?;
        Ok(())
    }
}

/// The warnings of an `OK (WARNINGS) "…"`.
fn warnings(reply: &Reply) -> Option<String> {
    matches!(reply.done.code, Some(ResponseCode::Warnings)).then(|| reply.done.text_lossy())
}

#[cfg(test)]
impl Client<NotAuthenticated> {
    /// A session over an in-memory stream (as if TLS were already on).
    pub(crate) async fn connect_test(
        stream: tokio::io::DuplexStream,
        limits: Limits,
    ) -> Result<Self, Error> {
        let mut connection = Connection::new(SecureStream::Test(stream), limits);
        let deadline = after(limits.greeting_timeout);
        let capabilities = read_capabilities(&mut connection, deadline, "greeting").await?;
        Ok(Self {
            connection: Box::new(connection),
            capabilities,
            state: NotAuthenticated,
        })
    }
}
