// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Writing the change log inside a synchronous transaction.

use diesel::SqliteConnection;
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};

use crate::schema::{changes, states};
use crate::{AccountId, Change, DataType, Seq, StoreError};

/// Records `list` as one new state inside an open transaction.
pub(crate) fn record_in(
    conn: &mut SqliteConnection,
    account: AccountId,
    data_type: DataType,
    list: &[Change],
) -> Result<Seq, StoreError> {
    let current: Option<i64> = states::table
        .find((account.get(), data_type.code()))
        .select(states::seq)
        .first(conn)
        .optional()?;
    let current = current.unwrap_or(0);
    if list.is_empty() {
        return Seq::from_db(current);
    }
    let next = current.checked_add(1).ok_or(StoreError::Corrupt("state"))?;
    // `EmailDelivery` is push only: its state moves, but nobody reads its
    // rows (no `/changes`), so none are kept.
    let rows: &[Change] = if data_type == DataType::EmailDelivery {
        &[]
    } else {
        list
    };
    let added = i64::try_from(rows.len()).map_err(|_| StoreError::Corrupt("change count"))?;
    diesel::insert_into(states::table)
        .values((
            states::account_id.eq(account.get()),
            states::data_type.eq(data_type.code()),
            states::seq.eq(next),
            states::oldest.eq(1_i64),
            states::log_rows.eq(added),
        ))
        .on_conflict((states::account_id, states::data_type))
        .do_update()
        .set((
            states::seq.eq(next),
            states::log_rows.eq(states::log_rows + added),
        ))
        .execute(conn)?;
    // One statement per row: diesel can't batch-insert tuples on SQLite.
    for change in rows {
        diesel::insert_into(changes::table)
            .values((
                changes::account_id.eq(account.get()),
                changes::data_type.eq(data_type.code()),
                changes::seq.eq(next),
                changes::object_id.eq(change.object),
                changes::kind.eq(change.kind.code()),
            ))
            .execute(conn)?;
    }
    Seq::from_db(next)
}
