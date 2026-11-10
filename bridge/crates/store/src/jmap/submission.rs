// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Submissions (RFC 8621 §7). Most are sent at once and stored `final`;
//! held ones (RFC 4865 FUTURERELEASE) are `pending` until the scheduler
//! sends them ([`held`](super::held)). They are kept for
//! `EmailSubmission/get` until they expire.

use std::collections::BTreeMap;

use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, QueryDsl as _, Queryable, RunQueryDsl as _,
    Selectable, SelectableHelper as _, SqliteConnection,
};
use nebula_jmap_proto::object::Envelope;

use super::delivery::{self, CLOCK_SLACK_SECS, DeliveryReport, RecipientStatus, ReportKind};
use crate::mail::convert::{email_id, thread_id};
use crate::record::record_in;
use crate::schema::{email_submissions, held_submissions};
use crate::{
    AccountId, Change, ChangeKind, DataType, EmailId, IdentityId, Recorded, Seq, Store, StoreError,
    SubmissionId, ThreadId, UnixTime,
};

/// A submission to store after sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubmission {
    /// The identity it was sent as.
    pub identity: IdentityId,
    /// The email sent.
    pub email: EmailId,
    /// Its thread.
    pub thread: ThreadId,
    /// The SMTP envelope used.
    pub envelope: Envelope,
    /// When it was sent.
    pub send_at: UnixTime,
    /// The server's reply to the message.
    pub smtp_reply: String,
    /// When to forget it.
    pub expires_at: UnixTime,
    /// `Message-ID` of the email sent (without `<>`).
    pub message_id: Option<String>,
    /// The ENVID sent with `MAIL FROM`.
    pub envid: Option<String>,
}

/// Where a submission stands (`undoStatus`, RFC 8621 §7). The text of
/// [`UndoState::as_str`] is what the database stores: never change one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UndoState {
    /// Held: not sent yet, and it can still be canceled.
    Pending,
    /// Being handed to the SMTP server: `pending` for JMAP, but too late
    /// to cancel. A row the bridge stopped in is settled as `final` with
    /// an unknown outcome, never sent again.
    Sending,
    /// Sent (or its outcome is unknown, or it failed for good).
    Final,
    /// Canceled before it was sent.
    Canceled,
}

impl UndoState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Final => "final",
            Self::Canceled => "canceled",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "pending" => Ok(Self::Pending),
            "sending" => Ok(Self::Sending),
            "final" => Ok(Self::Final),
            "canceled" => Ok(Self::Canceled),
            _ => Err(StoreError::Corrupt("undo status")),
        }
    }
}

/// A stored submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    /// Its id.
    pub id: SubmissionId,
    /// Whether it went out.
    pub undo: UndoState,
    /// The rest.
    pub sent: NewSubmission,
    /// Status per recipient (`deliveryStatus`), keyed by the envelope's
    /// addresses.
    pub status: BTreeMap<String, RecipientStatus>,
    /// The emails with delivery reports about it (`dsnBlobIds`).
    pub dsn_emails: Vec<EmailId>,
    /// The emails with read receipts about it (`mdnBlobIds`).
    pub mdn_emails: Vec<EmailId>,
    /// Held, and it goes out even if the user signs out before it is due
    /// ([`held`](super::held)). Always `false` once it is no longer held.
    pub send_when_signed_out: bool,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = email_submissions, check_for_backend(diesel::sqlite::Sqlite))]
pub(super) struct SubmissionRow {
    id: i64,
    identity_id: i64,
    email_id: i64,
    thread_id: i64,
    envelope: String,
    send_at: i64,
    smtp_reply: String,
    expires_at: i64,
    message_id: Option<String>,
    envid: Option<String>,
    delivery: Option<String>,
    dsn_emails: String,
    mdn_emails: String,
    pub(super) undo_status: String,
}

fn email_list(text: &str) -> Result<Vec<EmailId>, StoreError> {
    let raw: Vec<i64> =
        serde_json::from_str(text).map_err(|_| StoreError::Corrupt("report list"))?;
    raw.into_iter().map(email_id).collect()
}

fn email_list_text(list: &[EmailId]) -> Result<String, StoreError> {
    let raw: Vec<i64> = list.iter().map(|id| id.get()).collect();
    serde_json::to_string(&raw).map_err(|_| StoreError::Invalid("report list"))
}

