// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Held submissions (RFC 4865 FUTURERELEASE): undo send
//! (`HOLDFOR`) and scheduled send (`HOLDUNTIL`). The message waits here,
//! sealed by the caller with the account's data key, until the bridge's
//! scheduler sends it or the user cancels it.
//!
//! The life of one, all in `email_submissions.undo_status`:
//!
//! - `pending`, with a `held_submissions` row ([`Store::add_held_submission`]).
//! - `canceled` by the user while still `pending` ([`Store::cancel_held`]):
//!   the message is deleted.
//! - `sending` once the scheduler takes it ([`Store::claim_held`]); a cancel
//!   is then too late. The scheduler then settles it
//!   ([`Store::release_held`]): `final` (sent, outcome unknown or failed for
//!   good, the message deleted) or back to `pending` for a later attempt.
//! - A row left `sending` by a bridge that stopped is settled `final` with an
//!   unknown outcome at the next start ([`Store::settle_interrupted_held`]):
//!   it may have gone out, so it is never sent again.
//!
//! Held rows never expire (the purge skips them); `expires_at` counts from
//! when they are settled.
//!
//! A held message may be marked to go out even after the user signs out
//! (`send_when_signed_out`, [`Store::set_held_send_when_signed_out`]):
//! while one waits ([`Store::held_while_signed_out`]), the HTTP layer
//! keeps the account running and its password sealed.

use std::collections::BTreeMap;
use std::time::Duration;

use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, JoinOnDsl as _, OptionalExtension as _,
    QueryDsl as _, RunQueryDsl as _, SelectableHelper as _, SqliteConnection,
};

use super::delivery::RecipientStatus;
use super::submission::{HELD, NewSubmission, Submission, SubmissionRow, UndoState};
use crate::record::record_in;
use crate::schema::{email_submissions, held_submissions};
use crate::{
    AccountId, Change, ChangeKind, DataType, Seq, Store, StoreError, SubmissionId, UnixTime,
};

/// Per-account limits on what may be held at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldLimits {
    /// Most held (pending or sending) submissions.
    pub max_count: u64,
    /// Most bytes of held messages, counted in clear.
    pub max_bytes: u64,
}

/// What a held submission keeps besides its submission row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewHeld {
    /// Length of the message in clear.
    pub size: u64,
    /// Whether to append a copy to Sent once it is sent.
    pub sent_copy: bool,
    /// Whether it goes out even if the user signs out before it is due.
    pub send_when_signed_out: bool,
}

/// What [`Store::set_held_send_when_signed_out`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SetSignedOut {
    /// Changed (or it already had that value); the new `EmailSubmission`
    /// state, if it changed.
    Set(Option<Seq>),
    /// It is no longer pending (being sent, sent or canceled).
    NotPending,
}

/// A held submission whose time has come ([`Store::due_held`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueHeld {
    /// Its account.
    pub account: AccountId,
    /// The submission.
    pub id: SubmissionId,
    /// When it was to be sent.
    pub send_at: UnixTime,
    /// Failed attempts so far.
    pub attempts: u32,
}

/// A held submission taken by the scheduler ([`Store::claim_held`]): it is
/// `sending` until [`Store::release_held`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedHeld {
    /// The submission.
    pub submission: Submission,
    /// The message, sealed as it was given.
    pub sealed_message: Vec<u8>,
    /// Whether to append a copy to Sent once it is sent.
    pub sent_copy: bool,
    /// Failed attempts so far.
    pub attempts: u32,
}

/// How a claimed held submission ends ([`Store::release_held`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Release {
    /// The server took it.
    Sent {
        /// Its reply to the message.
        smtp_reply: String,
        /// The ENVID that went out with `MAIL FROM`.
        envid: Option<String>,
    },
    /// It may or may not have gone out (the connection broke after the
    /// message was written): `final` with an empty reply, never again.
    Unknown,
    /// It can't be sent (refused for good, or it waited too long):
    /// `final`, with every recipient's `delivered: no` and why.
    Failed {
        /// What happened, per envelope recipient.
        status: BTreeMap<String, RecipientStatus>,
    },
    /// Not now: back to `pending`, to try again at `at`.
    Retry {
        /// When to look at it again.
        at: UnixTime,
        /// Whether this counts as a failed attempt (not when the account
        /// was just not signed in).
        failed: bool,
    },
}

/// What [`Store::cancel_held`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cancel {
    /// Canceled now; the new `EmailSubmission` state.
    Canceled(Seq),
    /// It was already canceled.
    AlreadyCanceled,
    /// It is being sent or was sent (or was never held).
    TooLate,
}

