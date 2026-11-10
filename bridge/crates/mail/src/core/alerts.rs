// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The latest `[ALERT]`s of an account (RFC 3501 §7.1, RFC 9051 §7.1:
//! they MUST be presented to the user). Every connection of the account
//! sends its alerts here; the JMAP session shows them to the web app.
//!
//! Bounded: at most [`MAX_ALERTS`], each kept [`ALERT_TTL`]. The same text
//! again (servers repeat it at every login, and the bridge opens several
//! connections) keeps the first one, with its id, so a notice the user
//! closed stays closed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_imap::{AlertSink, ServerAlert};

/// Alerts kept per account.
pub const MAX_ALERTS: usize = 5;

/// How long an alert is shown.
pub const ALERT_TTL: Duration = Duration::from_hours(24);

/// One alert of the mail server.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccountAlert {
    /// Stable while the alert is kept, unique (time and a counter), so the
    /// web app can remember which ones the user closed.
    pub id: String,
    /// The cleaned text (plain text: escape it to show it).
    pub text: ServerAlert,
    /// When it first arrived.
    pub received_at: SystemTime,
}

/// The alerts of one account.
#[derive(Debug, Default)]
pub(crate) struct AlertLog {
    entries: Mutex<VecDeque<AccountAlert>>,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl AlertLog {
    pub(crate) fn add(&self, text: ServerAlert, now: SystemTime) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        expire(&mut entries, now);
        if entries.iter().any(|entry| entry.text == text) {
            return;
        }
        let millis = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        entries.push_back(AccountAlert {
            id: format!("a{millis:x}-{seq:x}"),
            text,
            received_at: now,
        });
        while entries.len() > MAX_ALERTS {
            entries.pop_front();
        }
    }

    /// The alerts not older than [`ALERT_TTL`], oldest first.
    pub(crate) fn current(&self, now: SystemTime) -> Vec<AccountAlert> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        expire(&mut entries, now);
        entries.iter().cloned().collect()
    }
}

fn expire(entries: &mut VecDeque<AccountAlert>, now: SystemTime) {
    entries.retain(|entry| {
        now.duration_since(entry.received_at)
            .map_or(true, |age| age < ALERT_TTL)
    });
}

impl AlertSink for AlertLog {
    fn alert(&self, alert: ServerAlert) {
        self.add(alert, SystemTime::now());
    }
}

/// Collects the alerts of a single login check (no account yet).
#[derive(Debug, Default)]
pub(crate) struct Collected(pub(crate) Mutex<Vec<ServerAlert>>);

impl AlertSink for Collected {
    fn alert(&self, alert: ServerAlert) {
        let mut list = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if list.len() < MAX_ALERTS && !list.contains(&alert) {
            list.push(alert);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(text: &str) -> ServerAlert {
        ServerAlert::new(text).unwrap()
    }

    #[test]
    fn bounded_deduplicated_and_expiring() {
        let log = AlertLog::default();
        let start = UNIX_EPOCH + Duration::from_hours(500_000);
        log.add(alert("Quota at 95%"), start);
        let first = log.current(start);
        // The same text again keeps the first one (and its id).
        log.add(alert("Quota at 95%"), start + Duration::from_secs(60));
        assert_eq!(log.current(start + Duration::from_secs(60)), first);
        for n in 0..MAX_ALERTS {
            log.add(
                alert(&format!("notice {n}")),
                start + Duration::from_secs(120),
            );
        }
        let now = start + Duration::from_secs(120);
        let texts: Vec<String> = log
            .current(now)
            .iter()
            .map(|entry| entry.text.to_string())
            .collect();
        assert_eq!(texts.len(), MAX_ALERTS);
        assert_eq!(texts.first().map(String::as_str), Some("notice 0"));
        let ids: std::collections::HashSet<String> =
            log.current(now).into_iter().map(|entry| entry.id).collect();
        assert_eq!(ids.len(), MAX_ALERTS);
        assert!(log.current(now + ALERT_TTL).is_empty());
    }
}
