// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduler of held messages (RFC 4865 FUTURERELEASE)
//! it sends each one when it is due, through the same SMTP path
//! as an immediate send, with the account's login.
//!
//! It sleeps until the next one is due (at most a minute), or until
//! `Inner::held_wake` (a new held message, an account that started). Each
//! due message is taken (`sending`: a cancel is now too late), opened with
//! the account's data key, sent, and settled:
//!
//! - sent: `final` with the server's reply; the Sent copy is appended if
//!   the client didn't file the email there;
//! - outcome unknown (the connection broke after the message was written):
//!   `final` with an empty reply ("unknown"), never sent again;
//! - refused for good (5xx, invalid recipients…): `final`, every recipient
//!   `delivered: no` with the reason;
//! - temporary trouble (network, 4xx, the login refused, the bridge busy):
//!   `pending` again, retried after 1, 2, 4… minutes (at most 30);
//! - the account is not signed in (its login and data key only exist
//!   while it runs): `pending`, looked at again every 5 minutes and at once
//!   when the account starts.
//!
//! Nothing is dropped silently: once a message has waited `max_wait` past
//! its time, it fails as above ("not sent: …"), visible in
//! `deliveryStatus`. A message the bridge stopped while sending is settled
//! `final` with an unknown outcome at the next start.
//!
//! Logs carry the account id and counts only: no address, subject or text.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use nebula_crypto::{Binding, Field, SealedField};
use nebula_jmap_proto::object::Envelope as JmapEnvelope;
use nebula_mail::MailAccount;
use nebula_smtp::{OutgoingMessage, SmtpError};
use nebula_store::{
    AccountId, ClaimedHeld, DataType, Delivered, DueHeld, RecipientStatus, Release, UnixTime,
};
use tokio_util::sync::CancellationToken;

use super::send::{self, SendError};
use super::set::{append_sent, envelope_to_smtp, held_binding, own_envid, sent_mailbox_of};
use crate::core::backend::{Inner, SmtpLogin};

/// The longest sleep between two looks at the store.
const MAX_SLEEP: Duration = Duration::from_mins(1);
/// Due messages taken in one look; more are taken right after.
const BATCH: i64 = 50;
/// Messages being sent at once.
const PARALLEL: usize = 4;
/// How often a message waiting for its account is looked at again (it is
/// also woken when the account starts).
const NOT_SIGNED_IN_RETRY: Duration = Duration::from_mins(5);
/// The first wait after a temporary failure; it doubles each time.
const FIRST_RETRY: Duration = Duration::from_mins(1);
/// The longest wait after a temporary failure.
const MAX_RETRY: Duration = Duration::from_mins(30);

/// Why a message is not sent this time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// Sent (or unknown, or failed for good): settled.
    Settled(Release),
    /// Not now: try again after a while (`failed` counts as an attempt).
    Later { failed: bool, why: String },
}

/// The wait after `attempts` temporary failures.
fn backoff(attempts: u32) -> Duration {
    let factor = 1_u32.checked_shl(attempts.min(16)).unwrap_or(u32::MAX);
    FIRST_RETRY.saturating_mul(factor).min(MAX_RETRY)
}

/// Every envelope recipient `delivered: no`, with `reason` (or, for the
/// ones the server named, its own reply).
fn failed_status(
    envelope: &JmapEnvelope,
    reason: &str,
    rejected: &[(String, String)],
) -> std::collections::BTreeMap<String, RecipientStatus> {
    envelope
        .rcpt_to
        .iter()
        .map(|rcpt| {
            let address = rcpt.email.as_str();
            let reply = rejected
                .iter()
                .find(|(who, _)| who.eq_ignore_ascii_case(address))
                .map_or_else(|| reason.to_owned(), |(_, reply)| reply.clone());
            (
                address.to_owned(),
                RecipientStatus {
                    smtp_reply: reply,
                    delivered: Delivered::No,
                    displayed: false,
                },
            )
        })
        .collect()
}

