// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `EmailSubmission/query` (RFC 8621 §7.3) and
//! `EmailSubmission/queryChanges` (§7.4), in memory: the bridge only keeps
//! the submissions of the last day (`submission_lifetime`), the same ones
//! `EmailSubmission/get` shows.
//!
//! Every condition of the RFC is supported (`identityIds`, `emailIds`,
//! `threadIds`, `undoStatus`, `before`, `after`) and every sort (`emailId`,
//! `threadId`, `sentAt`); anything else is `unsupportedFilter` or
//! `unsupportedSort` when parsing, never a partial result. Ids are ordered
//! by their row number (the order they were created in); the default
//! order, and the tie-break, is the submission's (oldest first). No sort
//! property is text, so a `collation` the session offers has no effect
//! (as on `Email/query` dates and sizes) and any other is
//! `unsupportedSort` before any work (RFC 8620 §5.5).
//!
//! `EmailSubmission/queryChanges` uses the `EmailSubmission` change log
//! like `Email/queryChanges`: every submission created, updated or
//! destroyed since `sinceQueryState` goes to `removed` (except new ones)
//! and, if it is in the current results, to `added` with its index (the
//! RFC allows such extra ids). Only those can move: a held submission
//! that goes out or is canceled (`undoStatus`, and `sendAt` when it is
//! sent later than planned) is recorded as updated. A submission
//! that expired but isn't purged yet has no `destroyed` change: it goes to
//! `removed` as well. `upToId` is ignored (allowed).

use std::cmp::Ordering;
use std::collections::BTreeSet;

use nebula_jmap_proto::method::{AddedItem, Filter, Operator, QueryChangesResponse, QueryResponse};
use nebula_jmap_proto::object::{
    EmailSubmissionFilterCondition, EmailSubmissionQueryArguments,
    EmailSubmissionQueryChangesArguments, EmailSubmissionSort,
};
use nebula_jmap_proto::{MethodErrorType, MethodResponse, UnsignedInt};
use nebula_store::{ChangesSince, DataType, Submission, SubmissionId};

use crate::collation::CheckedSort;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::CallError;

/// Most ids in one page, whatever the client asks for (as `Email/query`).
const MAX_LIMIT: u64 = 1_000;

/// Most changes `EmailSubmission/queryChanges` works through (as
/// `Email/queryChanges`).
const MAX_QUERY_CHANGES: usize = 10_000;

fn condition_matches(submission: &Submission, condition: &EmailSubmissionFilterCondition) -> bool {
    let sent = &submission.sent;
    // Ids that aren't ours match nothing.
    if let Some(list) = &condition.identity_ids
        && !list
            .iter()
            .any(|id| ids::store_identity(id) == Some(sent.identity))
    {
        return false;
    }
    if let Some(list) = &condition.email_ids
        && !list
            .iter()
            .any(|id| ids::store_email(id) == Some(sent.email))
    {
        return false;
    }
    if let Some(list) = &condition.thread_ids
        && !list
            .iter()
            .any(|id| ids::store_thread(id) == Some(sent.thread))
    {
        return false;
    }
    if condition
        .undo_status
        .is_some_and(|status| status != super::undo_status(submission.undo))
    {
        return false;
    }
    let send_at = sent.send_at.secs();
    if let Some(date) = condition.before
        && send_at >= date.unix_timestamp()
    {
        return false;
    }
    if let Some(date) = condition.after
        && send_at < date.unix_timestamp()
    {
        return false;
    }
    true
}

fn matches(submission: &Submission, filter: &Filter<EmailSubmissionFilterCondition>) -> bool {
    match filter {
        Filter::Condition(condition) => condition_matches(submission, condition),
        Filter::Operator(operator) => {
            let mut results = operator.conditions.iter().map(|f| matches(submission, f));
            match operator.operator {
                Operator::And => results.all(|r| r),
                Operator::Or => results.any(|r| r),
                Operator::Not => !results.any(|r| r),
                _ => false,
            }
        }
        _ => false,
    }
}

