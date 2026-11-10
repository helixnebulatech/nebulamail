// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The mail service: the accounts with an active session, and their events.

use nebula_imap::{AlertSink, ServerAlert};

use crate::core::alerts::Collected;
use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use tokio::time::Instant;

use nebula_imap::Credentials;
use nebula_imap::proto::types::Capability;
use nebula_store::{AccountId, DataType, Store};
use net_guard::Resolver;
use secrecy::SecretString;
use tokio::sync::broadcast;

use crate::core::account::{MailAccount, Shared};
use crate::core::auth::AuthFailures;
use crate::core::cache::RawCache;
use crate::core::connect::Connector;
use crate::core::live::Live;
use crate::core::memory::MemoryBudget;
use crate::core::slot::backoff;
use crate::{MailConfig, MailError, MailEvent, Unavailable};

/// Events waiting for slow listeners; older ones are dropped (a listener
/// that lagged just reloads the states). Only state changes go there: a
/// refused password must never be dropped ([`Mail::auth_failures`]).
const EVENT_BUFFER: usize = 1024;

/// Why an account is started ([`Mail::open`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Start {
    /// Right after a login the server accepted: start now, even if recent
    /// attempts failed (the server just answered).
    Login,
    /// With the stored password (after a restart): waits out the backoff
    /// of earlier failed attempts.
    Restore,
}

/// Failed attempts to start an account. Kept in [`Mail`], not in the
/// account object that a failed start throws away, so retries back off.
#[derive(Debug, Default)]
struct StartGate {
    failures: u32,
    retry_at: Option<Instant>,
}

impl StartGate {
    fn reset(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }

    fn failed(&mut self) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(Instant::now() + backoff(self.failures));
    }

    fn waiting(&self) -> bool {
        self.retry_at.is_some_and(|at| Instant::now() < at)
    }
}

type Gate = Arc<tokio::sync::Mutex<StartGate>>;

/// The mail layer of the bridge.
///
/// ```no_run
/// # async fn example(store: nebula_store::Store, account: nebula_store::AccountId) -> Result<(), nebula_mail::MailError> {
/// use std::sync::Arc;
/// use secrecy::SecretString;
/// use nebula_mail::{ImapServer, Mail, MailConfig, TlsMode};
///
/// let server = ImapServer::new("imap.example.com".parse().unwrap(), net_guard::Port::IMAPS, TlsMode::Implicit);
/// let resolver = Arc::new(net_guard::SystemResolver::new().unwrap());
/// let mail = Mail::new(store, MailConfig::new(server), resolver)?;
/// let mut events = mail.subscribe();
/// // At login: checks the password and starts syncing.
/// let alice = mail.open(account, "alice", SecretString::from("secret"), nebula_mail::Start::Login).await?;
/// // When the account has no sessions left:
/// mail.close(account).await;
/// # Ok(()) }
/// ```
pub struct Mail {
    pub(crate) shared: Arc<Shared>,
    accounts: std::sync::Mutex<HashMap<AccountId, Arc<MailAccount>>>,
    /// One per account that ever started: starts of an account go one at
    /// a time, and remember their failures.
    starts: std::sync::Mutex<HashMap<AccountId, Gate>>,
}

impl std::fmt::Debug for Mail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mail").finish_non_exhaustive()
    }
}

