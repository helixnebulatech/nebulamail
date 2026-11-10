// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The data timeouts of RFC 5321 §4.5.3.2, measured on the socket.
//!
//! lettre sends the message and reads the final reply in one call, so one
//! timeout around that call can only be a limit on the whole transfer: a
//! big message on a slow link was cut although it was moving. The RFC asks
//! for two other limits instead:
//!
//! - **Data block** (3 min): while the message goes out, each write must
//!   make progress; only a connection where nothing has gone out for that
//!   long is given up.
//! - **DATA termination** (10 min): after the last byte, the time the
//!   server may take for its reply (spam and virus checks).
//!
//! [`Activity`] is shared between the stream under TLS and lettre (where it
//! sees every byte that really leaves) and the code that sends the message,
//! which watches it with [`Activity::watch`].

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::error::{SmtpError, Stage};

#[derive(Debug, Default)]
struct State {
    /// Set while a message is being sent.
    watching: bool,
    /// The last time bytes went out (or the message started).
    last_write: Option<Instant>,
    /// When lettre first asked for the reply: everything was written.
    reading_since: Option<Instant>,
}

/// What the socket did while a message was sent. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub(crate) struct Activity(Arc<Mutex<State>>);

impl Activity {
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Called by the stream when `count` bytes were written.
    pub(crate) fn wrote(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.with(|state| {
            if state.watching && state.reading_since.is_none() {
                state.last_write = Some(Instant::now());
            }
        });
    }

    /// Called by the stream when lettre reads: while a message is sent,
    /// the first read means the whole message was written (lettre only
    /// reads the reply after writing and flushing everything).
    pub(crate) fn reading(&self) {
        self.with(|state| {
            if state.watching && state.reading_since.is_none() {
                state.reading_since = Some(Instant::now());
            }
        });
    }

    fn start(&self) {
        self.with(|state| {
            *state = State {
                watching: true,
                last_write: Some(Instant::now()),
                reading_since: None,
            };
        });
    }

    fn stop(&self) {
        self.with(|state| *state = State::default());
    }

    /// When the current limit runs out, and which one it is.
    fn deadline(&self, block: Duration, end: Duration) -> (Instant, Duration) {
        self.with(|state| match (state.reading_since, state.last_write) {
            (Some(since), _) => (since + end, end),
            (None, Some(last)) => (last + block, block),
            (None, None) => (Instant::now() + block, block),
        })
    }

    /// Runs `send` (lettre writing the message and reading the reply) with
    /// the data-block and DATA-termination timeouts. A timeout is at
    /// [`Stage::DataEnd`]: the server may have the whole message, so the
    /// outcome is unknown.
    pub(crate) async fn watch<T>(
        &self,
        block: Duration,
        end: Duration,
        send: impl Future<Output = T>,
    ) -> Result<T, SmtpError> {
        self.start();
        let watchdog = async {
            loop {
                let (deadline, after) = self.deadline(block, end);
                tokio::time::sleep_until(deadline).await;
                let (now_deadline, _) = self.deadline(block, end);
                if now_deadline <= Instant::now() {
                    return after;
                }
                // Something happened meanwhile: wait for the new deadline.
            }
        };
        let result = tokio::select! {
            result = send => Ok(result),
            after = watchdog => Err(SmtpError::Timeout {
                stage: Stage::DataEnd,
                after,
            }),
        };
        self.stop();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: Duration = Duration::from_mins(3);
    const END: Duration = Duration::from_mins(10);

    #[tokio::test(start_paused = true)]
    async fn a_slow_transfer_that_moves_is_never_cut() {
        let activity = Activity::default();
        let sender = activity.clone();
        // 20 minutes of writes, one every 2 minutes, then the reply.
        let send = async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_mins(2)).await;
                sender.wrote(4096);
            }
            sender.reading();
            tokio::time::sleep(Duration::from_mins(1)).await;
            "250 queued"
        };
        assert_eq!(
            activity.watch(BLOCK, END, send).await.unwrap(),
            "250 queued"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_transfer_is_cut_after_the_block_timeout() {
        let activity = Activity::default();
        let sender = activity.clone();
        let started = Instant::now();
        let send = async move {
            sender.wrote(4096);
            // Nothing more goes out.
            std::future::pending::<()>().await;
        };
        let error = activity.watch(BLOCK, END, send).await.unwrap_err();
        assert!(matches!(
            error,
            SmtpError::Timeout {
                stage: Stage::DataEnd,
                after: BLOCK
            }
        ));
        assert!(error.outcome_unknown());
        assert_eq!(started.elapsed(), BLOCK);
    }

    #[tokio::test(start_paused = true)]
    async fn the_server_gets_ten_minutes_for_its_reply() {
        let activity = Activity::default();
        let sender = activity.clone();
        // A reply after 9 minutes is fine…
        let send = async move {
            sender.wrote(100);
            sender.reading();
            tokio::time::sleep(Duration::from_mins(9)).await;
            "250 queued"
        };
        assert_eq!(
            activity.watch(BLOCK, END, send).await.unwrap(),
            "250 queued"
        );
        // …none after 10 is not, and writes don't extend it.
        let sender = activity.clone();
        let started = Instant::now();
        let send = async move {
            sender.reading();
            loop {
                tokio::time::sleep(Duration::from_mins(1)).await;
                sender.wrote(1);
            }
        };
        let error = activity.watch(BLOCK, END, send).await.unwrap_err();
        assert!(matches!(error, SmtpError::Timeout { after: END, .. }));
        assert_eq!(started.elapsed(), END);
    }

    #[test]
    fn nothing_is_recorded_outside_a_message() {
        let activity = Activity::default();
        activity.wrote(10);
        activity.reading();
        activity.with(|state| {
            assert!(!state.watching);
            assert!(state.last_write.is_none());
            assert!(state.reading_since.is_none());
        });
    }
}
