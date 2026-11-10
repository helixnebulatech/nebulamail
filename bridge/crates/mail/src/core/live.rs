// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Live updates of an account, from login until
//! the account has no sessions:
//!
//! - first a full sync (retried with backoff while the server is down);
//! - then, together:
//!   - IDLE on INBOX over a second connection; any news → incremental sync
//!     of INBOX over the command connection. Without IDLE, INBOX is polled
//!     every minute. With NOTIFY (RFC 5465) the same connection also hears
//!     about the other mailboxes (`STATUS` → sync of that mailbox, `LIST`
//!     → mailbox list check), so they are up to date at once too;
//!   - every `status_interval` (2 min): LIST/STATUS of every mailbox and a
//!     sync of those that changed, then old hidden emails are purged. With
//!     NOTIFY working this is only a safety net: every 5th interval.
//! - A refused login stops everything; the account reports it (see
//!   [`Mail::auth_failures`](crate::Mail::auth_failures)) so its sessions
//!   end.
//! - Closing the account stops the syncs between two IMAP commands
//!   ([`MailAccount::request_stop`]), so [`Live::stop`] waits for one
//!   command at most, not a whole first sync.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use nebula_imap::proto::command::{Access, NotifyEvent, NotifyFilter, NotifyGroup};
use nebula_imap::proto::response::Untagged;
use nebula_imap::proto::types::{Capability, MailboxName};
use nebula_imap::{Authenticated, Client, Events, IdleEvent, SelectOptions, SyncLevel};
use nebula_store::{Role, UnixTime};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::account::MailAccount;
use crate::sync::Trigger;
use crate::{MailError, Unavailable};

/// How often INBOX is synced when the server has no IDLE.
const POLL_WITHOUT_IDLE: Duration = Duration::from_mins(1);
/// First wait after a failure; doubles up to `MAX_RETRY`.
const FIRST_RETRY: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_mins(5);
/// With NOTIFY, the periodic check runs every this many intervals.
const CHECK_EVERY_WITH_NOTIFY: u32 = 5;

/// The running live-update task of an account.
///
/// The task holds the account (`Arc`) and the account holds this, so
/// nothing would ever stop it by itself: dropping `Live` (when the account
/// is replaced, or [`Mail`](crate::Mail) is dropped without
/// [`Mail::close`](crate::Mail::close)) asks the task to stop. It finishes
/// at its next wait (after the IMAP command it is in, ending IDLE with
/// DONE), which also lets go of the account and its connections.
#[derive(Debug)]
pub(crate) struct Live {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl Live {
    pub(crate) fn start(account: Arc<MailAccount>) -> Self {
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(run(account, stopped));
        Self {
            stop,
            task: Some(task),
        }
    }

