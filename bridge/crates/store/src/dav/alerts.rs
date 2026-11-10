// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendar alerts already shown, so a restart
//! of the bridge does not show them again. One row per resource, alert,
//! occurrence and ring (repeated alarms); rows go away with their resource (`ON DELETE CASCADE`)
//! and with [`Store::purge_fired_alerts`] once they are old.

use diesel::{
    ExpressionMethods as _, Insertable, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
};

use crate::schema::{calendar_alerts_fired, dav_resources};
use crate::{AccountId, ResourceId, Store, StoreError, UnixTime};

/// Longest alert id kept (the JMAP `CalendarAlert` allows 255 bytes).
const MAX_ALERT_ID: usize = 255;

/// One alert of one occurrence of an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiredAlert {
    /// The event.
    pub resource: ResourceId,
    /// The key of the alert in the event's `alerts`.
    pub alert_id: String,
    /// The UTC start of the occurrence it belongs to.
    pub occurrence: UnixTime,
    /// Which ring of the alarm (0 the first; then its `REPEAT`s).
    pub repetition: u32,
}

#[derive(Insertable)]
#[diesel(table_name = calendar_alerts_fired)]
struct NewFired<'a> {
    account_id: i64,
    resource_id: i64,
    alert_id: &'a str,
    occurrence: i64,
    fired_at: i64,
    repetition: i64,
}

impl Store {
    /// The alerts of the account shown at or after `since`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn fired_alerts(
        &self,
        account: AccountId,
        since: UnixTime,
    ) -> Result<Vec<FiredAlert>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<(i64, String, i64, i64)> = calendar_alerts_fired::table
                .filter(calendar_alerts_fired::account_id.eq(account.get()))
                .filter(calendar_alerts_fired::fired_at.ge(since.secs()))
                .order(calendar_alerts_fired::id.asc())
                .select((
                    calendar_alerts_fired::resource_id,
                    calendar_alerts_fired::alert_id,
                    calendar_alerts_fired::occurrence,
                    calendar_alerts_fired::repetition,
                ))
                .load(conn)?;
            rows.into_iter()
                .map(|(resource, alert_id, occurrence, repetition)| {
                    Ok(FiredAlert {
                        resource: ResourceId::from_raw(resource)
                            .ok_or(StoreError::Corrupt("resource id"))?,
                        alert_id,
                        occurrence: UnixTime::from_secs(occurrence),
                        repetition: u32::try_from(repetition)
                            .map_err(|_| StoreError::Corrupt("alert repetition"))?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Marks an alert as shown at `at`. `true` if it was not yet: only
    /// then should it be shown (at most once, also across restarts and
    /// with two tasks racing).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the resource is not the account's;
    /// [`StoreError::Invalid`] for an empty or too long alert id; database
    /// errors.
    pub async fn record_fired_alert(
        &self,
        account: AccountId,
        alert: FiredAlert,
        at: UnixTime,
    ) -> Result<bool, StoreError> {
        if alert.alert_id.is_empty() || alert.alert_id.len() > MAX_ALERT_ID {
            return Err(StoreError::Invalid("alert id"));
        }
        self.write(move |conn| {
            let owner: Option<i64> = dav_resources::table
                .find(alert.resource.get())
                .select(dav_resources::account_id)
                .first(conn)
                .optional()?;
            if owner != Some(account.get()) {
                return Err(StoreError::NotFound("resource"));
            }
            let inserted = diesel::insert_or_ignore_into(calendar_alerts_fired::table)
                .values(NewFired {
                    account_id: account.get(),
                    resource_id: alert.resource.get(),
                    alert_id: &alert.alert_id,
                    occurrence: alert.occurrence.secs(),
                    fired_at: at.secs(),
                    repetition: i64::from(alert.repetition),
                })
                .execute(conn)?;
            Ok(inserted == 1)
        })
        .await
    }

    /// Forgets the account's alerts shown before `before`; returns how
    /// many. Keep them longer than an alert can still be shown late.
    ///
    /// # Errors
    /// On database errors.
    pub async fn purge_fired_alerts(
        &self,
        account: AccountId,
        before: UnixTime,
    ) -> Result<usize, StoreError> {
        self.write(move |conn| {
            Ok(diesel::delete(
                calendar_alerts_fired::table
                    .filter(calendar_alerts_fired::account_id.eq(account.get()))
                    .filter(calendar_alerts_fired::fired_at.lt(before.secs())),
            )
            .execute(conn)?)
        })
        .await
    }
}
