// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! State shared by all handlers, and the account lifecycle: what happens
//! after a login, on first use after a restart, and when the last session
//! ends.
//!
//! The stored IMAP password goes with the last session, except for
//! scheduled mail ([`SignedOutSending`]): after a normal end (logout,
//! revoke, expiry) an
//! account with held messages marked `sendWhenSignedOut` keeps running, so
//! the scheduler can send them, until none is left (the periodic cleanup
//! looks again every minute). A session revoked as stolen
//! ([`SessionEnd::Compromised`]) or a password the mail server refuses
//! deletes it at once.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nebula_crypto::{
    AccountId as CryptoAccountId, AccountKey, DataKey, Field, Keyring, SealedField, WrappedDataKey,
};
use nebula_store::{Account, AccountId, NewAccount, SecretField, Store, StoreError, UnixTime};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

use crate::backend::{
    AccountContext, Backend, BackendError, LoginName, LoginOutcome, StartReason, jmap_account_id,
};
use crate::config::{HttpConfig, SignedOutSending};
use crate::error::HttpError;
use crate::limits::{DOWNLOAD_WAIT, Downloads, LoginLimiter, Slots};
use crate::oauth::client_auth::ClientAuth;
use crate::oauth::pending::PendingAuthorizations;

/// Where the current time comes from (a fixed clock in tests).
pub(crate) type Clock = Arc<dyn Fn() -> Result<UnixTime, StoreError> + Send + Sync>;

/// How long ended grants stay in the database before cleanup deletes them.
const KEEP_ENDED_GRANTS: Duration = Duration::from_hours(24 * 7);

/// How a session ended ([`Shared::stop_if_idle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEnd {
    /// Logged out, revoked by its client, or expired: held messages marked
    /// `sendWhenSignedOut` keep the password.
    Ended,
    /// Revoked because its code or refresh token was used twice (it may be
    /// stolen): nothing but another live session keeps the password.
    Compromised,
}

/// Accounts whose backend is running, and logins in progress.
#[derive(Debug, Default)]
struct Accounts {
    started: HashSet<AccountId>,
    /// Running accounts without a session, kept for this many held
    /// messages (to log only changes).
    kept_for_held: HashMap<AccountId, u64>,
    /// Logins between storing the password and storing their code: the
    /// password must stay. From there on, the code in the store keeps it
    /// (see [`Store::has_pending_code`]), and then the session.
    logging_in: HashMap<AccountId, usize>,
}

/// An account's lifecycle lock, held: starting and stopping the account
/// never interleave (a stop can't miss a start that is half done).
pub(crate) struct AccountLock {
    _guard: OwnedMutexGuard<()>,
    account: AccountId,
}

/// A login that stored its password and hasn't stored its code yet. While
/// it lives, the account isn't idle.
pub(crate) struct LoginInProgress<'a> {
    accounts: &'a Mutex<Accounts>,
    account: AccountId,
}

impl Drop for LoginInProgress<'_> {
    fn drop(&mut self) {
        let mut accounts = self.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = accounts.logging_in.get_mut(&self.account) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                accounts.logging_in.remove(&self.account);
            }
        }
    }
}

/// What [`rewrap_data_keys`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RewrapReport {
    /// Data keys wrapped again with the current master key.
    pub rewrapped: usize,
    /// Data keys that already used the current master key.
    pub current: usize,
    /// Data keys that could not be unwrapped (their master key is missing
    /// or wrong). Their accounts can't use their stored password.
    pub failed: usize,
}

impl RewrapReport {
    /// Whether every data key now uses the current master key, so the older
    /// master keys can be removed from the configuration.
    #[must_use]
    pub fn all_current(&self) -> bool {
        self.failed == 0
    }
}

