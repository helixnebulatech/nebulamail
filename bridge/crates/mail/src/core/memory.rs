// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Memory for whole messages: one budget for the whole
//! bridge, and one turn per account for loading a message the cache can't
//! hold.
//!
//! Everything the bridge holds whole (downloads, `Email/get` of one
//! message, the blobs and the message of an `Email/set` create, the blobs
//! of `Email/import` and their copy in the `APPEND`, a submission and its
//! 7-bit copy, MDNs) takes its share with a [`Lease`]: bytes are counted
//! in permits of 1 MiB, asked for before loading (with the stored size)
//! and topped up with the real one. A lease waits up to
//! [`MailConfig::whole_wait`](crate::MailConfig) for its memory, then
//! fails with [`Unavailable::Busy`] (retryable: a 503, `serverUnavailable`
//! or "try again later"). Everything is given back when the lease is
//! dropped.
//!
//! The turn: a message that doesn't fit the cache (bigger than the stream
//! threshold, or of unknown size) is loaded from the server by one
//! operation of the account at a time, so four downloads of a 60 MiB
//! message hold one copy, not four, and one account can't take the whole
//! budget. The turn covers only the load and the whole copy from the
//! server: once the message is cut, sent to SMTP or handed to HTTP, the
//! holder calls [`Lease::drop_turn`] (the memory stays reserved). It is
//! reentrant: an operation that has it doesn't wait for itself. It is
//! waited for up to [`MailConfig::whole_turn_wait`](crate::MailConfig)
//! (longer than the memory: several `bp…` downloads of the same big
//! message wait for each other while the IMAP server sends it).
//!
//! The turn is always taken before the memory. An operation holding
//! memory for small things may still ask for the turn while another one
//! holding the turn asks for more memory: the waits end by their time
//! limit ("try again later"), never in a deadlock.
//!
//! An account opened again while the old one is still in use has its own
//! turn (as the HTTP download slots): two at most, for a moment.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{MailError, Unavailable};

/// One permit.
pub(crate) const MIB: u64 = 1024 * 1024;

/// The memory of the whole bridge for whole messages.
#[derive(Debug)]
pub struct MemoryBudget {
    permits: Arc<Semaphore>,
    total: u32,
    wait: Duration,
    turn_wait: Duration,
}

/// Permits (MiB) for `bytes`: none for nothing, and never more than the
/// whole budget (a 1 GiB message waits for all of it, not for ever).
fn permits_for(bytes: u64, total: u32) -> u32 {
    let mib = bytes.div_ceil(MIB);
    u32::try_from(mib).unwrap_or(u32::MAX).min(total)
}

fn busy() -> MailError {
    MailError::Unavailable(Unavailable::Busy)
}

impl MemoryBudget {
    /// A budget of `bytes` (at least 1 MiB), waited for up to `wait`; the
    /// turns of its leases are waited for up to `turn_wait`.
    #[must_use]
    pub fn new(bytes: u64, wait: Duration, turn_wait: Duration) -> Arc<Self> {
        let total = u32::try_from(bytes.div_ceil(MIB))
            .unwrap_or(u32::MAX)
            .clamp(1, u32::try_from(Semaphore::MAX_PERMITS).unwrap_or(u32::MAX));
        Arc::new(Self {
            permits: Arc::new(Semaphore::new(usize::try_from(total).unwrap_or(usize::MAX))),
            total,
            wait,
            turn_wait,
        })
    }

    /// A lease with a turn of its own (not an account's: it can't load
    /// messages, see [`MailAccount::load_message`](crate::MailAccount)).
    #[must_use]
    pub fn lease(self: &Arc<Self>) -> Lease {
        Lease::new(Arc::clone(self), Arc::new(Semaphore::new(1)))
    }

    /// The bytes not reserved by any lease (whole MiB).
    #[must_use]
    pub fn available(&self) -> u64 {
        u64::try_from(self.permits.available_permits())
            .unwrap_or(u64::MAX)
            .saturating_mul(MIB)
    }
}

/// The memory and the turn of one operation; see the module docs. Given
/// back when dropped.
pub struct Lease {
    budget: Arc<MemoryBudget>,
    turns: Arc<Semaphore>,
    /// What it covers (exact; the permits are this rounded up).
    bytes: u64,
    permit: Option<OwnedSemaphorePermit>,
    turn: Option<OwnedSemaphorePermit>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("bytes", &self.bytes)
            .field("turn", &self.turn.is_some())
            .finish_non_exhaustive()
    }
}

impl Lease {
    pub(crate) fn new(budget: Arc<MemoryBudget>, turns: Arc<Semaphore>) -> Self {
        Self {
            budget,
            turns,
            bytes: 0,
            permit: None,
            turn: None,
        }
    }