impl Mail {
    /// The service for the configured server.
    ///
    /// # Errors
    /// [`MailError::Config`] if the TLS setup or the host name can't work.
    pub fn new(
        store: Store,
        config: MailConfig,
        resolver: Arc<dyn Resolver>,
    ) -> Result<Self, MailError> {
        let connector = Connector::new(config.server.clone(), resolver)?;
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let cache = RawCache::new(config.raw_cache_bytes, config.raw_cache_max_item);
        let memory = MemoryBudget::new(
            config.whole_memory,
            config.whole_wait,
            config.whole_turn_wait,
        );
        Ok(Self {
            shared: Arc::new(Shared {
                store,
                connector,
                config,
                events,
                cache,
                memory,
                auth_failures: AuthFailures::default(),
            }),
            accounts: std::sync::Mutex::new(HashMap::new()),
            starts: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// State changes of every account.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<MailEvent> {
        self.shared.events.subscribe()
    }

    /// Tells the listeners that these states of the account moved forward
    /// by something outside the mail layer (the bridge's scheduler sending
    /// a held submission). Nobody listening is fine.
    pub fn announce(&self, account: AccountId, data_types: Vec<DataType>) {
        if data_types.is_empty() {
            return;
        }
        let _ = self
            .shared
            .events
            .send(MailEvent::StateChanged(crate::StateChanged {
                account,
                data_types,
                delivered: Vec::new(),
            }));
    }

    /// Waits for running accounts whose password the server refused, and
    /// returns them: end their sessions (that calls [`Mail::close`]). None
    /// is lost, however late this is called. Safe to cancel.
    pub async fn auth_failures(&self) -> Vec<AccountId> {
        self.shared.auth_failures.wait().await
    }

    fn gate(&self, account: AccountId) -> Gate {
        let mut starts = self.starts.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(starts.entry(account).or_default())
    }

    /// Checks a user name and password by logging in to the IMAP server
    /// (the bridge has no passwords of its own).
    ///
    /// # Errors
    /// [`MailError::AuthFailed`] for wrong credentials; others if the server
    /// can't be reached.
    pub async fn verify_login(&self, login: &str, password: SecretString) -> Result<(), MailError> {
        self.check_login(login, password).await.0
    }

    /// As [`Self::verify_login`], with the server's `[ALERT]`s during the
    /// check (a `NO [ALERT] Account locked…` to show in the login form,
    /// RFC 3501 §7.1, RFC 9051 §7.1).
    pub async fn check_login(
        &self,
        login: &str,
        password: SecretString,
    ) -> (Result<(), MailError>, Vec<ServerAlert>) {
        let credentials = Credentials::new(login, password);
        let collected = Arc::new(Collected::default());
        let sink: Arc<dyn AlertSink> = collected.clone();
        let result = match self.shared.connector.login(&credentials, sink).await {
            Ok(client) => {
                let _ = client.logout().await;
                Ok(())
            }
            Err(error) => Err(error),
        };
        let alerts =
            std::mem::take(&mut *collected.0.lock().unwrap_or_else(PoisonError::into_inner));
        (result, alerts)
    }

    /// Starts an account (at login, or when the bridge restarts with
    /// sessions): logs in, then syncs in the background and keeps it up to
    /// date. If the account is already running it is returned, with these
    /// credentials for its next connections; if its password was
    /// refused it starts again with the new one.
    ///
    /// Starts of one account go one at a time. A start that couldn't reach
    /// the server makes the next [`Start::Restore`] wait (1 s, 2 s, 4 s… up
    /// to 5 min) and fail with [`Unavailable::Backoff`] meanwhile, so
    /// requests don't log in again and again.
    ///
    /// # Errors
    /// [`MailError::AuthFailed`] if the password is wrong (not reported in
    /// [`Self::auth_failures`]: the caller acts on it); others if the
    /// server can't be reached.
    pub async fn open(
        &self,
        account: AccountId,
        login: &str,
        password: SecretString,
        start: Start,
    ) -> Result<Arc<MailAccount>, MailError> {
        let gate = self.gate(account);
        let mut gate = gate.lock().await;
        let credentials = Credentials::new(login, password);
        if let Some(running) = self.account(account)
            && !running.auth_failed()
        {
            running.set_credentials(credentials);
            return Ok(running);
        }
        match start {
            Start::Login => gate.reset(),
            Start::Restore if gate.waiting() => {
                return Err(MailError::Unavailable(Unavailable::Backoff));
            }
            Start::Restore => {}
        }
        // Not registered yet: a refused login here is only returned.
        let started = Arc::new(MailAccount::new(
            account,
            Arc::clone(&self.shared),
            credentials,
        ));
        {
            // Log in now: a wrong password fails here, not in the background.
            let mut slot = started.slot().await;
            match slot.session(&started).await {
                Ok(session) => {
                    started.set_supports_quota(session.capabilities().has(&Capability::Quota));
                }
                Err(error @ MailError::Unavailable(_)) => {
                    gate.failed();
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        gate.reset();
        // These credentials work: an older refusal must not end the
        // sessions of this login.
        self.shared.auth_failures.clear(account);
        started.register();
        started.set_live(Some(Live::start(Arc::clone(&started))));
        let previous = self
            .accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(account, Arc::clone(&started));
        drop(gate);
        if let Some(previous) = previous {
            shut_down(&previous).await;
        }
        Ok(started)
    }

    /// The running account, if any.
    #[must_use]
    pub fn account(&self, account: AccountId) -> Option<Arc<MailAccount>> {
        self.accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .cloned()
    }

    /// Stops an account (its last session ended, or its password was
    /// refused): live sync stops, the connection is closed and its cached
    /// messages are dropped.
    pub async fn close(&self, account: AccountId) {
        // Before any wait, so a cancelled close still drops them, and again
        // at the end: an operation still running may have cached more.
        self.shared.cache.forget_account(account);
        self.shared.auth_failures.clear(account);
        let removed = self
            .accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&account);
        if let Some(removed) = removed {
            shut_down(&removed).await;
        }
        self.shared.cache.forget_account(account);
    }
}

impl Drop for Mail {
    /// Without [`Mail::close`]: each account's live task is asked to stop
    /// (it ends at its next wait and lets go of the account and its
    /// connections).
    fn drop(&mut self) {
        let accounts = self
            .accounts
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for (_, account) in accounts.drain() {
            account.request_stop();
            drop(account.set_live(None));
        }
    }
}

async fn shut_down(account: &MailAccount) {
    // Syncs end at their next command instead of running to the end.
    account.request_stop();
    if let Some(live) = account.set_live(None) {
        live.stop().await;
    }
    account.slot().await.close().await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::test_support::fixture;

    /// A server that accepts connections and closes them at once, and
    /// counts them.
    async fn hanging_up() -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&count);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        (port, count)
    }

    fn password() -> SecretString {
        SecretString::from("secret")
    }

    fn is_backoff(result: &Result<Arc<MailAccount>, MailError>) -> bool {
        matches!(result, Err(MailError::Unavailable(Unavailable::Backoff)))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_starts_back_off() {
        let (port, connections) = hanging_up().await;
        let f = fixture(port).await;
        let first = f
            .mail
            .open(f.account, "alice", password(), Start::Restore)
            .await;
        assert!(matches!(first, Err(MailError::Unavailable(_))) && !is_backoff(&first));
        let second = f
            .mail
            .open(f.account, "alice", password(), Start::Restore)
            .await;
        assert!(is_backoff(&second), "{second:?}");
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        assert!(f.mail.account(f.account).is_none());

        // A login the server just accepted doesn't wait.
        let fresh = f
            .mail
            .open(f.account, "alice", password(), Start::Login)
            .await;
        assert!(matches!(fresh, Err(MailError::Unavailable(_))) && !is_backoff(&fresh));
        assert_eq!(connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn starts_of_one_account_go_one_at_a_time() {
        let (port, connections) = hanging_up().await;
        let f = fixture(port).await;
        let (one, two) = tokio::join!(
            f.mail.open(f.account, "alice", password(), Start::Restore),
            f.mail.open(f.account, "alice", password(), Start::Restore),
        );
        assert!(one.is_err() && two.is_err());
        assert!(is_backoff(&one) != is_backoff(&two), "{one:?} {two:?}");
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancelled_close_still_drops_the_cache() {
        let f = fixture(crate::test_support::closed_port()).await;
        let account = f.account();
        f.mail
            .accounts
            .lock()
            .unwrap()
            .insert(f.account, Arc::clone(&account));
        let email = f.add_message(1).await;
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::from(&b"x"[..]));
        // The connection is busy: closing waits for it.
        let busy = account.slot().await;
        let closing = tokio::time::timeout(Duration::ZERO, f.mail.close(f.account)).await;
        assert!(closing.is_err());
        assert!(f.mail.shared.cache.get(f.account, email).is_none());
        // And the account was asked to stop.
        assert!(account.ensure_running().is_err());
        drop(busy);
    }
}