/// Wraps every data key made with an older master key again with the
/// current one (after a rotation). A key that can't be unwrapped (its
/// master key isn't configured, or it doesn't match) is left as it is
/// and counted in [`RewrapReport::failed`]. Run it at startup, before
/// serving: once [`RewrapReport::all_current`] is true the older master
/// keys can be removed from the configuration.
///
/// # Errors
/// On store failures.
pub async fn rewrap_data_keys(store: &Store, keyring: &Keyring) -> Result<RewrapReport, HttpError> {
    let mut report = RewrapReport::default();
    for row in store.wrapped_data_keys().await? {
        let rewrapped = WrappedDataKey::from_bytes(&row.wrapped_dek)
            .map_err(HttpError::from)
            .and_then(|wrapped| {
                let key_id = crypto_account(&row.login)?;
                Ok(keyring.rewrap(&key_id, &wrapped)?)
            });
        match rewrapped {
            Ok(None) => report.current = report.current.saturating_add(1),
            Ok(Some(new)) => {
                // Only if nobody changed it meanwhile (another process).
                if store
                    .replace_wrapped_data_key(row.id, &row.wrapped_dek, new.as_bytes())
                    .await?
                {
                    report.rewrapped = report.rewrapped.saturating_add(1);
                }
            }
            Err(error) => {
                tracing::error!(
                    account = row.id.get(),
                    error = %error,
                    "data key could not be re-wrapped"
                );
                report.failed = report.failed.saturating_add(1);
            }
        }
    }
    Ok(report)
}

pub(crate) struct Shared<B> {
    pub(crate) config: HttpConfig,
    pub(crate) store: Store,
    pub(crate) keyring: Keyring,
    pub(crate) backend: B,
    pub(crate) shutdown: CancellationToken,
    pub(crate) clock: Clock,
    pub(crate) limiter: LoginLimiter,
    pub(crate) pending: PendingAuthorizations,
    pub(crate) client_auth: ClientAuth,
    pub(crate) api_slots: Slots,
    pub(crate) upload_slots: Slots,
    pub(crate) stream_slots: Slots,
    pub(crate) downloads: Downloads,
    /// How long a download waits for a slot ([`DOWNLOAD_WAIT`]).
    pub(crate) download_wait: Duration,
    accounts: Mutex<Accounts>,
    lifecycle: Mutex<HashMap<AccountId, Arc<tokio::sync::Mutex<()>>>>,
}

impl<B> std::fmt::Debug for Shared<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// The key that binds an account's data key and secrets (the AAD of
/// `nebula-crypto`). It must be known before the account row exists (the
/// wrapped key is stored with the row), so it is the login, lower-cased
/// in ASCII like SQLite's `NOCASE` that makes logins unique.
pub(crate) fn crypto_account(login: &str) -> Result<CryptoAccountId, HttpError> {
    let id = format!("login:{}", login.to_ascii_lowercase());
    Ok(CryptoAccountId::new(id.as_bytes())?)
}

/// The handler view of a store account.
pub(crate) fn context(account: &Account) -> Result<AccountContext, HttpError> {
    Ok(AccountContext {
        id: account.id,
        jmap_id: jmap_account_id(account.id).ok_or(HttpError::Corrupt("account id"))?,
        login: account.login.clone(),
        email: account.email.clone(),
    })
}

impl<B: Backend> Shared<B> {
    pub(crate) fn new(
        config: HttpConfig,
        store: Store,
        keyring: Keyring,
        backend: B,
        shutdown: CancellationToken,
    ) -> Self {
        let limiter = LoginLimiter::new(config.login_limits);
        let api_slots = Slots::new(config.jmap.max_concurrent_requests());
        let upload_slots = Slots::new(config.jmap.max_concurrent_upload());
        let stream_slots = Slots::new(config.jmap.event_streams_per_account());
        Self {
            config,
            store,
            keyring,
            backend,
            shutdown,
            clock: Arc::new(UnixTime::now),
            limiter,
            pending: PendingAuthorizations::default(),
            client_auth: ClientAuth::default(),
            api_slots,
            upload_slots,
            stream_slots,
            downloads: Downloads::default(),
            download_wait: DOWNLOAD_WAIT,
            accounts: Mutex::new(Accounts::default()),
            lifecycle: Mutex::new(HashMap::new()),
        }
    }