    /// The bytes it covers.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Whether it holds its account's turn.
    #[must_use]
    pub fn has_turn(&self) -> bool {
        self.turn.is_some()
    }

    pub(crate) fn turns(&self) -> &Arc<Semaphore> {
        &self.turns
    }

    fn held_permits(&self) -> u32 {
        self.permit.as_ref().map_or(0, |permit| {
            u32::try_from(permit.num_permits()).unwrap_or(u32::MAX)
        })
    }

    /// Covers `bytes` more, waiting for the memory if needed.
    ///
    /// # Errors
    /// [`Unavailable::Busy`] if it isn't free within the wait; then the
    /// lease stays as it was.
    pub async fn reserve(&mut self, bytes: u64) -> Result<(), MailError> {
        self.resize(self.bytes.saturating_add(bytes)).await
    }

    /// Covers exactly `bytes`: waits for more memory, or gives back what
    /// is left over.
    ///
    /// # Errors
    /// As [`Self::reserve`].
    pub async fn resize(&mut self, bytes: u64) -> Result<(), MailError> {
        let wanted = permits_for(bytes, self.budget.total);
        let held = self.held_permits();
        if let Some(more) = wanted.checked_sub(held).filter(|more| *more > 0) {
            let acquired = tokio::time::timeout(
                self.budget.wait,
                Arc::clone(&self.budget.permits).acquire_many_owned(more),
            )
            .await;
            let Ok(Ok(extra)) = acquired else {
                tracing::warn!("the memory for whole messages is full; try again later");
                return Err(busy());
            };
            match &mut self.permit {
                Some(permit) => permit.merge(extra),
                None => self.permit = Some(extra),
            }
        }
        self.bytes = bytes;
        self.shrink();
        Ok(())
    }

    /// Covers `bytes` less (never below zero).
    pub fn release(&mut self, bytes: u64) {
        self.bytes = self.bytes.saturating_sub(bytes);
        self.shrink();
    }

    /// Covers at most `bytes` and gives back the turn: for a lease of one
    /// thing that is now smaller (the part cut from a message).
    pub fn keep_only(&mut self, bytes: u64) {
        self.bytes = self.bytes.min(bytes);
        self.shrink();
        self.drop_turn();
    }

    /// Gives back the turn; the memory stays reserved. Call it once the
    /// whole copy from the server is done with (cut, or about to be sent
    /// over the network).
    pub fn drop_turn(&mut self) {
        self.turn = None;
    }

    /// Gives back everything.
    pub fn clear(&mut self) {
        self.bytes = 0;
        self.permit = None;
        self.turn = None;
    }

    /// Gives back the permits not needed for `self.bytes`.
    fn shrink(&mut self) {
        let wanted = permits_for(self.bytes, self.budget.total);
        if wanted == 0 {
            self.permit = None;
            return;
        }
        if let Some(permit) = &mut self.permit
            && let Some(extra) = permit
                .num_permits()
                .checked_sub(usize::try_from(wanted).unwrap_or(usize::MAX))
                .filter(|extra| *extra > 0)
        {
            drop(permit.split(extra));
        }
    }

    /// Takes the account's turn (no wait if it has it already).
    ///
    /// # Errors
    /// [`Unavailable::Busy`] if another operation keeps it for longer than
    /// the wait.
    pub(crate) async fn take_turn(&mut self) -> Result<(), MailError> {
        if self.turn.is_some() {
            return Ok(());
        }
        let acquired = tokio::time::timeout(
            self.budget.turn_wait,
            Arc::clone(&self.turns).acquire_owned(),
        )
        .await;
        let Ok(Ok(turn)) = acquired else {
            tracing::warn!("another big message of the account is being loaded; try again later");
            return Err(busy());
        };
        self.turn = Some(turn);
        Ok(())
    }
}

/// How many copies of a loaded message an operation holds at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Copies {
    /// The message as it is.
    One,
    /// The message and one more copy as big (a part cut from it, the
    /// `APPEND` literal, an `OutgoingMessage`).
    Two,
}

