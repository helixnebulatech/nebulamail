// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The `/changes` methods (RFC 8620 §5.2), shared by every data type: the
//! store's change log, with its row ids turned into JMAP ids.

use std::collections::BTreeSet;

use nebula_jmap_proto::method::{
    AddedItem, ChangesArguments, ChangesResponse, QueryChangesResponse,
};
use nebula_jmap_proto::types::PositiveInt;
use nebula_jmap_proto::{MethodErrorType, UnsignedInt};
use nebula_store::{ChangesSince, DataType};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

/// Changes returned when the client doesn't set `maxChanges`, and the most
/// it may ask for.
const DEFAULT_MAX_CHANGES: usize = 500;
const MAX_CHANGES: usize = 5_000;

fn max_changes(asked: Option<PositiveInt>) -> usize {
    asked.map_or(DEFAULT_MAX_CHANGES, |n| {
        n.get().to_usize_saturating().min(MAX_CHANGES)
    })
}

/// The changes since `since` within the limit (RFC 8620 §5.2: the ids
/// returned never exceed it; one state is never split, so a first state
/// over it is `cannotCalculateChanges`). Without the client's `maxChanges`
/// the limit is the server's own: [`DEFAULT_MAX_CHANGES`], raised to fit a
/// bigger first state up to [`MAX_CHANGES`] (still a limit "set
/// automatically by the server" that the answer honours).
async fn changes_within(
    ctx: &Ctx<'_>,
    data_type: DataType,
    since: nebula_store::Seq,
    asked: Option<PositiveInt>,
) -> Result<nebula_store::Changes, CallError> {
    let store = ctx.store();
    let mut found = store
        .changes_since(ctx.id(), data_type, since, max_changes(asked))
        .await?;
    if asked.is_none() && found == ChangesSince::TooMany {
        found = store
            .changes_since(ctx.id(), data_type, since, MAX_CHANGES)
            .await?;
    }
    match found {
        ChangesSince::Changes(changes) => Ok(changes),
        ChangesSince::CannotCalculate | ChangesSince::TooMany => {
            Err(MethodErrorType::CannotCalculateChanges.into())
        }
    }
}

/// `Foo/changes` for `data_type`. `to_id` turns a row id into the JMAP id.
pub(crate) async fn changes<I>(
    ctx: &Ctx<'_>,
    data_type: DataType,
    args: ChangesArguments,
    to_id: impl Fn(i64) -> Result<I, Bug>,
) -> Result<ChangesResponse<I>, CallError> {
    changes_detail(ctx, data_type, args, to_id)
        .await
        .map(|(response, _)| response)
}

/// [`changes`], and whether every updated object only had its counters
/// changed (for `Mailbox/changes` `updatedProperties`).
pub(crate) async fn changes_detail<I>(
    ctx: &Ctx<'_>,
    data_type: DataType,
    args: ChangesArguments,
    to_id: impl Fn(i64) -> Result<I, Bug>,
) -> Result<(ChangesResponse<I>, bool), CallError> {
    ctx.check_account(&args.account_id)?;
    let since = ids::parse_state(&args.since_state)
        .ok_or(CallError::from(MethodErrorType::CannotCalculateChanges))?;
    let changes = changes_within(ctx, data_type, since, args.max_changes).await?;
    let convert = |list: Vec<i64>| list.into_iter().map(&to_id).collect::<Result<Vec<_>, _>>();
    let only_counts = changes.only_counts_updated;
    let response = ChangesResponse {
        account_id: ctx.jmap_id(),
        old_state: args.since_state,
        new_state: ids::state(changes.new_state),
        has_more_changes: changes.has_more_changes,
        created: convert(changes.created)?,
        updated: convert(changes.updated)?,
        destroyed: convert(changes.destroyed)?,
    };
    Ok((response, only_counts))
}

/// Most changes a `/queryChanges` computes from the log (as
/// `Email/queryChanges`); beyond, `cannotCalculateChanges`.
const MAX_QUERY_CHANGES: usize = 10_000;