    /// Replaces the clock (tests travel in time).
    #[cfg(test)]
    pub(crate) fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    pub(crate) fn now(&self) -> Result<UnixTime, HttpError> {
        Ok((self.clock)()?)
    }

    fn accounts(&self) -> std::sync::MutexGuard<'_, Accounts> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the account's lifecycle lock.
    async fn lock_account(&self, account: AccountId) -> AccountLock {
        let lock = {
            let mut locks = self
                .lifecycle
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Arc::clone(locks.entry(account).or_default())
        };
        AccountLock {
            _guard: lock.lock_owned().await,
            account,
        }
    }

    /// After the mail server accepted a login: creates or updates the
    /// account, stores the password sealed with the account's data key and
    /// starts the backend. The password is [SECRET] from cleanup while the
    /// returned guard lives: keep it until the login's code is stored.
    ///
    /// Logins are one account without case (`Alice` is `alice`), but only
    /// if the server agrees: when the typed spelling differs from the
    /// stored one, the password must also work for the stored one (a
    /// second LOGIN). On a server that tells them apart, `Alice` does not
    /// know `alice`'s password, so it cannot take over her account.
    ///
    /// # Errors
    ///
    /// [`HttpError::LoginMismatch`] if the second LOGIN is refused, and
    /// [`BackendError::Unavailable`] if it gets no verdict.
    pub(crate) async fn complete_login(
        &self,
        login: &LoginName,
        password: SecretString,
        email: &str,
    ) -> Result<(AccountContext, LoginInProgress<'_>), HttpError> {
        let now = self.now()?;
        let key_id = crypto_account(login.as_str())?;
        // Only used if the account is new; otherwise the stored key wins.
        let fresh = self.keyring.wrap(&key_id, &DataKey::generate()?)?;
        let account = self
            .store
            .get_or_create_account(
                NewAccount {
                    login: login.as_str(),
                    email,
                    wrapped_dek: fresh.as_bytes(),
                },
                now,
            )
            .await?;
        // Checked after the upsert, so two first logins at once cannot both
        // skip it, and before anything of the account is used or changed
        // (the upsert only touched `last_login_at`).
        if account.login != login.as_str() {
            let stored: LoginName = account
                .login
                .parse()
                .map_err(|_| HttpError::Corrupt("account login"))?;
            match self.backend.verify_login(&stored, &password).await {
                LoginOutcome::Success { .. } => {}
                // Another refusal counts as a failure too.
                LoginOutcome::WrongCredentials { .. } | LoginOutcome::Rejected => {
                    return Err(HttpError::LoginMismatch);
                }
                LoginOutcome::Unavailable => {
                    return Err(HttpError::Backend(BackendError::Unavailable));
                }
            }
        }
        let data_key = self
            .keyring
            .unwrap(&key_id, &WrappedDataKey::from_bytes(&account.wrapped_dek)?)?;
        let sealed = data_key.seal(
            &key_id,
            Field::ImapPassword,
            password.expose_secret().as_bytes(),
        )?;
        let lock = self.lock_account(account.id).await;
        let in_progress = {
            let mut accounts = self.accounts();
            let count = accounts.logging_in.entry(account.id).or_default();
            *count = count.saturating_add(1);
            LoginInProgress {
                accounts: &self.accounts,
                account: account.id,
            }
        };
        self.store
            .put_secret(account.id, SecretField::ImapPassword, sealed.as_bytes())
            .await?;

        let context = context(&account)?;
        let key = AccountKey::new(key_id, data_key);
        let started = self
            .backend
            .account_started(&context, password, key, StartReason::Login)
            .await;
        self.refused_locked(&lock, started).await?;
        self.accounts().started.insert(account.id);
        drop(lock);
        Ok((context, in_progress))
    }

