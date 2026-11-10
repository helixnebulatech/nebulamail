// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Principals (RFC 9670) and free-busy (libre/ocupado) over CalDAV,
//! through `nebula-groupware`:
//! `Principal/get`, `Principal/query` and `Principal/getAvailability`.
//!
//! What the web does (`apps/web/src/calendar/api.ts`): for each guest of
//! the event being edited, `Principal/query` with `{"email": …}` and
//! `limit: 1`, then `Principal/getAvailability` for the ids found, and
//! «Ocupado» next to the guest if a period overlaps the event. Both need
//! `principals` and `principals:availability` in the session; without the
//! second the web hides «Ocupado».
//!
//! - **Capabilities** per account: `principals` while the DAV discovery
//!   found the user's principal (and the login works);
//!   `principals:availability` only when the server schedules
//!   (`calendar-auto-schedule` and a `schedule-outbox-URL`): Stalwart's
//!   DAV yes, Radicale no.
//! - **Ids** `r{n}`: a stable number of the principal's DAV location.
//! - **Who:** the user's principal and the ones the server shows (see
//!   `nebula_groupware::principals`). A principal's `name` is its
//!   `displayname` (or its address), `email` its first `mailto:`.
//! - **`timeZone`:** what the DAV server says of their calendars (each
//!   calendar's `calendar-timezone-id` or `calendar-timezone`, kept by
//!   the sync): the user's default calendar's zone, else the one zone all
//!   their calendars agree on; for someone else, the one zone of the
//!   calendars they share with the user. Nothing more is asked of the
//!   server (CalDAV has no zone of a principal); unknown → `null`.

pub(crate) mod availability;
pub(crate) mod get;
pub(crate) mod ids;
pub(crate) mod query;

use std::sync::Arc;

use nebula_dav::proto::CalendarAddress;
use nebula_groupware::principals::{
    Principal as DavPrincipal, PrincipalKind, PrincipalZones, address_number,
};
use nebula_groupware::sharing::location_number;
use nebula_groupware::{GroupwareAccount, GroupwareError};
use nebula_http::shared_account_id;
use nebula_jmap_proto::AccountId as AccountIdJmap;
use nebula_jmap_proto::object::{
    Principal, PrincipalCalendars, PrincipalCapabilities, PrincipalProperty, PrincipalType,
};
use nebula_jmap_proto::session::{
    PrincipalsAccountCapability, PrincipalsAvailabilityAccountCapability,
};
use nebula_jmap_proto::{MethodError, MethodErrorType, State};
use nebula_store::AccountId;
use nebula_store::ShareMode;
use serde_json::{Map, json};

use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::error::CallError;

/// Longest `Principal/getAvailability` range: 52 weeks and a day, as
/// Stalwart (`maxAvailabilityDuration`).
pub(crate) const MAX_AVAILABILITY_DURATION: &str = "P52W1D";

/// The `principals` and `principals:availability` account capabilities,
/// if the account has them now.
pub(crate) fn capability(
    inner: &Inner,
    account: AccountId,
) -> (
    Option<PrincipalsAccountCapability>,
    Option<PrincipalsAvailabilityAccountCapability>,
) {
    let Some(running) = inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(account))
    else {
        return (None, None);
    };
    let available = running.availability();
    let Some(own) = running
        .own_principal()
        .filter(|_| available.calendars || available.contacts)
    else {
        return (None, None);
    };
    let principals = ids::principal_id(own.number())
        .ok()
        .map(|id| PrincipalsAccountCapability {
            current_user_principal_id: Some(id),
        });
    let availability = running
        .supports_free_busy()
        .then(|| MAX_AVAILABILITY_DURATION.parse().ok())
        .flatten()
        .map(|max| PrincipalsAvailabilityAccountCapability {
            max_availability_duration: max,
        });
    (principals.clone(), principals.and(availability))
}

/// The account's running DAV side with its principal, or
/// `accountNotSupportedByMethod`.
pub(crate) fn account(ctx: &Ctx<'_>) -> Result<Arc<GroupwareAccount>, CallError> {
    ctx.inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(ctx.id()))
        .filter(|running| {
            let available = running.availability();
            (available.calendars || available.contacts) && running.own_principal().is_some()
        })
        .ok_or_else(|| {
            MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
                .with_description("this account has no principals now")
                .into()
        })
}

/// A groupware failure as a method error (logged without content).
pub(crate) fn groupware_error(error: &GroupwareError) -> CallError {
    match error {
        GroupwareError::NotDiscovered => MethodError::new(MethodErrorType::ServerUnavailable)
            .with_description("calendars are starting")
            .into(),
        other => crate::calendars::groupware_error(other),
    }
}

