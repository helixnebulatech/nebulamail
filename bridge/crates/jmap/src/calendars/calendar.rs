// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Calendar/get` and `Calendar/changes`: the CalDAV calendars the last
//! sync saw. Every property is sent, as Stalwart does
//! (empty `shareWith` and default alerts, `includeInAvailability: "all"`),
//! plus the bridge's vendor property `https://helixnebula.tech/jmap/bridge/components`
//! when the request uses its capability (which items a calendar takes:
//! a list with only `VTODO` is a task list),
//! and on Nextcloud `…/isPublished` and `…/publicUrl` (a calendar the user
//! may publish) and `…/subscriptionUrl` (a webcal subscription).

use std::collections::BTreeMap;

use nebula_groupware::model::links::OpenLinks;
use nebula_jmap_proto::method::{ChangesArguments, GetArguments, GetResponse};
use nebula_jmap_proto::object::{
    Calendar, CalendarComponent, CalendarProperty, CalendarRights, DefaultAlerts,
    IncludeInAvailability,
};
use nebula_jmap_proto::{CalendarId, MethodResponse, UnsignedInt};
use nebula_store::{
    Collection, CollectionId, CollectionKind, CollectionProps, Components, DataType, Rights,
};

use super::ids;
use crate::changes;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// The JMAP form of stored default alarms (the bridge's own text).
fn default_alerts(stored: Option<&str>) -> DefaultAlerts {
    stored
        .map(nebula_groupware::model::defaults::DefaultAlerts::from_valarms)
        .unwrap_or_default()
        .iter()
        .map(|(id, alert)| (id.to_owned(), alert.to_jmap()))
        .collect()
}

/// What the user may do, from `current-user-privilege-set` (every right
/// but sharing when the server did not say). Sharing: see
/// [`crate::sharing`].
fn rights(rights: Option<Rights>) -> CalendarRights {
    let Some(rights) = rights else {
        return CalendarRights {
            may_share: false,
            ..CalendarRights::ALL
        };
    };
    let read = rights.contains(Rights::READ);
    let write = rights.contains(Rights::WRITE_CONTENT);
    CalendarRights {
        may_read_free_busy: read || rights.contains(Rights::READ_FREE_BUSY),
        may_read_items: read,
        may_write_all: write,
        may_write_own: write,
        may_update_private: write,
        may_rsvp: write,
        may_share: false,
        may_delete: rights.contains(Rights::UNBIND),
    }
}

/// What a calendar takes, by the bridge's vendor property: from
/// `supported-calendar-component-set`; events and tasks when the server
/// does not say (RFC 4791 §5.2.3: then it takes every component).
/// Journals and the rest are not items the bridge serves.
pub(crate) fn components(props: &CollectionProps) -> Vec<CalendarComponent> {
    let Some(set) = props.components else {
        return vec![CalendarComponent::Event, CalendarComponent::Todo];
    };
    [
        (Components::EVENT, CalendarComponent::Event),
        (Components::TODO, CalendarComponent::Todo),
    ]
    .into_iter()
    .filter(|(flag, _)| set.contains(*flag))
    .map(|(_, component)| component)
    .collect()
}