    /// Makes sure the backend runs the account (after a restart, the first
    /// request of a session starts it with the stored password).
    pub(crate) async fn ensure_started(
        &self,
        account: &Account,
    ) -> Result<AccountContext, HttpError> {
        let context = context(account)?;
        if self.accounts().started.contains(&account.id) {
            return Ok(context);
        }
        // Under the lock, and checked again: a stop that ran first deleted
        // the password (so this fails), one that comes later sees the
        // account started (so it stops it).
        let lock = self.lock_account(account.id).await;
        if self.accounts().started.contains(&account.id) {
            return Ok(context);
        }
        let sealed = self
            .store
            .secret(account.id, SecretField::ImapPassword)
            .await?
            .ok_or(HttpError::NoSecret)?;
        let key_id = crypto_account(&account.login)?;
        let data_key = self
            .keyring
            .unwrap(&key_id, &WrappedDataKey::from_bytes(&account.wrapped_dek)?)?;
        let password = data_key.open_string(
            &key_id,
            Field::ImapPassword,
            &SealedField::from_bytes(&sealed)?,
        )?;
        let key = AccountKey::new(key_id, data_key);
        let started = self
            .backend
            .account_started(&context, password, key, StartReason::Restore)
            .await;
        self.refused_locked(&lock, started).await?;
        self.accounts().started.insert(account.id);
        drop(lock);
        Ok(context)
    }

    /// After a start: if the mail server refused the password, it goes at
    /// once (whatever held messages wait), and the account stops.
    async fn refused_locked(
        &self,
        lock: &AccountLock,
        started: Result<(), BackendError>,
    ) -> Result<(), HttpError> {
        if started == Err(BackendError::PasswordRefused) {
            self.stop_account_locked(lock).await?;
        }
        Ok(started?)
    }

    /// Starts the account if it has an active session, or held messages
    /// that go out after sign-out (used at boot).
    pub(crate) async fn start_if_active(&self, account: AccountId) -> Result<(), HttpError> {
        let now = self.now()?;
        if !self.store.has_active_session(account, now).await?
            && (self.held_while_signed_out(account, now).await? == 0
                || self
                    .store
                    .secret(account, SecretField::ImapPassword)
                    .await?
                    .is_none())
        {
            return Ok(());
        }
        let Some(row) = self.store.account(account).await? else {
            return Ok(());
        };
        self.ensure_started(&row).await.map(|_| ())
    }

    /// The accounts to start at boot for their held messages.
    pub(crate) async fn accounts_held_while_signed_out(&self) -> Result<Vec<AccountId>, HttpError> {
        let SignedOutSending::On { max_wait } = self.config.signed_out_sending else {
            return Ok(Vec::new());
        };
        Ok(self
            .store
            .accounts_held_while_signed_out(self.now()?, max_wait)
            .await?)
    }