/// Someone `Principal/*` shows: a principal of the DAV server, or (on a
/// server that shares by address, sabre/dav and Apple) an address it
/// shows no principal for. Baïkal hides every other user, so the web
/// can only choose someone there by address; such an address gets an id
/// of its own (`r{n}` of [`nebula_groupware::principals::address_number`])
/// and is only good for sharing: never asked about free-busy.
#[derive(Debug, Clone)]
pub(crate) enum Who {
    Dav(DavPrincipal),
    Address(CalendarAddress),
}

impl Who {
    pub(crate) fn number(&self) -> i64 {
        match self {
            Self::Dav(principal) => principal.number(),
            Self::Address(address) => address_number(address),
        }
    }

    pub(crate) const fn is_own(&self) -> bool {
        matches!(self, Self::Dav(principal) if principal.own)
    }
}

/// The principal's address for JMAP: the account's own address for the
/// user (as Stalwart), else the first `mailto:` the server gave.
pub(crate) fn email_of(ctx: &Ctx<'_>, who: &Who) -> Option<String> {
    let principal = match who {
        Who::Dav(principal) => principal,
        Who::Address(address) => return Some(address.address().to_owned()),
    };
    if principal.own {
        let own = ctx.account.email.trim();
        if !own.is_empty() {
            return Some(own.to_owned());
        }
    }
    principal.addresses.first().map(|a| a.address().to_owned())
}

/// Whether one of the principal's addresses is `address` (no case); the
/// user's account address counts for their own.
pub(crate) fn has_email(ctx: &Ctx<'_>, who: &Who, address: &str) -> bool {
    let address = address.trim();
    match who {
        Who::Dav(principal) => {
            (principal.own && ctx.account.email.trim().eq_ignore_ascii_case(address))
                || principal
                    .addresses
                    .iter()
                    .any(|a| a.address().eq_ignore_ascii_case(address))
        }
        Who::Address(own) => own.address().eq_ignore_ascii_case(address),
    }
}

/// The display name: `displayname`, or the address, or nothing.
pub(crate) fn name_of(ctx: &Ctx<'_>, who: &Who) -> String {
    let name = match who {
        Who::Dav(principal) => principal.name.clone(),
        Who::Address(_) => None,
    };
    name.filter(|name| !name.trim().is_empty())
        .or_else(|| email_of(ctx, who))
        .unwrap_or_default()
}

pub(crate) const fn kind_of(who: &Who) -> PrincipalType {
    let principal = match who {
        Who::Dav(principal) => principal,
        Who::Address(_) => return PrincipalType::Individual,
    };
    match principal.kind {
        PrincipalKind::Individual => PrincipalType::Individual,
        PrincipalKind::Group => PrincipalType::Group,
        PrincipalKind::Resource => PrincipalType::Resource,
        PrincipalKind::Location => PrincipalType::Location,
        _ => PrincipalType::Other,
    }
}

/// What `Principal/get` adds from outside the directory: the shared
/// accounts of the people who share with the user (`accounts` and the
/// calendars' `accountId`) and how the user can share calendars
/// (`mayShareWith`). Read once per call.
#[derive(Debug, Default)]
pub(crate) struct Extras {
    /// Principal number → their shared account, with calendars or not.
    owners: Vec<(i64, AccountIdJmap, bool)>,
    /// How the user's own calendars can be shared.
    modes: Vec<ShareMode>,
    /// `timeZone`, from what the server says of the calendars.
    zones: PrincipalZones,
}

impl Extras {
    pub(crate) async fn load(ctx: &Ctx<'_>, running: &GroupwareAccount) -> Result<Self, CallError> {
        let owners = crate::sharing::accounts::owners(ctx.store(), ctx.id())
            .await?
            .into_iter()
            .filter_map(|owner| {
                let number = location_number(&owner.href)?;
                let id = shared_account_id(&ctx.jmap_id(), &owner.key)?;
                Some((number, id, owner.calendars))
            })
            .collect();
        let modes = running
            .calendar_share_modes()
            .await
            .map_err(|e| groupware_error(&e))?;
        let zones = running
            .principal_time_zones()
            .await
            .map_err(|e| groupware_error(&e))?;
        Ok(Self {
            owners,
            modes,
            zones,
        })
    }

    fn may_share_with(&self, running: &GroupwareAccount, who: &Who) -> bool {
        match who {
            Who::Dav(principal) => self
                .modes
                .iter()
                .any(|mode| running.can_name(principal, *mode)),
            Who::Address(_) => self.modes.contains(&ShareMode::CalendarServer),
        }
    }
}

