// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Live sync of an account, from login until the account
//! has no sessions:
//!
//! - discovery first (retried with backoff while the server fails);
//! - then a sync round, and again every `poll_with_push` (60 s) while the
//!   account has a push connection open, every `poll_without_push`
//!   (5 min) otherwise, or at once after a [`kick`](super::account::GroupwareAccount::kick);
//! - after the first round, a calendar and an address book if the homes
//!   have none ([`ensure_defaults`](super::account::GroupwareAccount::ensure_defaults));
//! - discovery again once a day, followed by a round that compares every
//!   collection whole with the server;
//! - hidden resources and collections older than `orphan_ttl` are deleted
//!   after each round.
//!
//! A 401 stops everything and broadcasts
//! [`GroupwareEvent::Unauthorized`](crate::GroupwareEvent::Unauthorized):
//! like a refused IMAP login, it is not retried by itself (the user logs
//! in again). Mail is not affected.

use std::sync::Arc;
use std::time::Duration;

use nebula_store::UnixTime;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::account::{DavStatus, GroupwareAccount};
use crate::GroupwareError;

/// The running live-sync task of an account. Dropping it asks the task to
/// stop at its next wait (as `nebula-mail`'s `Live`).
#[derive(Debug)]
pub(crate) struct Live {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    /// The alert scheduler, stopped with the sync.
    alerts: Option<JoinHandle<()>>,
}

impl Live {
    pub(crate) fn start(account: Arc<GroupwareAccount>) -> Self {
        let (stop, stopped) = watch::channel(false);
        let alerts = tokio::spawn(crate::alerts::run(Arc::clone(&account), stopped.clone()));
        let task = tokio::spawn(run(account, stopped));
        Self {
            stop,
            task: Some(task),
            alerts: Some(alerts),
        }
    }

    /// Asks the tasks to stop and waits for them, at most [`STOP_WAIT`]
    /// each: they stop at their next await ([`or_stop`]); one that does
    /// not by then is aborted.
    pub(crate) async fn stop(mut self) {
        let _ = self.stop.send(true);
        for task in [self.task.take(), self.alerts.take()].into_iter().flatten() {
            let abort = task.abort_handle();
            if tokio::time::timeout(STOP_WAIT, task).await.is_err() {
                tracing::warn!("calendar task did not stop in time, aborted");
                abort.abort();
            }
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Asked to stop, not aborted here: the tasks stop at their next
        // await (`or_stop`), which is as safe as aborting them. Every write
        // to the store is one transaction in `spawn_blocking`, which ends
        // or never starts when the future is dropped; and a sync cut
        // anywhere leaves a valid state (the token is saved last).
        let _ = self.stop.send(true);
    }
}

/// Longest wait for each task in [`Live::stop`].
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Runs `work` until it ends or the account is asked to stop (`None`):
/// a request to a server that takes minutes to answer does not hold the
/// stop back. Dropping `work` at an await is safe (see [`Live`]'s drop).
pub(crate) async fn or_stop<T>(
    stopped: &mut watch::Receiver<bool>,
    work: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = stopped.wait_for(|stop| *stop) => None,
        out = work => Some(out),
    }
}

/// What the loop does after an error.
enum Next {
    Retry,
    Stop,
}

fn after_error(account: &GroupwareAccount, what: &str, error: &GroupwareError) -> Next {
    if matches!(error, GroupwareError::Unauthorized) {
        tracing::warn!(
            account = %account.id,
            "DAV login refused: calendars and contacts are off for this account until it logs in again"
        );
        account.set_status(DavStatus::Unauthorized);
        account.emit_unauthorized();
        return Next::Stop;
    }
    tracing::warn!(account = %account.id, %error, "{what} failed");
    account.set_status(DavStatus::Failing);
    Next::Retry
}

/// Waits until `since + interval` (the interval follows the number of
/// push connections, read again whenever they change), a kick, or a stop.
/// `true` if it should stop.
async fn wait(
    account: &GroupwareAccount,
    stopped: &mut watch::Receiver<bool>,
    since: Instant,
    fixed: Option<Duration>,
) -> bool {
    loop {
        if *stopped.borrow() {
            return true;
        }
        let interval = fixed.unwrap_or_else(|| account.poll_interval());
        let deadline = since + interval;
        if Instant::now() >= deadline {
            return false;
        }
        tokio::select! {
            _ = stopped.changed() => return true,
            () = tokio::time::sleep_until(deadline) => return *stopped.borrow(),
            () = account.wake.notified() => {
                if account.take_kick() {
                    return *stopped.borrow();
                }
            }
        }
    }
}

fn discovery_due(account: &GroupwareAccount) -> bool {
    let Some(found) = account.discovered() else {
        return true;
    };
    UnixTime::now()
        .is_ok_and(|now| now >= found.at.saturating_add(account.config().discovery_interval))
}

async fn purge(account: &GroupwareAccount) {
    let Ok(now) = UnixTime::now() else { return };
    let before = now.saturating_sub(account.config().orphan_ttl);
    if let Err(error) = account.store().purge_dav_orphans(account.id, before).await {
        tracing::warn!(account = %account.id, %error, "purging hidden calendar data failed");
    }
}

async fn run(account: Arc<GroupwareAccount>, mut stopped: watch::Receiver<bool>) {
    let config = account.config();
    let mut retry = config.first_retry;
    // Discovery at login: until it works, nothing else can.
    loop {
        let Some(found) = or_stop(&mut stopped, account.discover()).await else {
            return;
        };
        match found {
            Ok(()) => break,
            Err(error) => {
                if let Next::Stop = after_error(&account, "DAV discovery", &error) {
                    return;
                }
            }
        }
        if wait(&account, &mut stopped, Instant::now(), Some(retry)).await {
            return;
        }
        retry = retry.saturating_mul(2).min(config.max_retry);
    }
    retry = config.first_retry;
    // Right after discovery, every collection is compared whole.
    let mut reconcile = true;
    // Once per login, after the first full round: a calendar and an
    // address book if the homes have none.
    let mut defaults_checked = false;
    loop {
        if !reconcile && discovery_due(&account) {
            let Some(found) = or_stop(&mut stopped, account.discover()).await else {
                return;
            };
            match found {
                Ok(()) => reconcile = true,
                Err(error) => {
                    if let Next::Stop = after_error(&account, "DAV discovery", &error) {
                        return;
                    }
                }
            }
        }
        let started = Instant::now();
        let Some(round) = or_stop(&mut stopped, account.sync_round(reconcile)).await else {
            return;
        };
        let failed = match round {
            Ok(_) => {
                if !defaults_checked {
                    defaults_checked = true;
                    if or_stop(&mut stopped, account.ensure_defaults())
                        .await
                        .is_none()
                    {
                        return;
                    }
                }
                account.set_status(DavStatus::Ready);
                reconcile = false;
                retry = config.first_retry;
                false
            }
            Err(error) => {
                if let Next::Stop = after_error(&account, "DAV sync", &error) {
                    return;
                }
                true
            }
        };
        if or_stop(&mut stopped, purge(&account)).await.is_none() {
            return;
        }
        let fixed = failed.then(|| retry.min(account.poll_interval()));
        if failed {
            retry = retry.saturating_mul(2).min(config.max_retry);
        }
        if wait(&account, &mut stopped, started, fixed).await {
            return;
        }
    }
}