    /// How many held messages of the account still need its password after
    /// sign-out (none when [`SignedOutSending::Off`]).
    async fn held_while_signed_out(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<u64, HttpError> {
        let SignedOutSending::On { max_wait } = self.config.signed_out_sending else {
            return Ok(0);
        };
        Ok(self
            .store
            .held_while_signed_out(account, now, max_wait)
            .await?)
    }

    /// The account's last session ended: delete its password, stop it.
    /// The lock proves no start is half done.
    async fn stop_account_locked(&self, lock: &AccountLock) -> Result<(), HttpError> {
        let account = lock.account;
        self.store.delete_secrets(account).await?;
        let was_kept = {
            let mut accounts = self.accounts();
            accounts.started.remove(&account);
            accounts.kept_for_held.remove(&account).is_some()
        };
        self.api_slots.remove(account);
        self.upload_slots.remove(account);
        self.stream_slots.remove(account);
        self.downloads.remove(account);
        self.backend.account_stopped(account).await;
        if was_kept {
            tracing::info!(
                account = account.get(),
                "account stopped: no scheduled message needs its password any more"
            );
        } else {
            tracing::info!(
                account = account.get(),
                "account stopped: last session ended"
            );
        }
        Ok(())
    }

    /// Whether nothing needs the account's password: no active session,
    /// no login waiting for its code exchange, none storing its code, and
    /// (after a normal end, see [`SessionEnd`]) no held message that goes
    /// out after sign-out.
    async fn is_idle(
        &self,
        account: AccountId,
        now: UnixTime,
        end: SessionEnd,
    ) -> Result<bool, HttpError> {
        if self.accounts().logging_in.contains_key(&account) {
            return Ok(false);
        }
        if self.store.has_active_grant(account, now).await?
            || self.store.has_pending_code(account, now).await?
        {
            self.accounts().kept_for_held.remove(&account);
            return Ok(false);
        }
        if end == SessionEnd::Compromised {
            return Ok(true);
        }
        let held = self.held_while_signed_out(account, now).await?;
        // A password already deleted (the mail server refused it) can't be
        // kept: the account just stops.
        if held == 0
            || self
                .store
                .secret(account, SecretField::ImapPassword)
                .await?
                .is_none()
        {
            return Ok(true);
        }
        let changed = self.accounts().kept_for_held.insert(account, held) != Some(held);
        if changed {
            tracing::info!(
                account = account.get(),
                count = held,
                "password kept after sign-out for {held} scheduled messages"
            );
        }
        Ok(false)
    }

    /// Stops the account if it has no active session left (nor a login
    /// on its way to one, nor, after a normal end, held messages that go
    /// out after sign-out). A session revoked because of a reused code or
    /// token ([`SessionEnd::Compromised`]) has no such login, and held
    /// messages don't count, so its password goes at once.
    pub(crate) async fn stop_if_idle(
        &self,
        account: AccountId,
        end: SessionEnd,
    ) -> Result<(), HttpError> {
        let lock = self.lock_account(account).await;
        if self.is_idle(account, self.now()?, end).await? {
            self.stop_account_locked(&lock).await?;
        } else {
            // Only one session ended: what was its own goes.
            self.backend.session_ended(account).await;
        }
        Ok(())
    }

    /// Periodic cleanup: expired codes and tokens, accounts without
    /// sessions (their passwords are deleted, and running ones stop, unless
    /// held messages keep them, until those are settled), and the
    /// in-memory maps.
    pub(crate) async fn cleanup(&self) -> Result<(), HttpError> {
        let now = self.now()?;
        let mut candidates: BTreeSet<AccountId> = self
            .store
            .cleanup_oauth(now, KEEP_ENDED_GRANTS)
            .await?
            .into_iter()
            .collect();
        // Also every running account: one started without a session (a
        // race, a bug) must not run on, and one kept for held messages
        // stops once they are settled.
        candidates.extend(self.accounts().started.iter().copied());
        for account in candidates {
            let lock = self.lock_account(account).await;
            if self.is_idle(account, now, SessionEnd::Ended).await? {
                self.stop_account_locked(&lock).await?;
            }
        }
        // Kept for held messages but not running (its start at boot
        // failed): try again, or they would wait for nothing.
        for account in self.accounts_held_while_signed_out().await? {
            if self.accounts().started.contains(&account) {
                continue;
            }
            if let Err(error) = self.start_if_active(account).await {
                tracing::warn!(
                    account = account.get(),
                    %error,
                    "account with scheduled messages not started"
                );
            }
        }
        self.lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, lock| Arc::strong_count(lock) > 1);
        self.store.cleanup_devices(now).await?;
        self.limiter.prune(now);
        self.client_auth.prune(now);
        self.pending.prune(now);
        Ok(())
    }
}
