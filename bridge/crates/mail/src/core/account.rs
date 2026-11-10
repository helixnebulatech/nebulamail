// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One mail account: credentials, the command connection, the sync lock
//! and the live-update task.

use nebula_imap::AlertSink;

use super::alerts::{AccountAlert, AlertLog};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use nebula_imap::Credentials;
use nebula_store::{AccountId, Recorded, Store};
use tokio::sync::{Mutex, MutexGuard, Semaphore, broadcast};

use super::auth::AuthFailures;
use super::cache::RawCache;
use super::connect::Connector;
use super::live::Live;
use super::memory::{Lease, MemoryBudget};
use super::slot::Slot;
use crate::sync::backoff::MailboxBackoff;
use crate::{MailConfig, MailError, MailEvent, Unavailable};

/// What all accounts share.
pub(crate) struct Shared {
    pub(crate) store: Store,
    pub(crate) connector: Connector,
    pub(crate) config: MailConfig,
    pub(crate) events: broadcast::Sender<MailEvent>,
    pub(crate) cache: RawCache,
    /// Memory for whole messages (see [`super::memory`]).
    pub(crate) memory: Arc<MemoryBudget>,
    /// Running accounts whose password the server refused.
    pub(crate) auth_failures: AuthFailures,
}

/// A mail account on the IMAP server: sync, live updates and operations.
///
/// Get one from [`Mail::open`](crate::Mail::open). Operations write to IMAP
/// first and then update the store from the server's answer, recording the
/// JMAP changes in the same transaction, and broadcast the new states.
pub struct MailAccount {
    pub(crate) id: AccountId,
    pub(crate) shared: Arc<Shared>,
    /// Replaced by a new login (the user changed the password); each new
    /// connection reads the current ones.
    credentials: RwLock<Arc<Credentials>>,
    /// The server refused the current credentials (sticky: retrying could
    /// get the account locked on the server).
    auth_failed: AtomicBool,
    /// In [`Mail`](crate::Mail)'s accounts: only then does a refused
    /// login end the sessions (the login of `open` is handled by its
    /// caller).
    registered: AtomicBool,
    /// Asked to stop: syncs end between commands.
    stopping: AtomicBool,
    /// The command connection (one command at a time).
    pub(crate) commands: Mutex<Slot>,
    /// One sync at a time per account (IDLE, STATUS checks and the first
    /// sync may all want one).
    pub(crate) sync_lock: Mutex<()>,
    /// One change of an email's mailboxes at a time (`set_mailboxes`), from
    /// reading its locations to the last expunge: two at once could each
    /// expunge the copy the other kept, leaving the email in no mailbox.
    /// Not `commands`, which each step takes by itself. Always taken before
    /// `commands`, never while holding it.
    pub(crate) membership: Mutex<()>,
    pub(crate) live: std::sync::Mutex<Option<Live>>,
    /// Whether the server announced QUOTA at the last login (read without
    /// waiting for the command connection, for the JMAP session).
    pub(crate) quota: AtomicBool,
    /// The IDLE connection has NOTIFY working: every mailbox is watched, so
    /// the periodic check can be rarer.
    pub(crate) notify_active: AtomicBool,
    /// One load of a message bigger than the cache at a time (see
    /// [`super::memory`]).
    pub(crate) whole_turn: Arc<Semaphore>,
    /// Mailboxes whose sync failed on their own, and when to try again.
    pub(crate) mailbox_backoff: MailboxBackoff,
    /// The server's latest `[ALERT]`s, from every connection.
    pub(crate) alerts: Arc<AlertLog>,
}