/// What a failed send means for a held message.
fn outcome_of(error: &SendError, envelope: &JmapEnvelope) -> Outcome {
    let temporary = |why: String| Outcome::Later { failed: true, why };
    let fail = |reason: String, rejected: Vec<(String, String)>| {
        Outcome::Settled(Release::Failed {
            status: failed_status(envelope, &reason, &rejected),
        })
    };
    let smtp = match error {
        SendError::Connect(_) => return temporary("the mail server can't be reached".to_owned()),
        SendError::Busy(_) => return temporary("the bridge is busy".to_owned()),
        SendError::LineTooLong => {
            return fail(
                "not sent: a line is longer than 998 octets (RFC 5322 §2.1.1)".to_owned(),
                vec![],
            );
        }
        SendError::Smtp(smtp) => smtp,
    };
    if smtp.outcome_unknown() {
        return Outcome::Settled(Release::Unknown);
    }
    match smtp {
        // The password may have changed: the account stops, and the
        // message waits for the next login.
        SmtpError::Auth { .. } => temporary("the SMTP server refused the login".to_owned()),
        other if other.is_transient() => temporary(other.to_string()),
        SmtpError::RecipientsRejected { rejected } => fail(
            "not sent: other recipients were refused".to_owned(),
            rejected
                .iter()
                .map(|r| {
                    (
                        r.recipient.to_string(),
                        format!("{} {}", r.code, r.text.as_str()),
                    )
                })
                .collect(),
        ),
        SmtpError::Rejected { code, text, .. } => fail(format!("{code} {}", text.as_str()), vec![]),
        _ => {
            let set = send::set_error(error);
            fail(
                format!(
                    "not sent: {}",
                    set.description().unwrap_or("the message could not be sent")
                ),
                vec![],
            )
        }
    }
}

/// The current time, to the second.
fn now() -> Option<UnixTime> {
    UnixTime::now().ok()
}

/// How long until `at` (zero if it is past).
fn until(at: UnixTime) -> Duration {
    let target = u64::try_from(at.secs()).map_or(UNIX_EPOCH, |secs| {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(secs))
            .unwrap_or(UNIX_EPOCH)
    });
    target
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
}

