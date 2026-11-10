// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The groupware service: the accounts with an active session, their
//! push watchers and their events.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use nebula_crypto::AccountKey;
use nebula_dav::{Credentials, DavClient};
use nebula_store::{AccountId, Store};
use net_guard::Resolver;
use secrecy::SecretString;
use tokio::sync::broadcast;

use crate::core::account::{DavStatus, GroupwareAccount, Shared};
use crate::core::live::Live;
use crate::{GroupwareConfig, GroupwareError, GroupwareEvent};

/// Events waiting for slow listeners; older ones are dropped (a listener
/// that lagged just reloads the states).
const EVENT_BUFFER: usize = 1024;

/// The calendars and contacts layer of the bridge.
///
/// ```no_run
/// # async fn example(store: nebula_store::Store, account: nebula_store::AccountId, key: nebula_crypto::AccountKey) -> Result<(), nebula_groupware::GroupwareError> {
/// use std::sync::Arc;
/// use secrecy::SecretString;
/// use nebula_dav::DavServer;
/// use nebula_groupware::{Groupware, GroupwareConfig};
///
/// let server = DavServer::new("https://dav.example.com/").unwrap();
/// let resolver = Arc::new(net_guard::SystemResolver::new().unwrap());
/// let groupware = Groupware::new(store, GroupwareConfig::new(server), resolver);
/// let mut events = groupware.subscribe();
/// // At login, with the IMAP credentials: discovery and sync start in
/// // the background (a DAV failure never fails the login).
/// let alice = groupware.open(account, "alice", SecretString::from("secret"), key)?;
/// // While a push connection is open, sync every minute instead of 5.
/// let watching = groupware.watch(account);
/// // After the bridge wrote something itself:
/// alice.kick();
/// // When the account has no sessions left:
/// groupware.close(account).await;
/// # Ok(()) }
/// ```
pub struct Groupware {
    shared: Arc<Shared>,
    resolver: Arc<dyn Resolver>,
    accounts: std::sync::Mutex<HashMap<AccountId, Arc<GroupwareAccount>>>,
}

impl std::fmt::Debug for Groupware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Groupware").finish_non_exhaustive()
    }
}

/// An open push connection of an account: while one exists, the account
/// is synced every `poll_with_push`. Drop it when the connection ends.
pub struct Watch {
    shared: Arc<Shared>,
    account: AccountId,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A listener of the account's calendar alerts that is not a push
/// connection (Web Push): while one exists, due alerts are sent even
/// with no event source open; polling keeps its slower interval. Drop it
/// when the account has no subscription for alerts left.
pub struct AlertListener {
    shared: Arc<Shared>,
    account: AccountId,
}

impl std::fmt::Debug for AlertListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertListener")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl Drop for AlertListener {
    fn drop(&mut self) {
        decrement(&self.shared.alert_listeners, self.account);
    }
}

fn decrement(counts: &std::sync::Mutex<HashMap<AccountId, usize>>, account: AccountId) {
    let mut counts = counts.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(count) = counts.get_mut(&account) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&account);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut watchers = self
            .shared
            .watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = watchers.get_mut(&self.account) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                watchers.remove(&self.account);
            }
        }
    }
}

impl Groupware {
    /// The service for the configured DAV server.
    #[must_use]
    pub fn new(store: Store, config: GroupwareConfig, resolver: Arc<dyn Resolver>) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self {
            shared: Arc::new(Shared {
                store,
                config,
                events,
                watchers: std::sync::Mutex::new(HashMap::new()),
                alert_listeners: std::sync::Mutex::new(HashMap::new()),
            }),
            resolver,
            accounts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// State changes and refused logins of every account.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<GroupwareEvent> {
        self.shared.events.subscribe()
    }