    /// Asks the task to stop and waits for it (IDLE is ended with DONE).
    pub(crate) async fn stop(mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Not aborted: a write to the store may be half way; the task stops
        // at its next wait.
        let _ = self.stop.send(true);
    }
}

/// What a loop does after an error.
enum Next {
    Retry,
    Stop,
}

/// Common to every loop of the task: a refused login (already reported
/// by the account) or the account closing stops it; anything else is
/// retried.
fn after_error(account: &MailAccount, what: &str, error: &MailError) -> Next {
    match error {
        MailError::AuthFailed => {
            tracing::warn!(account = %account.id, "IMAP login refused: stopping live sync");
            return Next::Stop;
        }
        MailError::Unavailable(Unavailable::Stopping) => return Next::Stop,
        _ => {}
    }
    tracing::warn!(account = %account.id, %error, "{what} failed");
    Next::Retry
}

/// Sleeps, unless asked to stop first. `true` if it should stop.
async fn sleep_or_stop(stopped: &mut watch::Receiver<bool>, wait: Duration) -> bool {
    if *stopped.borrow() {
        return true;
    }
    tokio::select! {
        _ = stopped.changed() => true,
        () = tokio::time::sleep(wait) => *stopped.borrow(),
    }
}

async fn run(account: Arc<MailAccount>, mut stopped: watch::Receiver<bool>) {
    let mut wait = FIRST_RETRY;
    loop {
        match account.sync_all().await {
            Ok(_) => break,
            Err(error) => {
                if let Next::Stop = after_error(&account, "first sync", &error) {
                    return;
                }
            }
        }
        if sleep_or_stop(&mut stopped, wait).await {
            return;
        }
        wait = wait.saturating_mul(2).min(MAX_RETRY);
    }
    let idle = idle_loop(&account, stopped.clone());
    let status = status_loop(&account, stopped.clone());
    tokio::join!(idle, status);
}

async fn status_loop(account: &MailAccount, mut stopped: watch::Receiver<bool>) {
    let interval = account.config().status_interval;
    let mut tick: u32 = 0;
    loop {
        if sleep_or_stop(&mut stopped, interval).await {
            return;
        }
        tick = tick.wrapping_add(1);
        let notified = account.notify_active.load(Ordering::Relaxed);
        if notified && !tick.is_multiple_of(CHECK_EVERY_WITH_NOTIFY) {
            purge(account).await;
            continue;
        }
        if let Err(error) = account.check_mailboxes().await
            && let Next::Stop = after_error(account, "mailbox check", &error)
        {
            return;
        }
        purge(account).await;
    }
}

async fn purge(account: &MailAccount) {
    let Ok(now) = UnixTime::now() else { return };
    let before = now.saturating_sub(account.config().orphan_ttl);
    if let Err(error) = account.store().purge_orphans(account.id, before).await {
        tracing::warn!(account = %account.id, %error, "purging hidden emails failed");
    }
}

async fn idle_loop(account: &MailAccount, mut stopped: watch::Receiver<bool>) {
    let mut wait = FIRST_RETRY;
    loop {
        let result = idle_session(account, &mut stopped).await;
        // Until the next session has NOTIFY working again.
        account.notify_active.store(false, Ordering::Relaxed);
        match result {
            Ok(()) => return,
            Err(error) => {
                if let Next::Stop = after_error(account, "IDLE", &error) {
                    return;
                }
            }
        }
        if sleep_or_stop(&mut stopped, wait).await {
            return;
        }
        wait = wait.saturating_mul(2).min(MAX_RETRY);
    }
}

/// Syncs INBOX under its backoff: a broken INBOX doesn't end the IDLE
/// session (that would only reconnect and fail again).
async fn sync_inbox(account: &MailAccount, trigger: Trigger) -> Result<(), MailError> {
    if let Some(inbox) = account
        .store()
        .mailbox_by_role(account.id, Role::Inbox)
        .await?
    {
        account.sync_mailbox_for(inbox.id, trigger).await?;
    }
    Ok(())
}

/// Asks for the events of every mailbox (NOTIFY). `false` if the server
/// has no NOTIFY or refused (then only INBOX is watched).
async fn start_notify(client: &mut Client<Authenticated>) -> Result<bool, MailError> {
    use NotifyEvent::{FlagChange, MailboxName, MessageExpunge, MessageNew, SubscriptionChange};
    if !client.capabilities().has(&Capability::Notify) {
        return Ok(false);
    }
    // FlagChange of other mailboxes comes as STATUS HIGHESTMODSEQ, which
    // needs CONDSTORE.
    let personal: &[NotifyEvent] = if client.sync_level() >= SyncLevel::CondStore {
        &[
            MessageNew,
            MessageExpunge,
            FlagChange,
            MailboxName,
            SubscriptionChange,
        ]
    } else {
        &[MessageNew, MessageExpunge, MailboxName, SubscriptionChange]
    };
    let groups = [
        NotifyGroup {
            filter: NotifyFilter::Selected,
            events: &[MessageNew, MessageExpunge, FlagChange],
        },
        NotifyGroup {
            filter: NotifyFilter::Personal,
            events: personal,
        },
    ];
    match client.notify(&groups).await {
        Ok(()) => Ok(true),
        Err(error) if !client.is_broken() => {
            tracing::info!(%error, "NOTIFY refused: watching INBOX only");
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// What a batch of IDLE news asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Wanted {
    inbox: bool,
    /// Other mailboxes (NOTIFY `STATUS`), by IMAP name.
    mailboxes: Vec<MailboxName>,
    /// The mailbox list changed (NOTIFY `LIST`).
    list: bool,
}

impl Wanted {
    /// Events were dropped (too many): INBOX and the whole mailbox list.
    fn everything() -> Self {
        Self {
            inbox: true,
            mailboxes: Vec::new(),
            list: true,
        }
    }
}

fn wanted(changes: Vec<Untagged>, selected: &MailboxName) -> Wanted {
    let mut wanted = Wanted::default();
    for change in changes {
        match change {
            Untagged::MailboxStatus(status) if status.mailbox == *selected => wanted.inbox = true,
            Untagged::MailboxStatus(status) => {
                if !wanted.mailboxes.contains(&status.mailbox) {
                    wanted.mailboxes.push(status.mailbox);
                }
            }
            Untagged::List(_) => wanted.list = true,
            _ => wanted.inbox = true,
        }
    }
    wanted
}

async fn sync_wanted(account: &MailAccount, wanted: Wanted) -> Result<(), MailError> {
    if wanted.inbox {
        sync_inbox(account, Trigger::News).await?;
    }
    let mut list = wanted.list;
    for name in wanted.mailboxes {
        match account.store().mailbox_by_name(account.id, &name).await? {
            Some(mailbox) => {
                account.sync_mailbox_for(mailbox.id, Trigger::News).await?;
            }
            // A mailbox we don't know yet: the list check finds it.
            None => list = true,
        }
    }
    if list {
        account.check_mailboxes().await?;
    }
    Ok(())
}

/// One IDLE connection, until it breaks (`Err`) or we are stopped (`Ok`).
async fn idle_session(
    account: &MailAccount,
    stopped: &mut watch::Receiver<bool>,
) -> Result<(), MailError> {
    account.ensure_running()?;
    if account.auth_failed() {
        return Err(MailError::AuthFailed);
    }
    let credentials = account.credentials();
    let mut client = match account
        .connector()
        .login(&credentials, account.alert_sink())
        .await
    {
        Ok(client) => client,
        Err(MailError::AuthFailed) => return Err(account.login_refused(&credentials)),
        Err(error) => return Err(error),
    };
    if !client.capabilities().has(&Capability::Idle) {
        let _ = client.logout().await;
        loop {
            if sleep_or_stop(stopped, POLL_WITHOUT_IDLE).await {
                return Ok(());
            }
            sync_inbox(account, Trigger::Background).await?;
        }
    }
    let options = SelectOptions {
        // Read-only: watching must not change `\Recent` or anything else.
        access: Access::ReadOnly,
        ..SelectOptions::default()
    };
    let notify = start_notify(&mut client).await?;
    let (mut selected, _) = client
        .select(&MailboxName::inbox(), options)
        .await
        .map_err(|error| MailError::from(error.into_parts().0))?;
    account.notify_active.store(notify, Ordering::Relaxed);
    // Catch up with what happened before IDLE started.
    sync_inbox(account, Trigger::Background).await?;
    if notify {
        // And in the other mailboxes, now that their news reach us.
        account.check_mailboxes().await?;
    }
    loop {
        // News from before IDLE: already covered by the syncs above, unless
        // so many came that they were dropped.
        if selected.take_events().overflowed {
            sync_wanted(account, Wanted::everything()).await?;
        }
        let mut idle = selected
            .idle()
            .await
            .map_err(|error| MailError::from(error.into_parts().0))?;
        let event = tokio::select! {
            _ = stopped.changed() => None,
            event = idle.wait() => Some(event),
        };
        let Some(event) = event else {
            if let Ok(client) = idle.done().await {
                let _ = client.logout().await;
            }
            return Ok(());
        };
        let event = event?;
        selected = idle.done().await?;
        let inbox = selected.mailbox().name.clone();
        if let Some(found) = after_idle(event, selected.take_events(), &inbox) {
            sync_wanted(account, found).await?;
        }
    }
}

/// What to sync after an IDLE round: its changes plus the news that came
/// while ending it (also after a `Refresh`, which by itself only re-issues
/// IDLE). `None` if there is nothing new.
fn after_idle(event: IdleEvent, events: Events, inbox: &MailboxName) -> Option<Wanted> {
    if events.overflowed {
        return Some(Wanted::everything());
    }
    let mut changes = match event {
        IdleEvent::Changes(changes) => changes,
        _ => Vec::new(),
    };
    changes.extend(events.items);
    (!changes.is_empty()).then(|| wanted(changes, inbox))
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::{FrameLimits, Response, ResponseDecoder};

    use super::*;

    fn untagged(wire: &str) -> Untagged {
        let mut decoder = ResponseDecoder::new(FrameLimits::default());
        decoder.feed(wire.as_bytes());
        match decoder.decode() {
            Ok(Some(Response::Untagged(data))) => data,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn idle_news_are_sorted_out() {
        let inbox = MailboxName::inbox();
        let changes = vec![
            untagged("* STATUS Work (MESSAGES 3 UIDNEXT 9)\r\n"),
            untagged("* STATUS Work (MESSAGES 4 UIDNEXT 10)\r\n"),
            untagged("* LIST () \"/\" New\r\n"),
        ];
        let found = wanted(changes, &inbox);
        assert_eq!(found.mailboxes, vec![MailboxName::new("Work").unwrap()]);
        assert!(found.list && !found.inbox);
        let found = wanted(vec![untagged("* 4 EXISTS\r\n")], &inbox);
        assert!(found.inbox && found.mailboxes.is_empty() && !found.list);
        let found = wanted(vec![untagged("* STATUS INBOX (MESSAGES 4)\r\n")], &inbox);
        assert!(found.inbox);
    }

    fn events(items: Vec<Untagged>, overflowed: bool) -> Events {
        let mut events = Events::default();
        events.items = items;
        events.overflowed = overflowed;
        events
    }

    #[test]
    fn refresh_still_syncs_pending_news() {
        let inbox = MailboxName::inbox();
        assert_eq!(
            after_idle(IdleEvent::Refresh, Events::default(), &inbox),
            None
        );
        // News that came while ending IDLE after a refresh are not lost.
        let found = after_idle(
            IdleEvent::Refresh,
            events(vec![untagged("* 5 EXISTS\r\n")], false),
            &inbox,
        )
        .unwrap();
        assert!(found.inbox);
        let found = after_idle(
            IdleEvent::Changes(vec![untagged("* LIST () \"/\" New\r\n")]),
            events(vec![untagged("* 5 EXISTS\r\n")], false),
            &inbox,
        )
        .unwrap();
        assert!(found.inbox && found.list);
        // Too many events: everything.
        assert_eq!(
            after_idle(IdleEvent::Refresh, events(Vec::new(), true), &inbox),
            Some(Wanted::everything())
        );
    }

    /// News about a mailbox that waits after a local failure: not tried
    /// (with a closed port, a try would fail with a connection error and
    /// end the IDLE session).
    #[tokio::test(flavor = "multi_thread")]
    async fn news_respect_the_mailbox_backoff() {
        use crate::sync::backoff::Failure;
        use crate::test_support::{closed_port, fixture};

        let f = fixture(closed_port()).await;
        let account = f.account();
        account
            .mailbox_backoff
            .failed(f.inbox, tokio::time::Instant::now(), Failure::Local, true);
        let news = Wanted {
            inbox: true,
            mailboxes: vec![MailboxName::inbox()],
            list: false,
        };
        assert!(sync_wanted(&account, news).await.is_ok());
        // Without the backoff it does try (and the connection fails).
        account.mailbox_backoff.succeeded(f.inbox);
        let news = Wanted {
            inbox: true,
            mailboxes: Vec::new(),
            list: false,
        };
        assert!(matches!(
            sync_wanted(&account, news).await,
            Err(MailError::Unavailable(_))
        ));
    }
}
