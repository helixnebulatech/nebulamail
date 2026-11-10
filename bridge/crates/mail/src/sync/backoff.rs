// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Backoff per mailbox: a mailbox whose sync failed on its
//! own (the server refused it, sent nonsense for it…) is not tried again
//! by the background syncs until its time comes: 1 min, then doubling up
//! to 15 min (5 min for INBOX, where new mail arrives). Kept in memory
//! only, and bounded by the account's mailboxes ([`MailboxBackoff::retain`]).
//!
//! A temporary refusal (RFC 5530 `UNAVAILABLE`, `INUSE`, `LIMIT`,
//! `SERVERBUG`) waits 1 min and never doubles. News about the mailbox
//! itself from IDLE/NOTIFY counts as a reason to try again once a minute
//! has passed since the failure. Operations the user waits for ignore it
//! (and update it).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nebula_imap::proto::response::ResponseCode;
use nebula_store::MailboxId;
use tokio::time::Instant;

use crate::MailError;

/// The first wait, and the shortest time between news-driven retries.
pub(crate) const FIRST: Duration = Duration::from_mins(1);
/// The longest wait.
pub(crate) const MAX: Duration = Duration::from_mins(15);
/// The longest wait for INBOX.
pub(crate) const MAX_INBOX: Duration = Duration::from_mins(5);

/// How a mailbox failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Something about the mailbox: waits double.
    Local,
    /// The server said "not now" (RFC 5530): a fixed short wait.
    Temporary,
}

impl Failure {
    pub(crate) fn of(error: &MailError) -> Self {
        match error {
            MailError::Refused(refusal)
                if matches!(
                    refusal.code,
                    Some(
                        ResponseCode::Unavailable
                            | ResponseCode::InUse
                            | ResponseCode::Limit
                            | ResponseCode::ServerBug
                    )
                ) =>
            {
                Self::Temporary
            }
            _ => Self::Local,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Local failures in a row (temporary ones don't count).
    failures: u32,
    failed_at: Instant,
    retry_at: Instant,
}

/// See the module docs.
#[derive(Debug, Default)]
pub(crate) struct MailboxBackoff(Mutex<HashMap<MailboxId, Entry>>);

impl MailboxBackoff {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<MailboxId, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a background sync may try the mailbox now.
    pub(crate) fn due(&self, id: MailboxId, now: Instant) -> bool {
        self.entries()
            .get(&id)
            .is_none_or(|entry| now >= entry.retry_at)
    }

    /// Whether news about the mailbox itself (IDLE/NOTIFY) may try it now:
    /// when it is due, or a minute after its last failure.
    pub(crate) fn due_for_news(&self, id: MailboxId, now: Instant) -> bool {
        self.entries().get(&id).is_none_or(|entry| {
            now >= entry.retry_at || now >= entry.failed_at.checked_add(FIRST).unwrap_or(now)
        })
    }

    /// Notes a failure; returns how long the mailbox now waits.
    pub(crate) fn failed(
        &self,
        id: MailboxId,
        now: Instant,
        failure: Failure,
        inbox: bool,
    ) -> Duration {
        let mut entries = self.entries();
        let failures = entries.get(&id).map_or(0, |entry| entry.failures);
        let (failures, wait) = match failure {
            Failure::Temporary => (failures, FIRST),
            Failure::Local => {
                let failures = failures.saturating_add(1);
                let max = if inbox { MAX_INBOX } else { MAX };
                let doublings = failures.saturating_sub(1).min(16);
                (failures, FIRST.saturating_mul(1 << doublings).min(max))
            }
        };
        entries.insert(
            id,
            Entry {
                failures,
                failed_at: now,
                retry_at: now.checked_add(wait).unwrap_or(now),
            },
        );
        wait
    }

    /// Forgets the failures of a mailbox that synced.
    pub(crate) fn succeeded(&self, id: MailboxId) {
        self.entries().remove(&id);
    }

    /// Forgets the mailboxes that are no longer there (after LIST).
    pub(crate) fn retain(&self, alive: &HashSet<MailboxId>) {
        self.entries().retain(|id, _| alive.contains(id));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use nebula_imap::Refusal;
    use nebula_imap::proto::response::Completion;

    use super::*;

    fn id(n: i64) -> MailboxId {
        MailboxId::from_raw(n).unwrap()
    }

    fn refused(code: Option<ResponseCode>) -> MailError {
        MailError::Refused(Refusal {
            command: "SELECT",
            status: Completion::No,
            code,
            text: "no".to_owned(),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn waits_double_up_to_the_cap() {
        let backoff = MailboxBackoff::default();
        let now = Instant::now();
        let mins = |n| Duration::from_mins(n);
        let waits: Vec<Duration> = (0..7)
            .map(|_| backoff.failed(id(1), now, Failure::Local, false))
            .collect();
        assert_eq!(
            waits,
            vec![
                mins(1),
                mins(2),
                mins(4),
                mins(8),
                mins(15),
                mins(15),
                mins(15)
            ]
        );
        // INBOX: at most 5 min.
        let waits: Vec<Duration> = (0..5)
            .map(|_| backoff.failed(id(2), now, Failure::Local, true))
            .collect();
        assert_eq!(waits, vec![mins(1), mins(2), mins(4), mins(5), mins(5)]);
        assert!(!backoff.due(id(1), now));
        assert!(!backoff.due(id(1), now + mins(14)));
        assert!(backoff.due(id(1), now + mins(15)));
        assert!(backoff.due(id(3), now));
    }

    #[tokio::test(start_paused = true)]
    async fn temporary_refusals_wait_a_minute_and_never_double() {
        let backoff = MailboxBackoff::default();
        let now = Instant::now();
        for code in [
            ResponseCode::Unavailable,
            ResponseCode::InUse,
            ResponseCode::Limit,
            ResponseCode::ServerBug,
        ] {
            assert_eq!(Failure::of(&refused(Some(code))), Failure::Temporary);
        }
        assert_eq!(Failure::of(&refused(None)), Failure::Local);
        assert_eq!(
            Failure::of(&refused(Some(ResponseCode::NoPerm))),
            Failure::Local
        );
        for _ in 0..5 {
            assert_eq!(backoff.failed(id(1), now, Failure::Temporary, false), FIRST);
        }
        // The local failures before are kept, not added to.
        backoff.failed(id(2), now, Failure::Local, false);
        backoff.failed(id(2), now, Failure::Temporary, false);
        assert_eq!(
            backoff.failed(id(2), now, Failure::Local, false),
            Duration::from_mins(2)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn news_retries_after_a_minute() {
        let backoff = MailboxBackoff::default();
        let now = Instant::now();
        for _ in 0..4 {
            backoff.failed(id(1), now, Failure::Local, false);
        }
        assert!(!backoff.due_for_news(id(1), now + Duration::from_secs(59)));
        assert!(backoff.due_for_news(id(1), now + FIRST));
        assert!(!backoff.due(id(1), now + FIRST));
    }

    #[tokio::test(start_paused = true)]
    async fn success_and_retain_forget() {
        let backoff = MailboxBackoff::default();
        let now = Instant::now();
        backoff.failed(id(1), now, Failure::Local, false);
        backoff.failed(id(2), now, Failure::Local, false);
        backoff.succeeded(id(1));
        assert!(backoff.due(id(1), now));
        assert_eq!(backoff.failed(id(1), now, Failure::Local, false), FIRST);
        backoff.retain(&HashSet::from([id(1)]));
        assert!(backoff.due(id(2), now));
        assert!(!backoff.due(id(1), now));
    }
}