fn submission_id(raw: i64) -> Result<SubmissionId, StoreError> {
    SubmissionId::from_raw(raw).ok_or(StoreError::Corrupt("submission id"))
}

fn account_id(raw: i64) -> Result<AccountId, StoreError> {
    AccountId::from_raw(raw).ok_or(StoreError::Corrupt("account id"))
}

/// The `send_at` a held submission must be after to still be sent at
/// `now`: the scheduler gives up `max_wait` after it.
fn oldest_send_at(now: UnixTime, max_wait: Duration) -> i64 {
    let wait = i64::try_from(max_wait.as_secs()).unwrap_or(i64::MAX);
    now.secs().saturating_sub(wait)
}

fn attempts(raw: i64) -> u32 {
    u32::try_from(raw).unwrap_or(u32::MAX)
}

fn record_updated(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: SubmissionId,
) -> Result<Seq, StoreError> {
    record_in(
        conn,
        account,
        DataType::EmailSubmission,
        &[Change {
            object: id.get(),
            kind: ChangeKind::Updated,
        }],
    )
}

/// The submission row of the account in `state`, or `None`.
fn row_in(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: SubmissionId,
) -> Result<Option<SubmissionRow>, StoreError> {
    Ok(email_submissions::table
        .filter(email_submissions::id.eq(id.get()))
        .filter(email_submissions::account_id.eq(account.get()))
        .select(SubmissionRow::as_select())
        .first(conn)
        .optional()?)
}

impl Store {
    /// Stores a held submission: its row (`pending`, `send_at` = when to
    /// send it) and its message, which `seal` gives sealed for the new id
    /// (it runs inside the transaction, so a message is never stored for
    /// another row). Returns its id and the new `EmailSubmission` state.
    ///
    /// # Errors
    /// [`StoreError::OverQuota`] past `limits`; what `seal` returns; and
    /// database errors.
    pub async fn add_held_submission<F>(
        &self,
        account: AccountId,
        sent: NewSubmission,
        held: NewHeld,
        limits: HeldLimits,
        seal: F,
    ) -> Result<(SubmissionId, Seq), StoreError>
    where
        F: FnOnce(SubmissionId) -> Result<Vec<u8>, StoreError> + Send + 'static,
    {
        self.write(move |conn| {
            let sizes: Vec<i64> = held_submissions::table
                .filter(held_submissions::account_id.eq(account.get()))
                .select(held_submissions::size)
                .load(conn)?;
            let count = u64::try_from(sizes.len()).unwrap_or(u64::MAX);
            if count >= limits.max_count {
                return Err(StoreError::OverQuota("held messages"));
            }
            let bytes = sizes
                .iter()
                .map(|size| u64::try_from(*size).unwrap_or(u64::MAX))
                .fold(held.size, u64::saturating_add);
            if bytes > limits.max_bytes {
                return Err(StoreError::OverQuota("bytes of held messages"));
            }
            let size = i64::try_from(held.size).map_err(|_| StoreError::Invalid("message size"))?;
            let envelope = serde_json::to_string(&sent.envelope)
                .map_err(|_| StoreError::Invalid("envelope"))?;
            let raw_id: i64 = diesel::insert_into(email_submissions::table)
                .values((
                    email_submissions::account_id.eq(account.get()),
                    email_submissions::identity_id.eq(sent.identity.get()),
                    email_submissions::email_id.eq(sent.email.get()),
                    email_submissions::thread_id.eq(sent.thread.get()),
                    email_submissions::envelope.eq(envelope),
                    email_submissions::send_at.eq(sent.send_at.secs()),
                    email_submissions::smtp_reply.eq(&sent.smtp_reply),
                    email_submissions::expires_at.eq(sent.expires_at.secs()),
                    email_submissions::message_id.eq(&sent.message_id),
                    email_submissions::envid.eq(&sent.envid),
                    email_submissions::undo_status.eq(UndoState::Pending.as_str()),
                ))
                .returning(email_submissions::id)
                .get_result(conn)?;
            let id = submission_id(raw_id)?;
            let message = seal(id)?;
            diesel::insert_into(held_submissions::table)
                .values((
                    held_submissions::submission_id.eq(raw_id),
                    held_submissions::account_id.eq(account.get()),
                    held_submissions::message.eq(message),
                    held_submissions::size.eq(size),
                    held_submissions::next_attempt_at.eq(sent.send_at.secs()),
                    held_submissions::sent_copy.eq(held.sent_copy),
                    held_submissions::send_when_signed_out.eq(held.send_when_signed_out),
                ))
                .execute(conn)?;
            let seq = record_in(
                conn,
                account,
                DataType::EmailSubmission,
                &[Change {
                    object: raw_id,
                    kind: ChangeKind::Created,
                }],
            )?;
            Ok((id, seq))
        })
        .await
    }

