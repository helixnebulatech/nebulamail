// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The command connection of an account, kept open between uses.
//!
//! - It lives behind a `tokio::sync::Mutex` in the account; whoever holds
//!   the lock owns the connection (one command at a time, as IMAP wants).
//! - A broken connection (fatal error, BYE, timeout) is dropped and the
//!   next use reconnects. Failed connection attempts back off (1 s, 2 s,
//!   4 s… up to 5 min): until then calls fail at once with
//!   [`Unavailable::Backoff`] instead of hammering the server.
//! - A refused login is sticky (kept in the account): the password
//!   changed, and retrying could get the account locked on the server.
//!   The session must end. A login refused with credentials replaced
//!   while it ran is not (see [`MailAccount::login_refused`]).
//! - Operations are not retried automatically after a network error: they
//!   may have been half done, so the caller decides.

use std::time::Duration;

use nebula_imap::proto::command::{AppendMessage, ListPattern, StatusItem};
use nebula_imap::proto::response::{MailboxStatus, Namespaces};
use nebula_imap::proto::types::{Capabilities, Flag, InternalDate, MailboxName, SpecialUse};
use nebula_imap::{
    AppendManyResult, AppendResult, Authenticated, Client, Error as ImapError, ListedMailbox,
    QuotaInfo, SelectOptions, SelectResult, Selected,
};
use tokio::time::Instant;

use super::account::MailAccount;
use crate::{MailError, Unavailable};

const MAX_BACKOFF: Duration = Duration::from_mins(5);

/// A logged-in connection in either state.
pub(crate) enum Session {
    Authenticated(Client<Authenticated>),
    Selected(Client<Selected>),
}

/// Runs the same call on both states (commands valid in both).
macro_rules! both {
    ($session:expr, $client:ident => $call:expr) => {
        match $session {
            Session::Authenticated($client) => $call,
            Session::Selected($client) => $call,
        }
    };
}

impl Session {
    fn is_broken(&self) -> bool {
        both!(self, client => client.is_broken())
    }

    pub(crate) fn capabilities(&self) -> &Capabilities {
        both!(self, client => client.capabilities())
    }

    pub(crate) fn selected_name(&self) -> Option<&MailboxName> {
        match self {
            Self::Selected(client) => Some(&client.mailbox().name),
            Self::Authenticated(_) => None,
        }
    }

    pub(crate) async fn list(
        &mut self,
        status: &[StatusItem],
    ) -> Result<Vec<ListedMailbox>, ImapError> {
        let pattern = ListPattern::all();
        both!(self, client => client.list(&pattern, status).await)
    }

    pub(crate) async fn quota_root(
        &mut self,
        mailbox: &MailboxName,
    ) -> Result<QuotaInfo, ImapError> {
        both!(self, client => client.get_quota_root(mailbox).await)
    }

    pub(crate) async fn namespace(&mut self) -> Result<Namespaces, ImapError> {
        both!(self, client => client.namespace().await)
    }

    pub(crate) async fn status(
        &mut self,
        mailbox: &MailboxName,
        items: &[StatusItem],
    ) -> Result<MailboxStatus, ImapError> {
        both!(self, client => client.status(mailbox, items).await)
    }

    pub(crate) async fn append(
        &mut self,
        mailbox: &MailboxName,
        message: &[u8],
        flags: &[Flag],
        date: Option<InternalDate>,
    ) -> Result<AppendResult, ImapError> {
        both!(self, client => client.append(mailbox, message, flags, date).await)
    }

    pub(crate) async fn append_many(
        &mut self,
        mailbox: &MailboxName,
        messages: &[AppendMessage<'_>],
    ) -> Result<AppendManyResult, ImapError> {
        both!(self, client => client.append_many(mailbox, messages).await)
    }

    pub(crate) async fn create(
        &mut self,
        mailbox: &MailboxName,
        special_use: Option<SpecialUse>,
    ) -> Result<(), ImapError> {
        both!(self, client => client.create(mailbox, special_use).await)
    }

    pub(crate) async fn delete(&mut self, mailbox: &MailboxName) -> Result<(), ImapError> {
        both!(self, client => client.delete(mailbox).await)
    }

    pub(crate) async fn rename(
        &mut self,
        from: &MailboxName,
        to: &MailboxName,
    ) -> Result<(), ImapError> {
        both!(self, client => client.rename(from, to).await)
    }

    pub(crate) async fn subscribe(
        &mut self,
        mailbox: &MailboxName,
        subscribed: bool,
    ) -> Result<(), ImapError> {
        if subscribed {
            both!(self, client => client.subscribe(mailbox).await)
        } else {
            both!(self, client => client.unsubscribe(mailbox).await)
        }
    }
}

/// When to send SELECT again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reselect {
    /// Always (the sync needs fresh UIDVALIDITY, HIGHESTMODSEQ, QRESYNC).
    Always,
    /// Only if another mailbox (or none) is selected.
    IfOther,
}