fn compare(
    a: &Submission,
    b: &Submission,
    sort: &CheckedSort<'_, EmailSubmissionSort>,
) -> Ordering {
    sort.compare(a, b, |property, _, a, b| match property {
        EmailSubmissionSort::EmailId => a.sent.email.cmp(&b.sent.email),
        EmailSubmissionSort::ThreadId => a.sent.thread.cmp(&b.sent.thread),
        EmailSubmissionSort::SentAt => a.sent.send_at.secs().cmp(&b.sent.send_at.secs()),
        _ => Ordering::Equal,
    })
    .then_with(|| a.id.cmp(&b.id))
}

/// The ids of the account's submissions that match, in order.
fn results(
    all: &[Submission],
    filter: Option<&Filter<EmailSubmissionFilterCondition>>,
    sort: &CheckedSort<'_, EmailSubmissionSort>,
) -> Vec<SubmissionId> {
    let mut found: Vec<&Submission> = all
        .iter()
        .filter(|s| filter.is_none_or(|f| matches(s, f)))
        .collect();
    found.sort_by(|a, b| compare(a, b, sort));
    found.iter().map(|s| s.id).collect()
}

fn total(list: &[SubmissionId]) -> UnsignedInt {
    UnsignedInt::try_from(list.len()).unwrap_or(UnsignedInt::MAX)
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    query: EmailSubmissionQueryArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&query.account_id)?;
    let sort = CheckedSort::new(&query.sort)?;
    let state = ctx.state(DataType::EmailSubmission).await?;
    let all = ctx.store().submissions(ctx.id(), None, Ctx::now()?).await?;
    let found = results(&all, query.filter.as_ref(), &sort);
    let anchor = match &query.anchor {
        Some(anchor) => Some(
            ids::store_submission(anchor)
                .ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
        ),
        None => None,
    };
    let asked = query.limit.map(UnsignedInt::get);
    let limit = asked.map_or(MAX_LIMIT, |n| n.min(MAX_LIMIT));
    let page = crate::paging::page(
        &found,
        query.position.get(),
        anchor.map(|a| (a, query.anchor_offset.get())),
        Some(limit),
    )?;
    Ok(MethodResponse::EmailSubmissionQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: state,
        can_calculate_changes: true,
        position: UnsignedInt::saturating(page.position),
        ids: page
            .ids
            .into_iter()
            .map(ids::submission_id)
            .collect::<Result<_, _>>()?,
        total: query.calculate_total.then(|| total(&found)),
        limit: (asked != Some(limit)).then(|| UnsignedInt::saturating(limit)),
    }))
}

/// `removed` and `added` from the changed submissions and the current
/// results (see the module comment).
fn diff(
    created: &BTreeSet<SubmissionId>,
    touched: &BTreeSet<SubmissionId>,
    current: &[SubmissionId],
) -> (Vec<SubmissionId>, Vec<(SubmissionId, u64)>) {
    let removed = touched
        .iter()
        .filter(|id| !created.contains(id))
        .copied()
        .collect();
    let added = current
        .iter()
        .enumerate()
        .filter(|(_, id)| touched.contains(id))
        .map(|(index, id)| (*id, u64::try_from(index).unwrap_or(u64::MAX)))
        .collect();
    (removed, added)
}

pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: EmailSubmissionQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let sort = CheckedSort::new(&args.sort)?;
    let cannot = || CallError::from(MethodErrorType::CannotCalculateChanges);
    let since = ids::parse_state(&args.since_query_state).ok_or_else(cannot)?;
    let max_changes = args.max_changes.map_or(MAX_QUERY_CHANGES, |n| {
        n.get().to_usize_saturating().min(MAX_QUERY_CHANGES)
    });
    let found = ctx
        .store()
        .changes_since(ctx.id(), DataType::EmailSubmission, since, max_changes)
        .await?;
    let log = match found {
        ChangesSince::Changes(log) => log,
        // RFC 8620 §5.6: over the client's `maxChanges`.
        ChangesSince::TooMany if args.max_changes.is_some() => {
            return Err(MethodErrorType::TooManyChanges.into());
        }
        ChangesSince::TooMany | ChangesSince::CannotCalculate => return Err(cannot()),
    };
    let now = Ctx::now()?;
    let expired = ctx.store().expired_submissions(ctx.id(), now).await?;
    let count = log
        .created
        .len()
        .saturating_add(log.updated.len())
        .saturating_add(log.destroyed.len())
        .saturating_add(expired.len());
    if log.has_more_changes || count > max_changes {
        return Err(if args.max_changes.is_some() {
            MethodErrorType::TooManyChanges.into()
        } else {
            cannot()
        });
    }
    let to_ids = |list: &[i64]| -> BTreeSet<SubmissionId> {
        list.iter()
            .filter_map(|&row| SubmissionId::from_raw(row))
            .collect()
    };
    let created = to_ids(&log.created);
    let mut touched = created.clone();
    touched.extend(to_ids(&log.updated));
    touched.extend(to_ids(&log.destroyed));
    touched.extend(expired);
    let all = ctx.store().submissions(ctx.id(), None, now).await?;
    let current = results(&all, args.filter.as_ref(), &sort);
    let (removed, added) = diff(&created, &touched, &current);
    Ok(MethodResponse::EmailSubmissionQueryChanges(
        QueryChangesResponse {
            account_id: ctx.jmap_id(),
            old_query_state: args.since_query_state,
            new_query_state: ids::state(log.new_state),
            total: args.calculate_total.then(|| total(&current)),
            removed: removed
                .into_iter()
                .map(ids::submission_id)
                .collect::<Result<_, _>>()?,
            added: added
                .into_iter()
                .map(|(id, index)| {
                    Ok(AddedItem {
                        id: ids::submission_id(id)?,
                        index: UnsignedInt::saturating(index),
                    })
                })
                .collect::<Result<_, crate::error::Bug>>()?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::method::Comparator;
    use nebula_store::{EmailId, IdentityId, NewSubmission, ThreadId, UndoState, UnixTime};
    use serde_json::json;

    use super::*;

    fn submission(id: i64, email: i64, thread: i64, send_at: i64) -> Submission {
        Submission {
            id: SubmissionId::from_raw(id).unwrap(),
            undo: UndoState::Final,
            send_when_signed_out: false,
            sent: NewSubmission {
                identity: IdentityId::from_raw(1).unwrap(),
                email: EmailId::from_raw(email).unwrap(),
                thread: ThreadId::from_raw(thread).unwrap(),
                envelope: serde_json::from_value(json!({
                    "mailFrom": {"email": "alice@example.com"},
                    "rcptTo": [{"email": "bob@example.com"}]
                }))
                .unwrap(),
                send_at: UnixTime::from_secs(send_at),
                smtp_reply: "250 OK".into(),
                expires_at: UnixTime::from_secs(send_at + 86_400),
                message_id: None,
                envid: None,
            },
            status: std::collections::BTreeMap::new(),
            dsn_emails: Vec::new(),
            mdn_emails: Vec::new(),
        }
    }

    fn filter(value: serde_json::Value) -> Filter<EmailSubmissionFilterCondition> {
        Filter::parse(value).unwrap()
    }

    fn sort(value: serde_json::Value) -> Vec<Comparator<EmailSubmissionSort>> {
        Comparator::parse_list(value).unwrap()
    }

    fn rows(list: &[SubmissionId]) -> Vec<i64> {
        list.iter().map(|id| id.get()).collect()
    }

    /// `2026-10-24T10:00:00Z`.
    const T0: i64 = 1_792_836_000;

    fn sample() -> Vec<Submission> {
        vec![
            submission(1, 30, 3, T0),
            submission(2, 10, 1, T0 + 60),
            submission(3, 20, 3, T0 + 30),
        ]
    }

    #[test]
    fn filters() {
        let all = sample();
        let none = CheckedSort::new(&[]).unwrap();
        let ids = |value| rows(&results(&all, Some(&filter(value)), &none));
        assert_eq!(ids(json!({"emailIds": ["e10", "e30", "e99"]})), [1, 2]);
        assert_eq!(ids(json!({"threadIds": ["t3"]})), [1, 3]);
        assert_eq!(ids(json!({"identityIds": ["i1"]})), [1, 2, 3]);
        assert!(ids(json!({"identityIds": ["i2"]})).is_empty());
        // Not one of our ids: nothing.
        assert!(ids(json!({"emailIds": ["x10"]})).is_empty());
        assert_eq!(ids(json!({"undoStatus": "final"})), [1, 2, 3]);
        assert!(ids(json!({"undoStatus": "pending"})).is_empty());
        assert!(ids(json!({"undoStatus": "canceled"})).is_empty());
        // Held ones: one being sent is still pending.
        let mut held = sample();
        held[0].undo = UndoState::Pending;
        held[1].undo = UndoState::Sending;
        held[2].undo = UndoState::Canceled;
        let held_ids = |value| rows(&results(&held, Some(&filter(value)), &none));
        assert_eq!(held_ids(json!({"undoStatus": "pending"})), [1, 2]);
        assert_eq!(held_ids(json!({"undoStatus": "canceled"})), [3]);
        assert!(held_ids(json!({"undoStatus": "final"})).is_empty());
        // `after` is inclusive, `before` exclusive.
        assert_eq!(ids(json!({"after": "2026-10-24T10:00:30Z"})), [2, 3]);
        assert_eq!(ids(json!({"before": "2026-10-24T10:00:30Z"})), [1]);
        assert_eq!(
            ids(json!({"operator": "NOT", "conditions": [{"threadIds": ["t3"]}]})),
            [2]
        );
        assert_eq!(
            ids(json!({"operator": "OR", "conditions": [
                {"emailIds": ["e10"]}, {"emailIds": ["e20"]}
            ]})),
            [2, 3]
        );
    }

    #[test]
    fn sorts() {
        let all = sample();
        let ids = |value| {
            let comparators = sort(value);
            rows(&results(
                &all,
                None,
                &CheckedSort::new(&comparators).unwrap(),
            ))
        };
        assert_eq!(ids(json!([])), [1, 2, 3]);
        assert_eq!(ids(json!([{"property": "emailId"}])), [2, 3, 1]);
        assert_eq!(
            ids(json!([{"property": "sentAt", "isAscending": false}])),
            [2, 3, 1]
        );
        // Ties by thread go in submission order, unless sorted again.
        assert_eq!(ids(json!([{"property": "threadId"}])), [2, 1, 3]);
        assert_eq!(
            ids(json!([
                {"property": "threadId", "isAscending": false},
                {"property": "sentAt", "isAscending": false}
            ])),
            [3, 1, 2]
        );
    }

    /// RFC 8620 §5.5: no sort property is text, so an offered collation
    /// changes nothing, and one the session doesn't offer is
    /// `unsupportedSort` (never accepted and ignored).
    #[test]
    fn sorts_check_collations() {
        let all = sample();
        let numeric = sort(json!([{"property": "emailId", "collation": "i;ascii-numeric"}]));
        assert_eq!(
            rows(&results(&all, None, &CheckedSort::new(&numeric).unwrap())),
            [2, 3, 1]
        );
        let octet = sort(json!([{"property": "sentAt", "collation": "i;octet"}]));
        assert_eq!(
            CheckedSort::new(&octet).err().map(|e| e.kind()),
            Some(MethodErrorType::UnsupportedSort)
        );
    }

    #[test]
    fn changes_diff() {
        let set = |list: &[i64]| -> BTreeSet<SubmissionId> {
            list.iter()
                .map(|&n| SubmissionId::from_raw(n).unwrap())
                .collect()
        };
        let current: Vec<SubmissionId> = [3, 1, 4]
            .iter()
            .map(|&n| SubmissionId::from_raw(n).unwrap())
            .collect();
        // 4 is new, 1 was updated, 2 was destroyed (or expired).
        let (removed, added) = diff(&set(&[4]), &set(&[1, 2, 4]), &current);
        assert_eq!(rows(&removed), [1, 2]);
        let added: Vec<(i64, u64)> = added.into_iter().map(|(id, i)| (id.get(), i)).collect();
        assert_eq!(added, [(1, 1), (4, 2)]);
    }
}