impl Copies {
    pub(crate) fn count(self) -> u64 {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use super::*;

    const WAIT: Duration = Duration::from_secs(30);
    const TURN_WAIT: Duration = Duration::from_secs(120);

    fn budget(mib: u64) -> Arc<MemoryBudget> {
        MemoryBudget::new(mib * MIB, WAIT, TURN_WAIT)
    }

    #[test]
    fn permits_are_mib_and_capped() {
        assert_eq!(permits_for(0, 4), 0);
        assert_eq!(permits_for(1, 4), 1);
        assert_eq!(permits_for(MIB, 4), 1);
        assert_eq!(permits_for(MIB + 1, 4), 2);
        assert_eq!(permits_for(1024 * MIB, 4), 4);
        assert_eq!(permits_for(u64::MAX, 256), 256);
        assert_eq!(budget(0).total, 1);
    }

    #[tokio::test]
    async fn a_lease_adds_up() {
        let budget = budget(8);
        let mut lease = budget.lease();
        lease.reserve(MIB).await.unwrap();
        lease.reserve(MIB + 1).await.unwrap();
        assert_eq!(lease.bytes(), 2 * MIB + 1);
        assert_eq!(budget.available(), 5 * MIB);
        lease.release(MIB);
        assert_eq!(budget.available(), 6 * MIB);
        lease.release(u64::MAX);
        assert_eq!(lease.bytes(), 0);
        assert_eq!(budget.available(), 8 * MIB);
        // Exact sizes, up and down.
        lease.resize(5 * MIB).await.unwrap();
        assert_eq!(budget.available(), 3 * MIB);
        lease.resize(2 * MIB).await.unwrap();
        assert_eq!(budget.available(), 6 * MIB);
        drop(lease);
        assert_eq!(budget.available(), 8 * MIB);
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_budget_waits_then_is_busy() {
        let budget = budget(2);
        let mut first = budget.lease();
        first.reserve(2 * MIB).await.unwrap();
        // Freed in time: the second one goes on.
        let waiting = {
            let budget = Arc::clone(&budget);
            tokio::spawn(async move { budget.lease().reserve(MIB).await.is_ok() })
        };
        tokio::time::sleep(WAIT / 2).await;
        drop(first);
        assert!(waiting.await.unwrap());
        // Never freed: busy once the wait is over, and nothing changed.
        let mut all = budget.lease();
        all.reserve(2 * MIB).await.unwrap();
        let started = tokio::time::Instant::now();
        let mut late = budget.lease();
        assert!(matches!(
            late.reserve(MIB).await,
            Err(MailError::Unavailable(Unavailable::Busy))
        ));
        assert!(started.elapsed() >= WAIT);
        assert_eq!(late.bytes(), 0);
        assert!(MailError::Unavailable(Unavailable::Busy).is_retryable());
    }

    #[tokio::test(start_paused = true)]
    async fn one_turn_per_account() {
        let budget = budget(8);
        let turns = Arc::new(Semaphore::new(1));
        let mut first = Lease::new(Arc::clone(&budget), Arc::clone(&turns));
        first.take_turn().await.unwrap();
        // Reentrant: no wait for itself.
        first.take_turn().await.unwrap();
        assert!(first.has_turn());
        let waiting = {
            let (budget, turns) = (Arc::clone(&budget), Arc::clone(&turns));
            tokio::spawn(async move {
                let mut second = Lease::new(budget, turns);
                second.take_turn().await.map(|()| second.has_turn())
            })
        };
        tokio::time::sleep(TURN_WAIT / 2).await;
        assert!(!waiting.is_finished());
        drop(first);
        assert!(waiting.await.unwrap().unwrap());
        // Kept: busy after the wait.
        let mut holder = Lease::new(Arc::clone(&budget), Arc::clone(&turns));
        holder.take_turn().await.unwrap();
        let started = tokio::time::Instant::now();
        let mut late = Lease::new(Arc::clone(&budget), Arc::clone(&turns));
        assert!(matches!(
            late.take_turn().await,
            Err(MailError::Unavailable(Unavailable::Busy))
        ));
        assert!(started.elapsed() >= TURN_WAIT);
        // Another account has its own.
        let mut other = budget.lease();
        other.take_turn().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_turn_keeps_the_memory() {
        let budget = budget(8);
        let turns = Arc::new(Semaphore::new(1));
        let mut sending = Lease::new(Arc::clone(&budget), Arc::clone(&turns));
        sending.take_turn().await.unwrap();
        sending.reserve(3 * MIB).await.unwrap();
        sending.drop_turn();
        assert!(!sending.has_turn());
        assert_eq!(budget.available(), 5 * MIB);
        // The next big load of the account doesn't wait.
        let mut next = Lease::new(Arc::clone(&budget), Arc::clone(&turns));
        let started = tokio::time::Instant::now();
        next.take_turn().await.unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
        // keep_only: smaller, and no turn either.
        next.reserve(4 * MIB).await.unwrap();
        next.keep_only(MIB);
        assert!(!next.has_turn());
        assert_eq!(next.bytes(), MIB);
        assert_eq!(budget.available(), 4 * MIB);
        next.keep_only(10 * MIB);
        assert_eq!(next.bytes(), MIB);
        next.clear();
        assert_eq!(budget.available(), 5 * MIB);
    }
}
