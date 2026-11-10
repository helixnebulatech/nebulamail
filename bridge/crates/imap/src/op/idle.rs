// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE (RFC 2177).
//!
//! ```no_run
//! # async fn example(client: nebula_imap::Client<nebula_imap::Selected>) -> Result<(), nebula_imap::Error> {
//! use nebula_imap::IdleEvent;
//!
//! // Your "stop" signal: a channel, a shutdown token…
//! let stop = tokio::time::sleep(std::time::Duration::from_secs(600));
//! tokio::pin!(stop);
//! let mut idle = client.idle().await?;
//! loop {
//!     tokio::select! {
//!         event = idle.wait() => match event? {
//!             IdleEvent::Changes(changes) => { /* EXISTS, EXPUNGE, VANISHED, FETCH… */ }
//!             // Time to restart IDLE (before the server's 30 min limit).
//!             IdleEvent::Refresh => idle = idle.done().await?.idle().await?,
//!             _ => {}
//!         },
//!         () = &mut stop => break,
//!     }
//! }
//! let client = idle.done().await?;
//! # Ok(())
//! # }
//! ```

use nebula_imap_proto::command::{self, Idle};
use nebula_imap_proto::response::{Condition, Response, Tagged, Untagged};
use nebula_imap_proto::types::{Capability, Tag};
use tokio::time::Instant;

use crate::core::client::{Client, Selected, Session};
use crate::core::connection::{after, expect_ok};
use crate::{Error, StateError};

/// What [`IdleSession::wait`] saw.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdleEvent {
    /// The server pushed changes (EXISTS, EXPUNGE, VANISHED, FETCH with new
    /// flags, alerts, BYE…). Keepalive `* OK` lines are not reported.
    Changes(Vec<Untagged>),
    /// It is time to end IDLE ([`IdleSession::done`]) and start it again,
    /// before the server drops the connection (RFC 2177: 29 minutes).
    Refresh,
}

/// A selected mailbox in IDLE. Commands are not possible until
/// [`IdleSession::done`] gives the client back.
#[derive(Debug)]
pub struct IdleSession {
    client: Client<Selected>,
    tag: Tag,
    refresh_at: Instant,
    pending: Vec<Untagged>,
    /// Set if the server ended IDLE by itself.
    finished: Option<Tagged>,
}

impl Client<Selected> {
    /// Starts IDLE (needs the IDLE capability).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] or a NO/BAD give the client back inside the
    /// error; connection errors do not.
    pub async fn idle(mut self) -> Result<IdleSession, StateError<Client<Selected>>> {
        if !self.session.has(&Capability::Idle) {
            return Err(StateError::new(Error::Unsupported("IDLE"), self));
        }
        let options = self.session.encode_options();
        let Self {
            connection,
            session,
            state,
        } = &mut self;
        // News sent before `+ idling` (a new message that arrived just
        // then) are changes too: the first `wait` returns them at once.
        let mut early = Vec::new();
        let mut on_untagged = |data: Untagged| {
            state.mailbox.apply(&data);
            if let Some(change) = change_or_record(session, data) {
                early.push(change);
            }
        };
        let started = connection
            .start_open_command(&Idle, options, &mut on_untagged)
            .await;
        match started {
            Ok(Ok(tag)) => {
                let refresh_at = after(self.connection.limits.idle_refresh);
                Ok(IdleSession {
                    client: self,
                    tag,
                    refresh_at,
                    pending: early,
                    finished: None,
                })
            }
            Ok(Err(tagged)) => {
                let error = expect_ok("IDLE", tagged)
                    .err()
                    .unwrap_or(Error::Protocol("IDLE"));
                Err(StateError::new(error, self))
            }
            Err(error) => Err(StateError::fatal(error)),
        }
    }
}

/// What IDLE reports as a change. Unknown responses and capabilities go
/// to the session (logged or applied); keepalive `* OK` lines are dropped.
fn change_or_record(session: &mut Session, data: Untagged) -> Option<Untagged> {
    session.observe(&data);
    match data {
        Untagged::Unknown(_) | Untagged::Capabilities(_) => {
            session.record(data);
            None
        }
        // `* OK still here` keepalives are not changes.
        Untagged::Status(ref status)
            if status.condition == Condition::Ok && status.code.is_none() =>
        {
            None
        }
        other => Some(other),
    }
}

impl IdleSession {
    /// Waits until the server pushes something or it is time to refresh.
    ///
    /// Cancel-safe: use it in `tokio::select!` next to your own "stop"
    /// signal, then call [`Self::done`]. Nothing received is lost if the
    /// future is dropped.
    ///
    /// # Errors
    ///
    /// Connection errors (the connection is then unusable).
    pub async fn wait(&mut self) -> Result<IdleEvent, Error> {
        loop {
            if !self.pending.is_empty() {
                return Ok(IdleEvent::Changes(std::mem::take(&mut self.pending)));
            }
            if self.finished.is_some() {
                return Ok(IdleEvent::Refresh);
            }
            match self.client.connection.read_until(self.refresh_at).await? {
                None => return Ok(IdleEvent::Refresh),
                Some(response) => self.accept(response)?,
            }
            // Take whatever else is already complete, without waiting.
            while let Some(response) = self.client.connection.try_decode()? {
                self.accept(response)?;
            }
        }
    }

    fn accept(&mut self, response: Response) -> Result<(), Error> {
        match response {
            Response::Untagged(data) => {
                self.client.state.mailbox.apply(&data);
                if let Some(change) = change_or_record(&mut self.client.session, data) {
                    self.pending.push(change);
                }
            }
            Response::Tagged(tagged) if tagged.tag == self.tag => self.finished = Some(tagged),
            Response::Tagged(_) => {
                return Err(self.protocol_error("tagged response for another command"));
            }
            // Some servers repeat `+ idling`: nothing to do.
            Response::Continuation(_) => {}
            _ => return Err(self.protocol_error("unknown response kind")),
        }
        Ok(())
    }

    /// A protocol error breaks the connection: we no longer know its state.
    fn protocol_error(&mut self, what: &'static str) -> Error {
        self.client.connection.mark_broken();
        Error::Protocol(what)
    }

    /// Ends IDLE (`DONE`) and gives the client back. Changes not yet
    /// returned by [`Self::wait`] go to [`Client::take_events`].
    ///
    /// # Errors
    ///
    /// Connection errors, or a NO/BAD that ended IDLE.
    pub async fn done(mut self) -> Result<Client<Selected>, Error> {
        let finished = match self.finished.take() {
            Some(tagged) => tagged,
            None => self.send_done().await?,
        };
        let mut client = self.client;
        // Our tagged response arrived: the stream is between commands.
        client.connection.finish_command();
        for data in self.pending {
            client.session.record(data);
        }
        expect_ok("IDLE", finished)?;
        Ok(client)
    }

    async fn send_done(&mut self) -> Result<Tagged, Error> {
        let connection = &mut self.client.connection;
        let deadline = connection.command_deadline();
        connection.begin_command();
        connection
            .write_line(&command::idle_done(), deadline, "IDLE")
            .await?;
        loop {
            match self
                .client
                .connection
                .read_response(deadline, "IDLE")
                .await?
            {
                Response::Tagged(tagged) if tagged.tag == self.tag => return Ok(tagged),
                other => self.accept(other)?,
            }
        }
    }
}