#[derive(Default)]
pub(crate) struct Slot {
    session: Option<Session>,
    failures: u32,
    retry_at: Option<Instant>,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("connected", &self.session.is_some())
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

/// How long to wait after this many failed connection attempts in a row.
pub(crate) fn backoff(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    Duration::from_secs(1_u64 << exponent).min(MAX_BACKOFF)
}

impl Slot {
    /// The logged-in connection, opening one if needed.
    pub(crate) async fn session(
        &mut self,
        account: &MailAccount,
    ) -> Result<&mut Session, MailError> {
        if self.session.as_ref().is_some_and(Session::is_broken) {
            self.session = None;
        }
        if self.session.is_none() {
            let client = self.connect(account).await?;
            self.session = Some(Session::Authenticated(client));
        }
        self.session
            .as_mut()
            .ok_or(MailError::Unavailable(Unavailable::Backoff))
    }

    async fn connect(&mut self, account: &MailAccount) -> Result<Client<Authenticated>, MailError> {
        account.ensure_running()?;
        if account.auth_failed() {
            return Err(MailError::AuthFailed);
        }
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return Err(MailError::Unavailable(Unavailable::Backoff));
        }
        let credentials = account.credentials();
        match account
            .connector()
            .login(&credentials, account.alert_sink())
            .await
        {
            Ok(client) => {
                self.failures = 0;
                self.retry_at = None;
                Ok(client)
            }
            Err(MailError::AuthFailed) => Err(account.login_refused(&credentials)),
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                self.retry_at = Some(Instant::now() + backoff(self.failures));
                Err(error)
            }
        }
    }

    /// The connection with `mailbox` selected (read-write).
    pub(crate) async fn select(
        &mut self,
        account: &MailAccount,
        mailbox: &MailboxName,
        options: SelectOptions,
        when: Reselect,
    ) -> Result<(&mut Client<Selected>, Option<SelectResult>), MailError> {
        let session = self.session(account).await?;
        let reuse = when == Reselect::IfOther && session.selected_name() == Some(mailbox);
        let mut result = None;
        if !reuse {
            let taken = self.session.take();
            let outcome = match taken {
                Some(Session::Authenticated(client)) => client.select(mailbox, options).await,
                Some(Session::Selected(client)) => client.select(mailbox, options).await,
                None => return Err(MailError::Unavailable(Unavailable::Backoff)),
            };
            match outcome {
                Ok((client, selected)) => {
                    self.session = Some(Session::Selected(client));
                    result = Some(selected);
                }
                Err(error) => {
                    let (error, client) = error.into_parts();
                    self.session = client.map(Session::Authenticated);
                    return Err(error.into());
                }
            }
        }
        match &mut self.session {
            Some(Session::Selected(client)) => Ok((client, result)),
            _ => Err(MailError::Unavailable(Unavailable::Backoff)),
        }
    }

    /// The selected connection, if a mailbox is selected.
    pub(crate) fn selected(&mut self) -> Option<&mut Client<Selected>> {
        match &mut self.session {
            Some(Session::Selected(client)) => Some(client),
            _ => None,
        }
    }

    /// Leaves the selected mailbox if it is `mailbox` (before DELETE or
    /// RENAME of it: some servers refuse while it's selected).
    pub(crate) async fn leave(&mut self, mailbox: &MailboxName) -> Result<(), MailError> {
        if self.session.as_ref().and_then(Session::selected_name) != Some(mailbox) {
            return Ok(());
        }
        if let Some(Session::Selected(client)) = self.session.take() {
            match client.unselect().await {
                Ok(client) => self.session = Some(Session::Authenticated(client)),
                // Can't unselect (no UNSELECT on a read-write mailbox): a
                // new connection starts unselected.
                Err(_) => self.session = None,
            }
        }
        Ok(())
    }

    /// Converts a command result; a fatal error drops the connection so the
    /// next use reconnects.
    pub(crate) fn check<T>(&mut self, result: Result<T, ImapError>) -> Result<T, MailError> {
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.session = None;
        }
        result.map_err(MailError::from)
    }

    /// Logs out (best effort) and forgets the connection.
    pub(crate) async fn close(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = both!(session, client => client.logout().await);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(4), Duration::from_secs(8));
        assert_eq!(backoff(30), MAX_BACKOFF);
    }
}