/// What changed in the log of a data type since a query state, for the
/// `/queryChanges` of the groupware objects (RFC 8620 §5.6), computed as
/// `EmailSubmission/queryChanges`: no filtered or sorted property is
/// outside the object itself, so every object created, updated or
/// destroyed since goes to `removed` (except the new ones) and, if it is
/// in the current results, to `added` with its index (the RFC allows ids
/// in `removed` that were not in the results). `upToId` is ignored (the
/// RFC allows it).
pub(crate) struct QueryLog {
    created: BTreeSet<i64>,
    touched: BTreeSet<i64>,
    new_state: nebula_jmap_proto::types::State,
}

impl QueryLog {
    /// The log of `data_type` since `since`.
    ///
    /// # Errors
    /// `cannotCalculateChanges` for a state the log no longer has (or not
    /// ours) or more than [`MAX_QUERY_CHANGES`] changes; `tooManyChanges`
    /// past the client's `maxChanges`.
    pub(crate) async fn since(
        ctx: &Ctx<'_>,
        data_type: DataType,
        since: &nebula_jmap_proto::types::State,
        asked: Option<PositiveInt>,
    ) -> Result<Self, CallError> {
        let cannot = || CallError::from(MethodErrorType::CannotCalculateChanges);
        let since = ids::parse_state(since).ok_or_else(cannot)?;
        let max = asked.map_or(MAX_QUERY_CHANGES, |n| {
            n.get().to_usize_saturating().min(MAX_QUERY_CHANGES)
        });
        let found = ctx
            .store()
            .changes_since(ctx.id(), data_type, since, max)
            .await?;
        let log = match found {
            ChangesSince::Changes(log) => log,
            // RFC 8620 §5.6: over the client's `maxChanges`.
            ChangesSince::TooMany if asked.is_some() => {
                return Err(MethodErrorType::TooManyChanges.into());
            }
            ChangesSince::TooMany | ChangesSince::CannotCalculate => return Err(cannot()),
        };
        let count = log
            .created
            .len()
            .saturating_add(log.updated.len())
            .saturating_add(log.destroyed.len());
        if log.has_more_changes || count > max {
            return Err(if asked.is_some() {
                MethodErrorType::TooManyChanges.into()
            } else {
                cannot()
            });
        }
        let created: BTreeSet<i64> = log.created.iter().copied().collect();
        let mut touched = created.clone();
        touched.extend(log.updated.iter().copied());
        touched.extend(log.destroyed.iter().copied());
        Ok(Self {
            created,
            touched,
            new_state: ids::state(log.new_state),
        })
    }

    /// The rows changed since the state (created, updated or destroyed).
    pub(crate) fn touched(&self) -> &BTreeSet<i64> {
        &self.touched
    }

    /// Counts `rows` as changed too: objects whose place or match depends
    /// on a changed one (`Mailbox/queryChanges` with a tree).
    pub(crate) fn touch(&mut self, rows: impl IntoIterator<Item = i64>) {
        self.touched.extend(rows);
    }

    /// The response, from the current results (`row`: each one's log
    /// id).
    pub(crate) fn response<I: Clone>(
        self,
        ctx: &Ctx<'_>,
        old_state: nebula_jmap_proto::types::State,
        current: &[(i64, I)],
        calculate_total: bool,
        to_id: impl Fn(i64) -> Result<I, Bug>,
    ) -> Result<QueryChangesResponse<I>, CallError> {
        let removed = self
            .touched
            .iter()
            .filter(|row| !self.created.contains(row))
            .map(|row| to_id(*row))
            .collect::<Result<Vec<_>, _>>()?;
        let added = current
            .iter()
            .enumerate()
            .filter(|(_, (row, _))| self.touched.contains(row))
            .map(|(index, (_, id))| AddedItem {
                id: id.clone(),
                index: UnsignedInt::saturating(u64::try_from(index).unwrap_or(u64::MAX)),
            })
            .collect();
        Ok(QueryChangesResponse {
            account_id: ctx.jmap_id(),
            old_query_state: old_state,
            new_query_state: self.new_state,
            total: calculate_total
                .then(|| UnsignedInt::try_from(current.len()).unwrap_or(UnsignedInt::MAX)),
            removed,
            added,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits() {
        assert_eq!(max_changes(None), DEFAULT_MAX_CHANGES);
        assert_eq!(max_changes(PositiveInt::new(10)), 10);
        assert_eq!(max_changes(PositiveInt::new(1_000_000)), MAX_CHANGES);
    }
}