/// Runs until `shutdown`. See the module docs.
pub(crate) async fn run(inner: Arc<Inner>, shutdown: CancellationToken) {
    let lifetime = inner.config.submission_lifetime;
    if let Some(now) = now() {
        match inner.store.settle_interrupted_held(now, lifetime).await {
            Ok(0) => {}
            Ok(count) => tracing::warn!(
                count,
                "held messages were being sent when the bridge stopped: their outcome is unknown"
            ),
            Err(error) => tracing::error!(error = %error, "held messages could not be settled"),
        }
    }
    loop {
        let full = release_due(&inner).await;
        if full {
            continue;
        }
        let sleep = match inner.store.next_held_attempt().await {
            Ok(Some(next)) => until(next).min(MAX_SLEEP),
            Ok(None) => MAX_SLEEP,
            Err(error) => {
                tracing::error!(error = %error, "held messages could not be read");
                MAX_SLEEP
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = inner.held_wake.notified() => {}
            () = tokio::time::sleep(sleep) => {}
        }
    }
}

/// Sends what is due. Returns whether a whole batch was due (more may be).
async fn release_due(inner: &Inner) -> bool {
    let Some(now) = now() else {
        return false;
    };
    let due = match inner.store.due_held(now, BATCH).await {
        Ok(due) => due,
        Err(error) => {
            tracing::error!(error = %error, "held messages could not be read");
            return false;
        }
    };
    let full = i64::try_from(due.len()).is_ok_and(|len| len >= BATCH);
    futures::stream::iter(due)
        .for_each_concurrent(PARALLEL, |due| release_one(inner, due))
        .await;
    full
}

/// The account started: its overdue held messages are due now.
pub(crate) async fn account_started(inner: &Inner, account: AccountId) {
    let Some(now) = now() else {
        return;
    };
    match inner.store.wake_held(account, now).await {
        Ok(()) => inner.held_wake.notify_one(),
        Err(error) => tracing::warn!(error = %error, "held messages could not be woken"),
    }
}

async fn release_one(inner: &Inner, due: DueHeld) {
    let account = due.account;
    let Some(claimed) = (match inner.store.claim_held(account, due.id).await {
        Ok(claimed) => claimed,
        Err(error) => {
            tracing::error!(error = %error, "a held message could not be taken");
            return;
        }
    }) else {
        // Canceled meanwhile.
        return;
    };
    let Some(now) = now() else {
        return;
    };
    let deadline = due.send_at.saturating_add(inner.config.hold.max_wait);
    let login = inner.smtp_login(account);
    let mail = inner.mail.account(account);
    let outcome = match (login, mail) {
        (Some(login), Some(mail)) => send_claimed(inner, &login, &mail, &claimed).await,
        _ => Outcome::Later {
            failed: false,
            why: "not signed in".to_owned(),
        },
    };
    let release = match outcome {
        Outcome::Settled(release) => release,
        Outcome::Later { failed, why } => {
            let wait = if failed {
                backoff(claimed.attempts)
            } else {
                NOT_SIGNED_IN_RETRY
            };
            let at = now.saturating_add(wait);
            if at.secs() > deadline.secs() {
                tracing::warn!(
                    account = account.get(),
                    attempts = claimed.attempts,
                    "a held message waited too long and was not sent"
                );
                let reason = if failed {
                    format!("not sent: it could not be sent in time ({why})")
                } else {
                    "not sent: the account was not signed in when it was due".to_owned()
                };
                Release::Failed {
                    status: failed_status(&claimed.submission.sent.envelope, &reason, &[]),
                }
            } else {
                tracing::info!(
                    account = account.get(),
                    attempts = claimed.attempts,
                    failed,
                    "a held message will be tried again"
                );
                Release::Retry { at, failed }
            }
        }
    };
    let retry = matches!(release, Release::Retry { .. });
    match inner
        .store
        .release_held(
            account,
            due.id,
            release,
            now,
            inner.config.submission_lifetime,
        )
        .await
    {
        Ok(_) if retry => {}
        Ok(_) => inner
            .mail
            .announce(account, vec![DataType::EmailSubmission]),
        Err(error) => {
            tracing::error!(error = %error, "a held message could not be settled");
        }
    }
}

/// Opens and sends a claimed message, and appends its Sent copy.
async fn send_claimed(
    inner: &Inner,
    login: &SmtpLogin,
    mail: &MailAccount,
    claimed: &ClaimedHeld,
) -> Outcome {
    let account = mail.id();
    let envelope = &claimed.submission.sent.envelope;
    let fail = |reason: &str| {
        Outcome::Settled(Release::Failed {
            status: failed_status(envelope, reason, &[]),
        })
    };
    let binding = held_binding(claimed.submission.id);
    let opened = Binding::new(&binding).ok().and_then(|binding| {
        let sealed = SealedField::from_bytes(&claimed.sealed_message).ok()?;
        login
            .key
            .open_bound(Field::HeldMessage, &binding, &sealed)
            .ok()
    });
    let Some(raw) = opened else {
        tracing::error!(
            account = account.get(),
            "a held message could not be opened"
        );
        return fail("not sent: the bridge could not read the held message");
    };
    let message = {
        use secrecy::ExposeSecret as _;
        OutgoingMessage::new(raw.expose_secret())
    };
    drop(raw);
    let Ok(message) = message else {
        return fail("not sent: the held message is empty");
    };
    let (smtp, _) = match envelope_to_smtp(envelope) {
        Ok(smtp) => smtp,
        Err(error) => {
            return fail(&format!(
                "not sent: {}",
                error.description().unwrap_or("invalid envelope")
            ));
        }
    };
    let Ok(own) = own_envid(&smtp) else {
        return Outcome::Later {
            failed: true,
            why: "internal error".to_owned(),
        };
    };
    let message = Arc::new(message);
    let mut lease = mail.lease();
    let len = u64::try_from(message.as_bytes().len()).unwrap_or(u64::MAX);
    if let Err(error) = lease.reserve(len).await {
        return outcome_of(&SendError::Busy(error), envelope);
    }
    lease.drop_turn();
    let result = send::send(inner, login, &smtp, Arc::clone(&message), &mut lease, own).await;
    let sent = match result {
        Ok(sent) => sent,
        Err(error) => {
            tracing::warn!(
                account = account.get(),
                error = %error,
                "a held message was not sent"
            );
            return outcome_of(&error, envelope);
        }
    };
    tracing::info!(
        account = account.get(),
        recipients = smtp.to().len(),
        "held message sent"
    );
    if claimed.sent_copy
        && let Some(sent_box) = sent_mailbox_of(inner, account).await
    {
        append_sent(mail, sent_box, message.as_bytes()).await;
    }
    Outcome::Settled(Release::Sent {
        smtp_reply: format!("{} {}", sent.reply.code, sent.reply.text),
        envid: sent.envid.map(|id| id.as_str().to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_smtp::Stage;

    fn envelope() -> JmapEnvelope {
        serde_json::from_value(serde_json::json!({
            "mailFrom": {"email": "alice@example.com", "parameters": {"HOLDFOR": "10"}},
            "rcptTo": [{"email": "bob@example.com"}, {"email": "carol@example.com"}]
        }))
        .unwrap()
    }

    #[test]
    fn retries_back_off_up_to_half_an_hour() {
        assert_eq!(backoff(0), Duration::from_mins(1));
        assert_eq!(backoff(1), Duration::from_mins(2));
        assert_eq!(backoff(4), Duration::from_mins(16));
        assert_eq!(backoff(5), MAX_RETRY);
        assert_eq!(backoff(u32::MAX), MAX_RETRY);
    }

    #[test]
    fn what_failures_mean() {
        let envelope = envelope();
        let outcome = |error: SmtpError| outcome_of(&SendError::Smtp(error), &envelope);
        assert!(matches!(
            outcome(SmtpError::Network {
                stage: Stage::MailFrom
            }),
            Outcome::Later { failed: true, .. }
        ));
        // Written, then the connection broke: unknown, never again.
        assert_eq!(
            outcome(SmtpError::Network {
                stage: Stage::DataEnd
            }),
            Outcome::Settled(Release::Unknown)
        );
        // Refused for good: every recipient says why.
        let Outcome::Settled(Release::Failed { status }) =
            outcome(SmtpError::EightBitMimeUnsupported)
        else {
            panic!("not failed")
        };
        assert_eq!(status.len(), 2);
        assert!(status.values().all(|s| s.delivered == Delivered::No));
        assert!(
            status
                .values()
                .all(|s| s.smtp_reply == "not sent: the server can't take 8-bit messages"),
            "{status:?}"
        );
    }

    #[test]
    fn failed_status_names_the_refused() {
        let status = failed_status(
            &envelope(),
            "not sent: other recipients were refused",
            &[("BOB@example.com".to_owned(), "550 5.1.1 unknown".to_owned())],
        );
        assert_eq!(status["bob@example.com"].smtp_reply, "550 5.1.1 unknown");
        assert_eq!(
            status["carol@example.com"].smtp_reply,
            "not sent: other recipients were refused"
        );
    }

    #[test]
    fn until_is_never_negative() {
        assert_eq!(until(UnixTime::from_secs(0)), Duration::ZERO);
        let soon = UnixTime::now()
            .unwrap()
            .saturating_add(Duration::from_secs(30));
        assert!(until(soon) <= Duration::from_secs(30));
    }
}