/// A principal as JMAP, with only `properties` (all if `None`).
pub(crate) fn to_jmap(
    ctx: &Ctx<'_>,
    running: &GroupwareAccount,
    extras: &Extras,
    who: &Who,
    properties: Option<&[PrincipalProperty]>,
) -> Result<Principal, CallError> {
    let wants = |p: PrincipalProperty| properties.is_none_or(|list| list.contains(&p));
    let number = who.number();
    let id = ids::principal_id(number)?;
    let email = email_of(ctx, who);
    let calendar_address = match who {
        Who::Dav(principal) => principal.addresses.first().map(|a| a.as_str().to_owned()),
        Who::Address(address) => Some(address.as_str().to_owned()),
    }
    .or_else(|| email.as_ref().map(|e| format!("mailto:{e}")));
    let shared_account = extras
        .owners
        .iter()
        .find(|(owner, _, _)| *owner == number && !who.is_own());
    let mut out = Principal {
        id: Some(id.clone()),
        ..Principal::default()
    };
    if wants(PrincipalProperty::Type) {
        out.kind = Some(kind_of(who));
    }
    if wants(PrincipalProperty::Name) {
        out.name = Some(name_of(ctx, who));
    }
    if wants(PrincipalProperty::Description) {
        out.description = Some(None);
    }
    if wants(PrincipalProperty::Email) {
        out.email = Some(email);
    }
    if wants(PrincipalProperty::TimeZone) {
        out.time_zone = Some(match who {
            Who::Dav(principal) => extras.zones.of(number, principal.own).map(str::to_owned),
            Who::Address(_) => None,
        });
    }
    if wants(PrincipalProperty::Capabilities) {
        let calendars = running
            .availability()
            .calendars
            .then(|| PrincipalCalendars {
                account_id: if who.is_own() {
                    Some(ctx.jmap_id())
                } else {
                    shared_account
                        .filter(|(_, _, calendars)| *calendars)
                        .map(|(_, account, _)| account.clone())
                },
                may_get_availability: matches!(who, Who::Dav(_))
                    && running.supports_free_busy()
                    && calendar_address.is_some(),
                may_share_with: extras.may_share_with(running, who),
                calendar_address,
            });
        out.capabilities = Some(PrincipalCapabilities { calendars });
    }
    if wants(PrincipalProperty::Accounts) {
        let account = if who.is_own() {
            Some(ctx.jmap_id())
        } else {
            shared_account.map(|(_, account, _)| account.clone())
        };
        out.accounts = Some(account.map(|account| {
            let owner = json!({
                "urn:ietf:params:jmap:principals:owner": {
                    "accountIdForPrincipal": ctx.jmap_id(),
                    "principalId": id,
                }
            });
            let mut map = Map::new();
            map.insert(account.as_str().to_owned(), owner);
            map
        }));
    }
    Ok(out)
}

/// A state for the principals the account knows now: a hash of their ids
/// and names (there is no change log for principals; `/changes` is not
/// offered).
pub(crate) fn state_of(ctx: &Ctx<'_>, principals: &[DavPrincipal]) -> State {
    let mut keys: Vec<(i64, String)> = principals
        .iter()
        .map(|p| (p.number(), name_of(ctx, &Who::Dav(p.clone()))))
        .collect();
    keys.sort_unstable();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (number, name) in &keys {
        for byte in number
            .to_string()
            .bytes()
            .chain(*b":")
            .chain(name.bytes())
            .chain(*b"\n")
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    State::new(format!("r{hash:016x}"))
}

/// `Principal/changes` (RFC 9670 §3; RFC 8620 §5.2). There is no change
/// log for principals (the state is a hash of the ones known now), so:
/// the same state, no changes; any other, `cannotCalculateChanges`, and
/// the client fetches them again (RFC 8620 allows it).
pub(crate) fn changes(
    ctx: &Ctx<'_>,
    args: nebula_jmap_proto::method::ChangesArguments,
) -> Result<nebula_jmap_proto::method::ChangesResponse<nebula_jmap_proto::PrincipalId>, CallError> {
    ctx.check_account(&args.account_id)?;
    let running = account(ctx)?;
    let state = state_of(ctx, &running.known_principals());
    if args.since_state != state {
        return Err(MethodErrorType::CannotCalculateChanges.into());
    }
    Ok(nebula_jmap_proto::method::ChangesResponse {
        account_id: ctx.jmap_id(),
        old_state: args.since_state,
        new_state: state,
        has_more_changes: false,
        created: Vec::new(),
        updated: Vec::new(),
        destroyed: Vec::new(),
    })
}