    /// Starts an account (at login, or when the bridge restarts with
    /// sessions) with the same login and password as IMAP, and the
    /// account's data key for the local copy. Discovery and sync run in
    /// the background, so nothing here waits for the DAV server. If the
    /// account is running it is returned with its login changed in place
    /// (a new password goes out from the next request on; the same one
    /// changes nothing), unless the DAV server refused its login, in which
    /// case it starts again with the new password.
    ///
    /// # Errors
    /// [`GroupwareError::Dav`] if the login can't be sent with Basic (a
    /// `:` in it) or the TLS setup fails.
    pub fn open(
        &self,
        account: AccountId,
        login: &str,
        password: SecretString,
        key: AccountKey,
    ) -> Result<Arc<GroupwareAccount>, GroupwareError> {
        let credentials = Credentials::new(login, password)?;
        if let Some(running) = self.account(account)
            && running.status() != DavStatus::Unauthorized
        {
            // Not started again: the old task would only stop at its next
            // wait and run beside the new one, on the same store.
            running.set_credentials(Some(&credentials));
            return Ok(running);
        }
        // One client per configured server, each sending the login only
        // to its own origin (`url`, and `contacts_url` if set).
        let contacts_client = self
            .shared
            .config
            .contacts_server
            .as_ref()
            .map(|server| {
                DavClient::with_resolver(
                    server.clone(),
                    Some(credentials.clone()),
                    Arc::clone(&self.resolver),
                )
            })
            .transpose()?;
        let client = DavClient::with_resolver(
            self.shared.config.server.clone(),
            Some(credentials),
            Arc::clone(&self.resolver),
        )?;
        let started = Arc::new(GroupwareAccount::new(
            account,
            Arc::clone(&self.shared),
            client,
            contacts_client,
            key,
        ));
        started.set_live(Some(Live::start(Arc::clone(&started))));
        let previous = self
            .accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(account, Arc::clone(&started));
        // The old task stops (after a refused login it already has).
        if let Some(previous) = previous {
            drop(previous.set_live(None));
        }
        Ok(started)
    }

    /// The running account, if any.
    #[must_use]
    pub fn account(&self, account: AccountId) -> Option<Arc<GroupwareAccount>> {
        self.accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .cloned()
    }

    /// Asks a running account for a sync round now (after the bridge's
    /// own writes).
    pub fn kick(&self, account: AccountId) {
        if let Some(running) = self.account(account) {
            running.kick();
        }
    }

    /// Counts a push connection of the account until the [`Watch`] is
    /// dropped. The first one wakes the live task, so the faster poll
    /// interval applies at once.
    #[must_use]
    pub fn watch(&self, account: AccountId) -> Watch {
        let first = {
            let mut watchers = self
                .shared
                .watchers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let count = watchers.entry(account).or_insert(0);
            *count = count.saturating_add(1);
            *count == 1
        };
        if first && let Some(running) = self.account(account) {
            running.wake.notify_one();
        }
        Watch {
            shared: Arc::clone(&self.shared),
            account,
        }
    }

    /// Counts a listener of the account's alerts (Web Push, or a push
    /// connection that asked for `CalendarAlert`) until the
    /// [`AlertListener`] is dropped. Alerts waiting for someone go out now.
    /// A [`Watch`] alone does not hear alerts.
    #[must_use]
    pub fn listen_alerts(&self, account: AccountId) -> AlertListener {
        {
            let mut counts = self
                .shared
                .alert_listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let count = counts.entry(account).or_insert(0);
            *count = count.saturating_add(1);
        }
        if let Some(running) = self.account(account) {
            running.alerts_wake.notify_one();
        }
        AlertListener {
            shared: Arc::clone(&self.shared),
            account,
        }
    }

    /// Stops an account (its last session ended). What was synced stays in
    /// the store (sealed) for its next login.
    pub async fn close(&self, account: AccountId) {
        let removed = self
            .accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&account);
        if let Some(removed) = removed {
            // Nothing sent from now on carries the password (a request in
            // flight while it stops, after the session was revoked).
            removed.set_credentials(None);
            if let Some(live) = removed.set_live(None) {
                live.stop().await;
            }
        }
    }
}

impl Drop for Groupware {
    /// Without [`Groupware::close`]: each account's task is asked to stop.
    fn drop(&mut self) {
        let accounts = self
            .accounts
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for (_, account) in accounts.drain() {
            drop(account.set_live(None));
        }
    }
}
