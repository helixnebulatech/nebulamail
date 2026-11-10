// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarTrash/get` and `CalendarTrash/set`, the bridge's own methods
//! for Nextcloud's calendar trash bin (the
//! shapes are in `nebula_jmap_proto::object::calendar_trash`). The bin is
//! read from the server on every call (nothing is kept); restoring brings
//! the calendar or event back through the next sync, as any change.
//! Only in the user's own account, and only where the server has one
//! (else `accountNotSupportedByMethod`).

use std::collections::BTreeMap;
use std::sync::Arc;

use nebula_groupware::{GroupwareAccount, GroupwareError, TrashItem, TrashKind};
use nebula_jmap_proto::object::{
    CalendarTrashGetArguments, CalendarTrashGetResponse, CalendarTrashSetArguments,
    CalendarTrashSetResponse, TrashId, TrashKind as JmapKind, TrashedCalendar, TrashedEvent,
};
use nebula_jmap_proto::types::UtcDate;
use nebula_jmap_proto::{MethodError, MethodErrorType, MethodResponse, SetError};
use nebula_store::UnixTime;

use super::ids;
use super::set_error::set_error;
use crate::core::context::Ctx;
use crate::error::CallError;

fn unsupported() -> CallError {
    MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
        .with_description("this account's calendar server has no trash bin")
        .into()
}

/// The account's calendars, if its server has a trash bin.
fn bin_account(ctx: &Ctx<'_>) -> Result<Arc<GroupwareAccount>, CallError> {
    let dav = super::account(ctx)?;
    if ctx.scope.is_shared() || !dav.nextcloud_support().trash_bin {
        return Err(unsupported());
    }
    Ok(dav)
}

fn date(time: Option<UnixTime>) -> Option<UtcDate> {
    time.and_then(|t| UtcDate::from_unix_timestamp(t.secs()).ok())
}

const fn item(id: TrashId) -> TrashItem {
    TrashItem {
        kind: match id.kind {
            JmapKind::Calendar => TrashKind::Calendar,
            JmapKind::Event => TrashKind::Event,
        },
        number: id.number,
    }
}

fn call_error(error: &GroupwareError) -> CallError {
    match error {
        GroupwareError::Unsupported => unsupported(),
        GroupwareError::Unauthorized => MethodError::new(MethodErrorType::ServerUnavailable)
            .with_description("the calendar server refused the login")
            .into(),
        other => {
            tracing::warn!(error = %other, "calendar trash bin failed");
            MethodError::new(MethodErrorType::ServerFail)
                .with_description("the calendar server failed")
                .into()
        }
    }
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: CalendarTrashGetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = bin_account(ctx)?;
    let trash = dav.trash().await.map_err(|e| call_error(&e))?;
    let calendars = trash
        .calendars
        .into_iter()
        .map(|calendar| TrashedCalendar {
            id: TrashId {
                kind: JmapKind::Calendar,
                number: calendar.number,
            },
            name: calendar.name,
            color: calendar.color,
            deleted_at: date(calendar.deleted_at),
        })
        .collect();
    let mut events = Vec::with_capacity(trash.events.len());
    for event in trash.events {
        events.push(TrashedEvent {
            id: TrashId {
                kind: JmapKind::Event,
                number: event.number,
            },
            title: event.title,
            utc_start: date(event.utc_start),
            calendar_id: event.calendar.map(ids::calendar_id).transpose()?,
            deleted_at: date(event.deleted_at),
        });
    }
    Ok(MethodResponse::CalendarTrashGet(CalendarTrashGetResponse {
        account_id: ctx.jmap_id(),
        retention_seconds: trash.retention_secs,
        calendars,
        events,
    }))
}

/// Splits the results of one action into done and not done.
fn sort_out(
    ids: &[TrashId],
    results: Vec<Result<(), GroupwareError>>,
) -> (Vec<TrashId>, BTreeMap<TrashId, SetError>) {
    let mut done = Vec::new();
    let mut failed = BTreeMap::new();
    for (id, result) in ids.iter().zip(results) {
        match result {
            Ok(()) => done.push(*id),
            Err(error) => {
                failed.insert(*id, set_error(&error));
            }
        }
    }
    (done, failed)
}

pub(crate) async fn set(
    ctx: &Ctx<'_>,
    args: CalendarTrashSetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = bin_account(ctx)?;
    let (restored, not_restored) = if args.restore.is_empty() {
        (Vec::new(), BTreeMap::new())
    } else {
        let items: Vec<TrashItem> = args.restore.iter().copied().map(item).collect();
        let results = dav
            .restore_trashed(&items)
            .await
            .map_err(|e| call_error(&e))?;
        sort_out(&args.restore, results)
    };
    let (destroyed, not_destroyed) = if args.destroy.is_empty() {
        (Vec::new(), BTreeMap::new())
    } else {
        let items: Vec<TrashItem> = args.destroy.iter().copied().map(item).collect();
        let results = dav
            .delete_trashed(&items)
            .await
            .map_err(|e| call_error(&e))?;
        sort_out(&args.destroy, results)
    };
    Ok(MethodResponse::CalendarTrashSet(CalendarTrashSetResponse {
        account_id: ctx.jmap_id(),
        restored,
        not_restored,
        destroyed,
        not_destroyed,
    }))
}
