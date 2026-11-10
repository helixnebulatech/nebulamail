// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP for Calendars, the read side, over
//! CalDAV through `nebula-groupware`: `Calendar/get|changes`,
//! `CalendarEvent/get|changes|query|parse` and `ParticipantIdentity/get`.
//!
//! This answers the way Stalwart does where the web depends on it
//! (`apps/web/src/calendar/api.ts`): occurrence ids for series, each with
//! `baseEventId`, and an event that doesn't repeat as itself with
//! `baseEventId: null` (Stalwart 1.0); occurrences in the event's zone (the
//! query's for UTC and floating ones); `utcStart`/`utcEnd` only when asked
//! for; unconverted iCalendar properties under `iCalendar` when
//! `properties` is given.
//!
//! The capabilities (`calendars` and `calendars:parse`, always together)
//! are announced per account only while its DAV server has a calendar
//! home and accepts the login ([`capability`]).

pub(crate) mod calendar;
pub(crate) mod counter;
pub(crate) mod event;
pub(crate) mod identity;
pub(crate) mod ids;
pub(crate) mod itip_text;
pub(crate) mod parse;
pub(crate) mod query;
pub(crate) mod set_calendar;
pub(crate) mod set_error;
pub(crate) mod set_event;
pub(crate) mod set_itip;
pub(crate) mod set_patch;
pub(crate) mod trash;

use std::sync::Arc;

use nebula_groupware::model::calendar::Zone;
use nebula_groupware::{GroupwareAccount, GroupwareError};
use nebula_jmap_proto::session::{BridgeAccountCapability, CalendarsAccountCapability};
use nebula_jmap_proto::types::{Duration, TimeZoneId};
use nebula_jmap_proto::{MethodError, MethodErrorType, UnsignedInt};
use nebula_store::AccountId;

use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// Longest `after`–`before` range of an expanded query: 52 weeks and a
/// day, as Stalwart (`maxExpandedQueryDuration`).
pub(crate) const MAX_EXPANDED_DURATION: &str = "P52W1D";
/// Most occurrences one expanded query returns (Stalwart's
/// `maxRecurrenceExpansions`); more is `cannotCalculateOccurrences`.
pub(crate) const MAX_EXPANDED_RESULTS: usize = 3000;

/// The `urn:ietf:params:jmap:calendars` account capability, if the
/// account has calendars now (fields as Stalwart sends them; at most
/// [`set_itip::MAX_RECIPIENTS`] invitation recipients per write).
pub(crate) fn capability(inner: &Inner, account: AccountId) -> Option<CalendarsAccountCapability> {
    let running = inner.groupware.as_ref()?.account(account)?;
    if !running.availability().calendars {
        return None;
    }
    Some(CalendarsAccountCapability {
        max_calendars_per_event: None,
        min_date_time: "0001-01-01T00:00:00Z".parse().ok()?,
        max_date_time: "9999-12-31T23:59:59Z".parse().ok()?,
        max_expanded_query_duration: MAX_EXPANDED_DURATION.parse().ok()?,
        max_participants_per_event: Some(UnsignedInt::saturating(
            u64::try_from(set_itip::MAX_RECIPIENTS).unwrap_or(20),
        )),
        may_create_calendar: true,
    })
}

/// The bridge's own calendar extras of the account's server (Nextcloud's
/// publishing, subscriptions and trash bin), while it has calendars.
pub(crate) fn bridge_capability(inner: &Inner, account: AccountId) -> BridgeAccountCapability {
    let support = inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(account))
        .filter(|running| running.availability().calendars)
        .map(|running| running.nextcloud_support())
        .unwrap_or_default();
    BridgeAccountCapability {
        calendar_publishing: support.publishing,
        calendar_subscriptions: support.subscriptions,
        calendar_trash: support.trash_bin,
        alerts: crate::core::alerts::server_alerts(inner, account),
    }
}

/// The longest expanded range, in seconds.
pub(crate) fn max_expanded_seconds() -> Result<u64, Bug> {
    MAX_EXPANDED_DURATION
        .parse::<Duration>()
        .map(|duration| duration.seconds())
        .map_err(|_| Bug("maxExpandedQueryDuration"))
}

/// The account's running calendars, or `accountNotSupportedByMethod` when
/// it has none now (the DAV server is down for it or refused the login).
pub(crate) fn account(ctx: &Ctx<'_>) -> Result<Arc<GroupwareAccount>, CallError> {
    ctx.inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(ctx.id()))
        .filter(|running| running.availability().calendars)
        .ok_or_else(|| {
            MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
                .with_description("this account has no calendars now")
                .into()
        })
}

/// A `timeZone` argument calcard knows.
pub(crate) fn zone(id: &TimeZoneId) -> Result<Zone, CallError> {
    Zone::named(id.as_str())
        .ok_or_else(|| MethodError::invalid_arguments("unknown timeZone").into())
}

/// The user's own calendar addresses, without `mailto:`: the account's
/// address and those of `calendar-user-address-set`.
pub(crate) fn own_addresses(ctx: &Ctx<'_>, running: &GroupwareAccount) -> Vec<String> {
    let mut own = vec![ctx.account.email.trim().to_owned()];
    for uri in running.calendar_addresses() {
        let address = uri.strip_prefix("mailto:").unwrap_or(&uri).to_owned();
        if !own.iter().any(|known| known.eq_ignore_ascii_case(&address)) {
            own.push(address);
        }
    }
    own
}

/// A groupware failure as a method error (logged without content).
pub(crate) fn groupware_error(error: &GroupwareError) -> CallError {
    match error {
        GroupwareError::Store(store) => {
            tracing::error!(error = %store, "calendars: store failed");
            MethodError::server_fail("database error").into()
        }
        other => {
            tracing::warn!(error = %other, "calendars: groupware failed");
            MethodError::new(MethodErrorType::ServerUnavailable).into()
        }
    }
}

/// CPU work (calcard) off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CallError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Bug("calendar task").into())
}