impl TryFrom<SubmissionRow> for Submission {
    type Error = StoreError;

    fn try_from(row: SubmissionRow) -> Result<Self, StoreError> {
        let envelope: Envelope =
            serde_json::from_str(&row.envelope).map_err(|_| StoreError::Corrupt("envelope"))?;
        let status = match &row.delivery {
            Some(text) => {
                serde_json::from_str(text).map_err(|_| StoreError::Corrupt("delivery status"))?
            }
            None => delivery::initial(&envelope, &row.smtp_reply),
        };
        Ok(Self {
            id: SubmissionId::from_raw(row.id).ok_or(StoreError::Corrupt("submission id"))?,
            undo: UndoState::parse(&row.undo_status)?,
            status,
            dsn_emails: email_list(&row.dsn_emails)?,
            mdn_emails: email_list(&row.mdn_emails)?,
            send_when_signed_out: false,
            sent: NewSubmission {
                identity: IdentityId::from_raw(row.identity_id)
                    .ok_or(StoreError::Corrupt("identity id"))?,
                email: email_id(row.email_id)?,
                thread: thread_id(row.thread_id)?,
                envelope,
                send_at: UnixTime::from_secs(row.send_at),
                smtp_reply: row.smtp_reply,
                expires_at: UnixTime::from_secs(row.expires_at),
                message_id: row.message_id,
                envid: row.envid,
            },
        })
    }
}

/// The states of a held submission (with a [`held`](super::held) row):
/// those never expire, they are settled first.
pub(super) const HELD: [&str; 2] = [UndoState::Pending.as_str(), UndoState::Sending.as_str()];

pub(super) fn purge(conn: &mut SqliteConnection, now: UnixTime) -> Result<(), StoreError> {
    let expired: Vec<(i64, i64)> = email_submissions::table
        .filter(email_submissions::expires_at.le(now.secs()))
        .filter(email_submissions::undo_status.ne_all(HELD))
        .select((email_submissions::account_id, email_submissions::id))
        .load(conn)?;
    let mut by_account: BTreeMap<i64, Vec<Change>> = BTreeMap::new();
    for (account, id) in expired {
        by_account.entry(account).or_default().push(Change {
            object: id,
            kind: ChangeKind::Destroyed,
        });
    }
    diesel::delete(
        email_submissions::table
            .filter(email_submissions::expires_at.le(now.secs()))
            .filter(email_submissions::undo_status.ne_all(HELD)),
    )
    .execute(conn)?;
    for (account, changes) in by_account {
        let account = AccountId::from_raw(account).ok_or(StoreError::Corrupt("account id"))?;
        record_in(conn, account, DataType::EmailSubmission, &changes)?;
    }
    Ok(())
}