/// A calendar as JMAP. `bridge`: the request uses the bridge's vendor
/// capability, so [`COMPONENTS_PROPERTY`](nebula_jmap_proto::object::COMPONENTS_PROPERTY)
/// is sent (always with every property, else when asked for).
pub(crate) fn to_jmap(
    collection: &Collection,
    properties: Option<&[CalendarProperty]>,
    bridge: bool,
    links: &OpenLinks,
) -> Result<Calendar, Bug> {
    let wants = |p| properties.is_none_or(|list| list.contains(&p));
    let props = &collection.props;
    let mut out = Calendar {
        id: Some(ids::calendar_id(collection.id)?),
        ..Calendar::default()
    };
    if wants(CalendarProperty::Name) {
        out.name = Some(props.name.clone().unwrap_or_else(|| "Calendar".to_owned()));
    }
    if wants(CalendarProperty::Description) {
        out.description = Some(props.description.clone());
    }
    if wants(CalendarProperty::Color) {
        out.color = Some(props.color.clone());
    }
    if wants(CalendarProperty::SortOrder) {
        let order = props.sort_order.unwrap_or(0).max(0);
        out.sort_order = Some(UnsignedInt::saturating(u64::try_from(order).unwrap_or(0)));
    }
    if wants(CalendarProperty::IsSubscribed) {
        out.is_subscribed = Some(collection.is_subscribed);
    }
    if wants(CalendarProperty::IsVisible) {
        out.is_visible = Some(collection.is_visible);
    }
    if wants(CalendarProperty::IsDefault) {
        out.is_default = Some(collection.is_default);
    }
    if wants(CalendarProperty::IncludeInAvailability) {
        out.include_in_availability = Some(IncludeInAvailability::All);
    }
    if wants(CalendarProperty::DefaultAlertsWithTime) {
        out.default_alerts_with_time = Some(Some(default_alerts(
            props.default_alarms_datetime.as_deref(),
        )));
    }
    if wants(CalendarProperty::DefaultAlertsWithoutTime) {
        out.default_alerts_without_time =
            Some(Some(default_alerts(props.default_alarms_date.as_deref())));
    }
    if wants(CalendarProperty::TimeZone) {
        out.time_zone = Some(props.time_zone.as_deref().and_then(|tz| tz.parse().ok()));
    }
    if wants(CalendarProperty::ShareWith) {
        // `null` where the user can't share it; `get` fills it in.
        out.share_with = Some(props.sharing.mode.map(|_| BTreeMap::new()));
    }
    if wants(CalendarProperty::MyRights) {
        out.my_rights = Some(CalendarRights {
            may_share: props.sharing.mode.is_some(),
            ..rights(props.rights)
        });
    }
    if bridge && wants(CalendarProperty::Components) {
        out.components = Some(components(props));
    }
    // Nextcloud's extras: only where they
    // apply.
    // The links are stored sealed: `links` has them opened.
    let extras = &props.extras;
    if bridge && extras.publishable {
        let url = links
            .publish_url
            .as_ref()
            .map(|url| url.as_str().to_owned());
        if wants(CalendarProperty::IsPublished) {
            out.is_published = Some(url.is_some());
        }
        if wants(CalendarProperty::PublicUrl) {
            out.public_url = Some(url);
        }
    }
    if bridge && extras.is_subscription && wants(CalendarProperty::SubscriptionUrl) {
        out.subscription_url = Some(
            links
                .subscription_source
                .as_ref()
                .map(|url| url.as_str().to_owned()),
        );
    }
    Ok(out)
}

/// The calendars of the call's account (the user's own, or those someone
/// shares in their account) that are on the server.
pub(crate) async fn calendars(ctx: &Ctx<'_>) -> Result<Vec<Collection>, CallError> {
    Ok(ctx
        .store()
        .dav_collections(ctx.id())
        .await?
        .into_iter()
        .filter(|c| {
            c.props.kind == CollectionKind::Calendar
                && c.orphaned_at.is_none()
                && ctx.scope.contains(c)
        })
        .collect())
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<CalendarId, CalendarProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = super::account(ctx)?;
    let state = ctx.state(DataType::Calendar).await?;
    let all = calendars(ctx).await?;
    let properties = args.properties.as_deref();
    let wants_shares = properties.is_none_or(|list| list.contains(&CalendarProperty::ShareWith));
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    let one = async |calendar: &Collection| -> Result<Calendar, CallError> {
        let links = dav.calendar_links(calendar);
        let mut out = to_jmap(calendar, properties, ctx.uses_bridge, &links)?;
        if wants_shares && calendar.props.sharing.mode.is_some() {
            out.share_with = Some(
                crate::sharing::share_with::share_with(ctx, calendar, |bits| {
                    crate::sharing::share_with::calendar_rights(
                        bits,
                        bits.contains(Rights::WRITE_ACL),
                    )
                })
                .await,
            );
        }
        Ok(out)
    };
    match &args.ids {
        None => {
            for calendar in &all {
                list.push(one(calendar).await?);
            }
        }
        Some(wanted) => {
            let mut seen: Vec<CalendarId> = Vec::new();
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id.clone());
                let found =
                    ids::store_calendar(&id).and_then(|row| all.iter().find(|c| c.id == row));
                match found {
                    Some(calendar) => list.push(one(calendar).await?),
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::CalendarGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    super::account(ctx)?;
    let response = changes::changes(ctx, DataType::Calendar, args, |row| {
        CollectionId::from_raw(row)
            .ok_or(Bug("calendar row"))
            .and_then(ids::calendar_id)
    })
    .await?;
    let response = crate::sharing::scope::collection_changes(
        ctx,
        CollectionKind::Calendar,
        response,
        ids::store_calendar,
    )
    .await?;
    Ok(MethodResponse::CalendarChanges(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_from_privileges() {
        let read_only = rights(Some(Rights::READ));
        assert!(read_only.may_read_items && !read_only.may_write_all && !read_only.may_delete);
        let owner = rights(Some(
            Rights::READ
                .union(Rights::WRITE_CONTENT)
                .union(Rights::UNBIND),
        ));
        assert!(owner.may_write_own && owner.may_rsvp && owner.may_delete && !owner.may_share);
        assert_eq!(
            rights(None),
            CalendarRights {
                may_share: false,
                ..CalendarRights::ALL
            }
        );
    }
}