    /// Whether a held submission appends a copy to Sent when it is sent
    /// (the client's `onSuccessUpdateEmail` may have filed the email in
    /// Sent after it was stored). Nothing happens if it is no longer held.
    ///
    /// # Errors
    /// On database errors.
    pub async fn set_held_sent_copy(
        &self,
        account: AccountId,
        id: SubmissionId,
        sent_copy: bool,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            diesel::update(
                held_submissions::table
                    .filter(held_submissions::submission_id.eq(id.get()))
                    .filter(held_submissions::account_id.eq(account.get())),
            )
            .set(held_submissions::sent_copy.eq(sent_copy))
            .execute(conn)?;
            Ok(())
        })
        .await
    }

    /// Whether a pending submission goes out even if the user signs out
    /// before it is due. Only while it is `pending`.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's.
    pub async fn set_held_send_when_signed_out(
        &self,
        account: AccountId,
        id: SubmissionId,
        value: bool,
    ) -> Result<SetSignedOut, StoreError> {
        self.write(move |conn| {
            let row = row_in(conn, account, id)?.ok_or(StoreError::NotFound("submission"))?;
            if row.undo_status != UndoState::Pending.as_str() {
                return Ok(SetSignedOut::NotPending);
            }
            let changed = diesel::update(
                held_submissions::table
                    .filter(held_submissions::submission_id.eq(id.get()))
                    .filter(held_submissions::account_id.eq(account.get()))
                    .filter(held_submissions::send_when_signed_out.ne(value)),
            )
            .set(held_submissions::send_when_signed_out.eq(value))
            .execute(conn)?;
            if changed == 0 {
                return Ok(SetSignedOut::Set(None));
            }
            Ok(SetSignedOut::Set(Some(record_updated(conn, account, id)?)))
        })
        .await
    }

    /// How many of the account's held submissions marked
    /// `send_when_signed_out` still need its password: pending or being
    /// sent, and not past their last chance (`send_at + max_wait`, when
    /// the scheduler gives up on them).
    ///
    /// # Errors
    /// On database errors.
    pub async fn held_while_signed_out(
        &self,
        account: AccountId,
        now: UnixTime,
        max_wait: Duration,
    ) -> Result<u64, StoreError> {
        self.read(move |conn| {
            let count: i64 = held_submissions::table
                .inner_join(
                    email_submissions::table
                        .on(email_submissions::id.eq(held_submissions::submission_id)),
                )
                .filter(held_submissions::account_id.eq(account.get()))
                .filter(held_submissions::send_when_signed_out.eq(true))
                .filter(email_submissions::undo_status.eq_any(HELD))
                .filter(email_submissions::send_at.gt(oldest_send_at(now, max_wait)))
                .count()
                .get_result(conn)?;
            Ok(u64::try_from(count).unwrap_or(0))
        })
        .await
    }

    /// The accounts with held submissions that need their password after
    /// sign-out (see [`Store::held_while_signed_out`]): started at boot.
    ///
    /// # Errors
    /// On database errors.
    pub async fn accounts_held_while_signed_out(
        &self,
        now: UnixTime,
        max_wait: Duration,
    ) -> Result<Vec<AccountId>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<i64> = held_submissions::table
                .inner_join(
                    email_submissions::table
                        .on(email_submissions::id.eq(held_submissions::submission_id)),
                )
                .filter(held_submissions::send_when_signed_out.eq(true))
                .filter(email_submissions::undo_status.eq_any(HELD))
                .filter(email_submissions::send_at.gt(oldest_send_at(now, max_wait)))
                .select(held_submissions::account_id)
                .distinct()
                .order(held_submissions::account_id.asc())
                .load(conn)?;
            rows.into_iter().map(account_id).collect()
        })
        .await
    }

    /// Cancels a pending submission (`undoStatus: canceled`) and deletes its
    /// message. It stays visible for `lifetime`.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's.
    pub async fn cancel_held(
        &self,
        account: AccountId,
        id: SubmissionId,
        now: UnixTime,
        lifetime: Duration,
    ) -> Result<Cancel, StoreError> {
        self.write(move |conn| {
            let row = row_in(conn, account, id)?.ok_or(StoreError::NotFound("submission"))?;
            match row.undo_status.as_str() {
                s if s == UndoState::Pending.as_str() => {}
                s if s == UndoState::Canceled.as_str() => return Ok(Cancel::AlreadyCanceled),
                _ => return Ok(Cancel::TooLate),
            }
            diesel::update(
                email_submissions::table
                    .filter(email_submissions::id.eq(id.get()))
                    .filter(email_submissions::account_id.eq(account.get()))
                    .filter(email_submissions::undo_status.eq(UndoState::Pending.as_str())),
            )
            .set((
                email_submissions::undo_status.eq(UndoState::Canceled.as_str()),
                email_submissions::expires_at.eq(now.saturating_add(lifetime).secs()),
            ))
            .execute(conn)?;
            diesel::delete(
                held_submissions::table.filter(held_submissions::submission_id.eq(id.get())),
            )
            .execute(conn)?;
            Ok(Cancel::Canceled(record_updated(conn, account, id)?))
        })
        .await
    }

    /// When the next pending submission is due, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn next_held_attempt(&self) -> Result<Option<UnixTime>, StoreError> {
        self.read(move |conn| {
            let next: Option<i64> = held_submissions::table
                .inner_join(
                    email_submissions::table
                        .on(email_submissions::id.eq(held_submissions::submission_id)),
                )
                .filter(email_submissions::undo_status.eq(UndoState::Pending.as_str()))
                .select(diesel::dsl::min(held_submissions::next_attempt_at))
                .first(conn)?;
            Ok(next.map(UnixTime::from_secs))
        })
        .await
    }

    /// Up to `limit` pending submissions due at `now`, the most overdue
    /// first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn due_held(&self, now: UnixTime, limit: i64) -> Result<Vec<DueHeld>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<(i64, i64, i64, i64)> = held_submissions::table
                .inner_join(
                    email_submissions::table
                        .on(email_submissions::id.eq(held_submissions::submission_id)),
                )
                .filter(email_submissions::undo_status.eq(UndoState::Pending.as_str()))
                .filter(held_submissions::next_attempt_at.le(now.secs()))
                .order((
                    held_submissions::next_attempt_at.asc(),
                    held_submissions::submission_id.asc(),
                ))
                .limit(limit)
                .select((
                    held_submissions::account_id,
                    held_submissions::submission_id,
                    email_submissions::send_at,
                    held_submissions::attempts,
                ))
                .load(conn)?;
            rows.into_iter()
                .map(|(account, id, send_at, tries)| {
                    Ok(DueHeld {
                        account: account_id(account)?,
                        id: submission_id(id)?,
                        send_at: UnixTime::from_secs(send_at),
                        attempts: attempts(tries),
                    })
                })
                .collect()
        })
        .await
    }

    /// Takes a pending submission to send it (`sending`: a cancel is now too
    /// late). `None` if it is no longer pending (canceled meanwhile).
    ///
    /// # Errors
    /// On database errors.
    pub async fn claim_held(
        &self,
        account: AccountId,
        id: SubmissionId,
    ) -> Result<Option<ClaimedHeld>, StoreError> {
        self.write(move |conn| {
            let claimed = diesel::update(
                email_submissions::table
                    .filter(email_submissions::id.eq(id.get()))
                    .filter(email_submissions::account_id.eq(account.get()))
                    .filter(email_submissions::undo_status.eq(UndoState::Pending.as_str())),
            )
            .set(email_submissions::undo_status.eq(UndoState::Sending.as_str()))
            .execute(conn)?;
            if claimed == 0 {
                return Ok(None);
            }
            let Some(row) = row_in(conn, account, id)? else {
                return Ok(None);
            };
            let (sealed_message, sent_copy, tries): (Vec<u8>, bool, i64) = held_submissions::table
                .filter(held_submissions::submission_id.eq(id.get()))
                .select((
                    held_submissions::message,
                    held_submissions::sent_copy,
                    held_submissions::attempts,
                ))
                .first(conn)?;
            Ok(Some(ClaimedHeld {
                submission: Submission::try_from(row)?,
                sealed_message,
                sent_copy,
                attempts: attempts(tries),
            }))
        })
        .await
    }

    /// Settles a claimed submission (see [`Release`]). Returns the new
    /// `EmailSubmission` state when it became `final` (a retry changes
    /// nothing a client sees).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's or isn't being
    /// sent.
    pub async fn release_held(
        &self,
        account: AccountId,
        id: SubmissionId,
        release: Release,
        now: UnixTime,
        lifetime: Duration,
    ) -> Result<Option<Seq>, StoreError> {
        self.write(move |conn| {
            let target = email_submissions::table
                .filter(email_submissions::id.eq(id.get()))
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(email_submissions::undo_status.eq(UndoState::Sending.as_str()));
            let final_state = (
                email_submissions::undo_status.eq(UndoState::Final.as_str()),
                email_submissions::expires_at.eq(now.saturating_add(lifetime).secs()),
            );
            let updated = match release {
                Release::Retry { at, failed } => {
                    let updated = diesel::update(target)
                        .set(email_submissions::undo_status.eq(UndoState::Pending.as_str()))
                        .execute(conn)?;
                    diesel::update(
                        held_submissions::table
                            .filter(held_submissions::submission_id.eq(id.get())),
                    )
                    .set((
                        held_submissions::next_attempt_at.eq(at.secs()),
                        held_submissions::attempts
                            .eq(held_submissions::attempts + i64::from(failed)),
                    ))
                    .execute(conn)?;
                    if updated == 0 {
                        return Err(StoreError::NotFound("submission"));
                    }
                    return Ok(None);
                }
                // Sent now: `sendAt` is when it really went out.
                Release::Sent { smtp_reply, envid } => diesel::update(target)
                    .set((
                        final_state,
                        email_submissions::smtp_reply.eq(smtp_reply),
                        email_submissions::envid.eq(envid),
                        email_submissions::send_at.eq(now.secs()),
                    ))
                    .execute(conn)?,
                Release::Unknown => diesel::update(target)
                    .set((final_state, email_submissions::smtp_reply.eq("")))
                    .execute(conn)?,
                Release::Failed { status } => {
                    let reply = status
                        .values()
                        .next()
                        .map(|s| s.smtp_reply.clone())
                        .unwrap_or_default();
                    let status = serde_json::to_string(&status)
                        .map_err(|_| StoreError::Invalid("delivery status"))?;
                    diesel::update(target)
                        .set((
                            final_state,
                            email_submissions::smtp_reply.eq(reply),
                            email_submissions::delivery.eq(Some(status)),
                        ))
                        .execute(conn)?
                }
            };
            if updated == 0 {
                return Err(StoreError::NotFound("submission"));
            }
            diesel::delete(
                held_submissions::table.filter(held_submissions::submission_id.eq(id.get())),
            )
            .execute(conn)?;
            Ok(Some(record_updated(conn, account, id)?))
        })
        .await
    }

    /// At start: the submissions a stopped bridge left `sending` become
    /// `final` with an unknown outcome (they may have gone out), and their
    /// messages are deleted. Returns how many.
    ///
    /// # Errors
    /// On database errors.
    pub async fn settle_interrupted_held(
        &self,
        now: UnixTime,
        lifetime: Duration,
    ) -> Result<usize, StoreError> {
        self.write(move |conn| {
            let rows: Vec<(i64, i64)> = email_submissions::table
                .filter(email_submissions::undo_status.eq(UndoState::Sending.as_str()))
                .select((email_submissions::account_id, email_submissions::id))
                .load(conn)?;
            let mut by_account: BTreeMap<i64, Vec<Change>> = BTreeMap::new();
            for (account, id) in &rows {
                by_account.entry(*account).or_default().push(Change {
                    object: *id,
                    kind: ChangeKind::Updated,
                });
            }
            let ids: Vec<i64> = rows.iter().map(|(_, id)| *id).collect();
            diesel::update(email_submissions::table.filter(email_submissions::id.eq_any(&ids)))
                .set((
                    email_submissions::undo_status.eq(UndoState::Final.as_str()),
                    email_submissions::smtp_reply.eq(""),
                    email_submissions::expires_at.eq(now.saturating_add(lifetime).secs()),
                ))
                .execute(conn)?;
            diesel::delete(
                held_submissions::table.filter(held_submissions::submission_id.eq_any(&ids)),
            )
            .execute(conn)?;
            for (account, changes) in by_account {
                record_in(
                    conn,
                    account_id(account)?,
                    DataType::EmailSubmission,
                    &changes,
                )?;
            }
            Ok(ids.len())
        })
        .await
    }

    /// The account just started: its overdue pending submissions (that
    /// waited for it) are due now.
    ///
    /// # Errors
    /// On database errors.
    pub async fn wake_held(&self, account: AccountId, now: UnixTime) -> Result<(), StoreError> {
        self.write(move |conn| {
            let overdue = email_submissions::table
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(email_submissions::undo_status.eq(UndoState::Pending.as_str()))
                .filter(email_submissions::send_at.le(now.secs()))
                .select(email_submissions::id);
            diesel::update(
                held_submissions::table.filter(
                    held_submissions::account_id
                        .eq(account.get())
                        .and(held_submissions::next_attempt_at.gt(now.secs()))
                        .and(held_submissions::submission_id.eq_any(overdue)),
                ),
            )
            .set(held_submissions::next_attempt_at.eq(now.secs()))
            .execute(conn)?;
            Ok(())
        })
        .await
    }
}