impl Store {
    /// Stores a sent submission and returns its id and the new
    /// `EmailSubmission` state.
    ///
    /// # Errors
    /// On database errors.
    pub async fn add_submission(
        &self,
        account: AccountId,
        sent: NewSubmission,
    ) -> Result<(SubmissionId, Seq), StoreError> {
        self.write(move |conn| {
            let envelope = serde_json::to_string(&sent.envelope)
                .map_err(|_| StoreError::Invalid("envelope"))?;
            let id: i64 = diesel::insert_into(email_submissions::table)
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
                ))
                .returning(email_submissions::id)
                .get_result(conn)?;
            let seq = record_in(
                conn,
                account,
                DataType::EmailSubmission,
                &[Change {
                    object: id,
                    kind: ChangeKind::Created,
                }],
            )?;
            let id = SubmissionId::from_raw(id).ok_or(StoreError::Corrupt("submission id"))?;
            Ok((id, seq))
        })
        .await
    }

    /// The account's unexpired submissions: these ids (unknown ones are left
    /// out), or all of them, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn submissions(
        &self,
        account: AccountId,
        ids: Option<Vec<SubmissionId>>,
        now: UnixTime,
    ) -> Result<Vec<Submission>, StoreError> {
        self.read(move |conn| {
            let mut query = email_submissions::table
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(
                    email_submissions::expires_at
                        .gt(now.secs())
                        .or(email_submissions::undo_status.eq_any(HELD)),
                )
                .order(email_submissions::id.asc())
                .select(SubmissionRow::as_select())
                .into_boxed();
            if let Some(ids) = ids {
                let wanted: Vec<i64> = ids.iter().map(|id| id.get()).collect();
                query = query.filter(email_submissions::id.eq_any(wanted));
            }
            let mut list = query
                .load(conn)?
                .into_iter()
                .map(Submission::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            // Which held ones go out after sign-out.
            let held: Vec<i64> = list
                .iter()
                .filter(|s| matches!(s.undo, UndoState::Pending | UndoState::Sending))
                .map(|s| s.id.get())
                .collect();
            if !held.is_empty() {
                let flagged: Vec<i64> = held_submissions::table
                    .filter(held_submissions::account_id.eq(account.get()))
                    .filter(held_submissions::submission_id.eq_any(held))
                    .filter(held_submissions::send_when_signed_out.eq(true))
                    .select(held_submissions::submission_id)
                    .load(conn)?;
                for submission in &mut list {
                    submission.send_when_signed_out = flagged.contains(&submission.id.get());
                }
            }
            Ok(list)
        })
        .await
    }

    /// The account's submissions that expired but aren't purged yet (their
    /// `destroyed` change is only recorded by
    /// [`purge_expired`](Store::purge_expired)): `EmailSubmission/get` no
    /// longer shows them, so `EmailSubmission/queryChanges` gives them as
    /// removed.
    ///
    /// # Errors
    /// On database errors.
    pub async fn expired_submissions(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<Vec<SubmissionId>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<i64> = email_submissions::table
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(email_submissions::expires_at.le(now.secs()))
                .filter(email_submissions::undo_status.ne_all(HELD))
                .order(email_submissions::id.asc())
                .select(email_submissions::id)
                .load(conn)?;
            rows.into_iter()
                .map(|id| SubmissionId::from_raw(id).ok_or(StoreError::Corrupt("submission id")))
                .collect()
        })
        .await
    }

    /// Whether the account has unexpired submissions sent before `at`
    /// (plus a little clock slack): only then can an arriving report be
    /// about one of them.
    ///
    /// # Errors
    /// On database errors.
    pub async fn expects_reports(
        &self,
        account: AccountId,
        at: UnixTime,
        now: UnixTime,
    ) -> Result<bool, StoreError> {
        self.read(move |conn| {
            let count: i64 = email_submissions::table
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(email_submissions::expires_at.gt(now.secs()))
                .filter(email_submissions::undo_status.eq(UndoState::Final.as_str()))
                .filter(email_submissions::send_at.le(at.secs().saturating_add(CLOCK_SLACK_SECS)))
                .count()
                .get_result(conn)?;
            Ok(count > 0)
        })
        .await
    }

    /// Applies a delivery report or read receipt found in the account's
    /// mail to the account's matching submissions (see
    /// [`delivery`](super::delivery) for the rules). Returns the new
    /// `EmailSubmission` state if anything changed.
    ///
    /// # Errors
    /// On database errors.
    pub async fn apply_report(
        &self,
        account: AccountId,
        report: DeliveryReport,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            // Only what went out: a report can't touch a held message.
            let base = email_submissions::table
                .filter(email_submissions::account_id.eq(account.get()))
                .filter(email_submissions::expires_at.gt(now.secs()))
                .filter(email_submissions::undo_status.eq(UndoState::Final.as_str()))
                .filter(
                    email_submissions::send_at
                        .le(report.received_at.secs().saturating_add(CLOCK_SLACK_SECS)),
                )
                .select(SubmissionRow::as_select())
                .into_boxed();
            // With an ENVID, only the submissions sent with it. A DSN
            // without one only matches by Message-ID the submissions that
            // had no ENVID (one sent with an ENVID gets its real DSNs with
            // it, so a forged DSN without it can't touch it). A read
            // receipt (MDN) never carries an ENVID (RFC 8098), so it always
            // matches by Message-ID.
            let rows: Vec<SubmissionRow> = match (&report.envid, &report.original_message_id) {
                (Some(envid), _) => base.filter(email_submissions::envid.eq(envid)).load(conn)?,
                (None, Some(message_id)) => {
                    let query = base.filter(email_submissions::message_id.eq(message_id));
                    match report.kind {
                        ReportKind::Delivery(_) => query
                            .filter(email_submissions::envid.is_null())
                            .load(conn)?,
                        ReportKind::Displayed(_) => query.load(conn)?,
                    }
                }
                (None, None) => Vec::new(),
            };
            let mut changes = Vec::new();
            for row in rows {
                let mut submission = Submission::try_from(row)?;
                // An ENVID match must also be about the same message, if
                // the report says which.
                if report.envid.is_some()
                    && let (Some(theirs), Some(ours)) =
                        (&report.original_message_id, &submission.sent.message_id)
                    && theirs != ours
                {
                    continue;
                }
                let mut updated = delivery::apply(
                    &mut submission.status,
                    &submission.sent.envelope,
                    &report.kind,
                );
                let list = match report.kind {
                    ReportKind::Delivery(_) => &mut submission.dsn_emails,
                    ReportKind::Displayed(_) => &mut submission.mdn_emails,
                };
                if !list.contains(&report.email) && list.len() < 64 {
                    list.push(report.email);
                    updated = true;
                }
                if !updated {
                    continue;
                }
                let status = serde_json::to_string(&submission.status)
                    .map_err(|_| StoreError::Invalid("delivery status"))?;
                diesel::update(
                    email_submissions::table
                        .filter(email_submissions::id.eq(submission.id.get()))
                        .filter(email_submissions::account_id.eq(account.get())),
                )
                .set((
                    email_submissions::delivery.eq(Some(status)),
                    email_submissions::dsn_emails.eq(email_list_text(&submission.dsn_emails)?),
                    email_submissions::mdn_emails.eq(email_list_text(&submission.mdn_emails)?),
                ))
                .execute(conn)?;
                changes.push(Change {
                    object: submission.id.get(),
                    kind: ChangeKind::Updated,
                });
            }
            let mut recorded = Recorded::default();
            if !changes.is_empty() {
                let seq = record_in(conn, account, DataType::EmailSubmission, &changes)?;
                recorded.push(DataType::EmailSubmission, seq);
            }
            Ok(recorded)
        })
        .await
    }

    /// Fills in what the server replied to a submission stored before
    /// sending (with an empty `smtp_reply`), and the ENVID that went out.
    /// Returns the new `EmailSubmission` state.
    ///
    /// The bridge stores the row first, so that a send can't happen
    /// without its record; a row whose `smtp_reply` stays empty (the
    /// bridge stopped, or the outcome is unknown) means the outcome is
    /// unknown, never that it was delivered.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's.
    pub async fn finish_submission(
        &self,
        account: AccountId,
        id: SubmissionId,
        smtp_reply: String,
        envid: Option<String>,
    ) -> Result<Seq, StoreError> {
        self.write(move |conn| {
            let updated = diesel::update(
                email_submissions::table
                    .filter(email_submissions::id.eq(id.get()))
                    .filter(email_submissions::account_id.eq(account.get())),
            )
            .set((
                email_submissions::smtp_reply.eq(&smtp_reply),
                email_submissions::envid.eq(&envid),
            ))
            .execute(conn)?;
            if updated == 0 {
                return Err(StoreError::NotFound("submission"));
            }
            record_in(
                conn,
                account,
                DataType::EmailSubmission,
                &[Change {
                    object: id.get(),
                    kind: ChangeKind::Updated,
                }],
            )
        })
        .await
    }

    /// Forgets a submission and returns the new `EmailSubmission` state.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's.
    pub async fn destroy_submission(
        &self,
        account: AccountId,
        id: SubmissionId,
    ) -> Result<Seq, StoreError> {
        self.write(move |conn| {
            let deleted = diesel::delete(
                email_submissions::table
                    .filter(email_submissions::id.eq(id.get()))
                    .filter(email_submissions::account_id.eq(account.get())),
            )
            .execute(conn)?;
            if deleted == 0 {
                return Err(StoreError::NotFound("submission"));
            }
            record_in(
                conn,
                account,
                DataType::EmailSubmission,
                &[Change {
                    object: id.get(),
                    kind: ChangeKind::Destroyed,
                }],
            )
        })
        .await
    }
}