impl std::fmt::Debug for MailAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailAccount")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl MailAccount {
    pub(crate) fn new(id: AccountId, shared: Arc<Shared>, credentials: Credentials) -> Self {
        Self {
            id,
            shared,
            credentials: RwLock::new(Arc::new(credentials)),
            auth_failed: AtomicBool::new(false),
            registered: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            commands: Mutex::new(Slot::default()),
            sync_lock: Mutex::new(()),
            membership: Mutex::new(()),
            live: std::sync::Mutex::new(None),
            quota: AtomicBool::new(false),
            notify_active: AtomicBool::new(false),
            whole_turn: Arc::new(Semaphore::new(1)),
            mailbox_backoff: MailboxBackoff::default(),
            alerts: Arc::new(AlertLog::default()),
        }
    }

    /// The mail server's `[ALERT]`s of the last day (at most
    /// [`MAX_ALERTS`](crate::MAX_ALERTS)), oldest first: the user must see
    /// them (RFC 3501 §7.1, RFC 9051 §7.1).
    #[must_use]
    pub fn alerts(&self) -> Vec<AccountAlert> {
        self.alerts.current(std::time::SystemTime::now())
    }

    /// Where this account's connections send their alerts.
    pub(crate) fn alert_sink(&self) -> Arc<dyn AlertSink> {
        let sink: Arc<dyn AlertSink> = self.alerts.clone();
        sink
    }

    /// The account id.
    #[must_use]
    pub fn id(&self) -> AccountId {
        self.id
    }

    /// Whether the server has quotas (QUOTA, RFC 9208), as of the last
    /// login.
    #[must_use]
    pub fn supports_quota(&self) -> bool {
        self.quota.load(Ordering::Relaxed)
    }

    pub(crate) fn set_supports_quota(&self, supported: bool) {
        self.quota.store(supported, Ordering::Relaxed);
    }

    /// A new lease for an operation that holds whole messages: the bridge's
    /// memory budget and this account's turn.
    #[must_use]
    pub fn lease(&self) -> Lease {
        Lease::new(
            Arc::clone(&self.shared.memory),
            Arc::clone(&self.whole_turn),
        )
    }

    pub(crate) fn store(&self) -> &Store {
        &self.shared.store
    }

    pub(crate) fn connector(&self) -> &Connector {
        &self.shared.connector
    }

    pub(crate) fn config(&self) -> &MailConfig {
        &self.shared.config
    }

    pub(crate) async fn slot(&self) -> MutexGuard<'_, Slot> {
        self.commands.lock().await
    }

    /// Broadcasts new states, if anything changed. Nobody listening is fine.
    pub(crate) fn emit(&self, recorded: &Recorded) {
        if let Some(event) = MailEvent::changed(self.id, recorded) {
            let _ = self.shared.events.send(event);
        }
    }

    /// The credentials new connections log in with.
    pub(crate) fn credentials(&self) -> Arc<Credentials> {
        Arc::clone(
            &self
                .credentials
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// New credentials (a new login with a changed password): connections
    /// opened from now on use them; the open ones stay as they are.
    pub(crate) fn set_credentials(&self, credentials: Credentials) {
        *self
            .credentials
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(credentials);
    }

    /// Whether the server refused the current credentials.
    pub(crate) fn auth_failed(&self) -> bool {
        self.auth_failed.load(Ordering::Relaxed)
    }

    /// From now on a refused login ends the account's sessions.
    pub(crate) fn register(&self) {
        self.registered.store(true, Ordering::Relaxed);
    }

    /// What a login refused with `used` means. With the current credentials
    /// the password is wrong: sticky [`MailError::AuthFailed`], reported (if
    /// the account is registered) so its sessions end. With credentials
    /// replaced meanwhile (a login with the old password that was already
    /// under way) it is nothing to act on: retry with the new ones.
    pub(crate) fn login_refused(&self, used: &Arc<Credentials>) -> MailError {
        if !Arc::ptr_eq(used, &self.credentials()) {
            return MailError::Unavailable(Unavailable::CredentialsChanged);
        }
        self.auth_failed.store(true, Ordering::Relaxed);
        if self.registered.load(Ordering::Relaxed) {
            self.shared.auth_failures.report(self.id);
        }
        MailError::AuthFailed
    }

    /// Asks the syncs to end at their next step (the account is closing).
    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }

    /// Fails with [`Unavailable::Stopping`] once [`Self::request_stop`] was
    /// called: long syncs check it between commands.
    pub(crate) fn ensure_running(&self) -> Result<(), MailError> {
        if self.stopping.load(Ordering::Relaxed) {
            return Err(MailError::Unavailable(Unavailable::Stopping));
        }
        Ok(())
    }

    pub(crate) fn set_live(&self, live: Option<Live>) -> Option<Live> {
        let mut slot = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *slot, live)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use std::time::Duration;

    use super::*;
    use crate::test_support::{closed_port, fixture};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refusal_counts_only_with_the_current_password() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        assert_eq!(account.credentials().username(), "alice");
        account.set_credentials(Credentials::new("[EMAIL]", "new".into()));
        assert_eq!(account.credentials().username(), "[EMAIL]");

        // A login with the old password, still under way when the new one
        // came in: nothing sticky, nothing reported.
        let old = account.credentials();
        account.set_credentials(Credentials::new("alice", "newer".into()));
        account.register();
        assert!(matches!(
            account.login_refused(&old),
            MailError::Unavailable(Unavailable::CredentialsChanged)
        ));
        assert!(!account.auth_failed());

        // With the current one: sticky and reported.
        let current = account.credentials();
        assert!(matches!(
            account.login_refused(&current),
            MailError::AuthFailed
        ));
        assert!(account.auth_failed());
        let reported =
            tokio::time::timeout(Duration::from_secs(1), f.mail.shared.auth_failures.wait())
                .await
                .unwrap();
        assert_eq!(reported, vec![account.id]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unregistered_account_reports_nothing() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        let current = account.credentials();
        assert!(matches!(
            account.login_refused(&current),
            MailError::AuthFailed
        ));
        let waited = tokio::time::timeout(
            Duration::from_millis(50),
            f.mail.shared.auth_failures.wait(),
        )
        .await;
        assert!(waited.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopping_account_does_not_sync() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        account.request_stop();
        // Not even a connection attempt (that would be `Connect`/`Imap`).
        assert!(matches!(
            account.sync_all().await,
            Err(MailError::Unavailable(Unavailable::Stopping))
        ));
        assert!(matches!(
            account.check_mailboxes().await,
            Err(MailError::Unavailable(Unavailable::Stopping))
        ));
        assert!(matches!(
            account.sync_mailbox(f.inbox).await,
            Err(MailError::Unavailable(Unavailable::Stopping))
        ));
    }
}
