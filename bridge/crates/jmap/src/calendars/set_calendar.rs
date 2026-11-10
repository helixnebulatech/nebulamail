// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Calendar/set` (draft-ietf-jmap-calendars §4.3) over CalDAV:
//!
//! - **create**: MKCALENDAR of `{home}/{uuid}/` with `displayname`,
//!   `calendar-description`, Apple's `calendar-color` and (if not 0)
//!   `calendar-order`, for events and tasks (`supported-calendar-component-set`
//!   VEVENT + VTODO), or what the bridge's vendor property
//!   `https://helixnebula.tech/jmap/bridge/components` asks for
//!   (`["VTODO"]`: a task list; only with that capability in `using`).
//!   It can't change later (CalDAV sets it once).
//! - **update**: `name`, `description`, `color` and `sortOrder` by
//!   PROPPATCH (all or nothing: a refused property is `forbidden`, nothing
//!   changed); `isSubscribed` and `isVisible` in the store (only the
//!   bridge keeps them).
//! - **destroy**: DELETE of the collection; with events in it only with
//!   `onDestroyRemoveEvents` (else `calendarHasEvent`, as Stalwart).
//! - `shareWith` (whole or `shareWith/<id>`) in an update: shared the way
//!   the server lets the user (`crate::sharing`). Also when creating:
//!   checked before the MKCALENDAR, set right after; if the server
//!   refuses, the calendar is deleted again (all or nothing).
//! - `defaultAlertsWithTime` / `defaultAlertsWithoutTime`: CalDAV's
//!   `default-alarm-vevent-datetime` / `-date` (draft-daboo-valarm-
//!   extensions; dead properties on Stalwart, Radicale and sabre/dav), only
//!   `OffsetTrigger` alerts shown or mailed, as Stalwart
//!   (`nebula_groupware::model::defaults`); whole or `…/<id>` patches.
//! - `timeZone`: `calendar-timezone` (a `VTIMEZONE`) and
//!   `calendar-timezone-id`; `null` removes both. Only zones calcard
//!   knows (`invalidProperties` else).
//! - Nothing is created in someone else's shared account (`forbidden`).
//! - `onSuccessSetIsDefault`: in the store, once everything worked.
//! - Nextcloud's extras (bridge capability in `using`):
//!   `…/isPublished` publishes or stops publishing a
//!   calendar the user may publish; `…/subscriptionUrl` in a create
//!   subscribes (an extended MKCOL with `CS:source`: the server fetches
//!   it, never the bridge) instead of creating an empty calendar.
//!   Destroying a subscription unsubscribes (its events are the
//!   server's copy: no `onDestroyRemoveEvents` needed).

use std::collections::BTreeMap;

use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{
    CalendarColor, Component, ComponentSet, ExternalUrl, Property, ResourceName,
};
use nebula_groupware::model::defaults::{DefaultAlerts, canonical_zone, vtimezone_calendar};
use nebula_groupware::{Contents, GroupwareAccount, GroupwareError};
use nebula_jmap_proto::method::{PatchObject, SetResponse};
use nebula_jmap_proto::object::{
    COMPONENTS_PROPERTY, Calendar, CalendarComponent, CalendarCreate, CalendarPatch,
    CalendarProperty, CalendarSetArguments, DefaultAlerts as DefaultAlertsMap,
    IS_PUBLISHED_PROPERTY, MapPatch, SUBSCRIPTION_URL_PROPERTY,
};
use nebula_jmap_proto::{CreationId, MethodResponse, SetError, SetErrorType, UnsignedInt};
use nebula_store::{Collection, CollectionId, CollectionKind, CollectionProps, DataType, Rights};
use serde_json::{Map, Value};

use super::calendar::{calendars, to_jmap};
use super::ids;
use super::set_error::{forbidden, set_error};
use crate::contacts::convert::uuid;
use crate::core::context::Ctx;
use crate::error::{CallError, store_set_error};
use crate::sharing::scope::{default_target, has_contents};

const fn alerts_property(with_time: bool) -> &'static str {
    if with_time {
        "defaultAlertsWithTime"
    } else {
        "defaultAlertsWithoutTime"
    }
}

/// A JMAP `Id[Alert]` the bridge can keep as CalDAV default alarms.
fn default_alerts(with_time: bool, map: &DefaultAlertsMap) -> Result<DefaultAlerts, SetError> {
    let object: Map<String, Value> = map
        .iter()
        .map(|(id, alert)| (id.clone(), Value::Object(alert.clone())))
        .collect();
    DefaultAlerts::from_jmap(&object).map_err(|error| {
        SetError::invalid_properties([alerts_property(with_time)])
            .with_description(error.to_string())
    })
}

/// `default-alarm-vevent-datetime` (with a time) or `-date`.
fn alarm_value(with_time: bool, text: String) -> PropValue {
    if with_time {
        PropValue::DefaultAlarmVeventDatetime(text)
    } else {
        PropValue::DefaultAlarmVeventDate(text)
    }
}

/// `calendar-timezone` (a `VTIMEZONE`, RFC 4791) and
/// `calendar-timezone-id` (RFC 7809), in that order: Stalwart keeps the
/// one set last, and the id is the better of the two.
fn zone_values(zone: &str) -> Result<Vec<PropValue>, SetError> {
    let unknown =
        || SetError::invalid_properties(["timeZone"]).with_description("unknown time zone");
    let name = canonical_zone(zone).ok_or_else(unknown)?;
    let calendar = vtimezone_calendar(&name).ok_or_else(unknown)?;
    Ok(vec![
        PropValue::CalendarTimezone(calendar),
        PropValue::CalendarTimezoneId(name),
    ])
}

/// Properties MKCALENDAR may drop without saying so: sabre/dav (Baïkal,
/// Nextcloud) answers 201 and keeps no dead property it was given, yet
/// keeps them from a PROPPATCH. They go in a PROPPATCH right after.
const fn is_extension(value: &PropValue) -> bool {
    matches!(
        value,
        PropValue::CalendarTimezoneId(_)
            | PropValue::DefaultAlarmVeventDatetime(_)
            | PropValue::DefaultAlarmVeventDate(_)
    )
}

/// The default alerts after a `defaultAlerts*` patch.
fn patched_alerts(
    with_time: bool,
    current: &DefaultAlerts,
    patch: &MapPatch<String, Map<String, Value>>,
) -> Result<DefaultAlerts, SetError> {
    let invalid = |error: nebula_groupware::model::defaults::DefaultAlertError| {
        SetError::invalid_properties([alerts_property(with_time)])
            .with_description(error.to_string())
    };
    match patch {
        MapPatch::Replace(None) => Ok(DefaultAlerts::new()),
        MapPatch::Replace(Some(map)) => default_alerts(with_time, map),
        MapPatch::Change { set, remove } => {
            let mut out = current.clone();
            for id in remove {
                out.patch(id, None).map_err(invalid)?;
            }
            for (id, alert) in set {
                out.patch(id, Some(&Value::Object(alert.clone())))
                    .map_err(invalid)?;
            }
            Ok(out)
        }
        _ => Err(SetError::invalid_properties([alerts_property(with_time)])),
    }
}

fn order(sort_order: UnsignedInt) -> Result<i32, SetError> {
    i32::try_from(sort_order.get()).map_err(|_| {
        SetError::invalid_properties(["sortOrder"]).with_description("sortOrder is too large")
    })
}

fn color(text: &str) -> Result<CalendarColor, SetError> {
    CalendarColor::parse(text).ok_or_else(|| {
        SetError::invalid_properties(["color"])
            .with_description("the color must be #RRGGBB or #RRGGBBAA here")
    })
}

const fn allows(calendar: &Collection, right: Rights) -> bool {
    match calendar.props.rights {
        Some(rights) => rights.contains(right),
        None => true,
    }
}

/// What a new calendar takes: the vendor property (only with the bridge's
/// capability in `using`), else events and tasks.
fn new_components(create: &CalendarCreate, bridge: bool) -> Result<ComponentSet, SetError> {
    let wanted = match &create.components {
        Some(_) if !bridge => {
            return Err(
                SetError::invalid_properties([COMPONENTS_PROPERTY]).with_description(
                    "this property needs the https://helixnebula.tech/jmap/bridge capability",
                ),
            );
        }
        Some(list) => list.clone(),
        None => vec![CalendarComponent::Event, CalendarComponent::Todo],
    };
    let mut out = Vec::new();
    for component in wanted {
        out.push(match component {
            CalendarComponent::Event => Component::Event,
            CalendarComponent::Todo => Component::Todo,
            _ => return Err(SetError::invalid_properties([COMPONENTS_PROPERTY])),
        });
    }
    Ok(ComponentSet::new(&out))
}

/// The DAV properties of a new calendar (`bridge`: as in
/// [`new_components`]).
fn create_values(create: &CalendarCreate, bridge: bool) -> Result<Vec<PropValue>, SetError> {
    let mut values = vec![
        PropValue::DisplayName(create.name.clone()),
        PropValue::SupportedComponents(new_components(create, bridge)?),
    ];
    if let Some(description) = &create.description {
        values.push(PropValue::CalendarDescription(description.clone()));
    }
    if let Some(text) = &create.color {
        values.push(PropValue::CalendarColor(color(text)?));
    }
    if create.sort_order != UnsignedInt::ZERO {
        values.push(PropValue::CalendarOrder(order(create.sort_order)?));
    }
    for (with_time, alerts) in [
        (true, &create.default_alerts_with_time),
        (false, &create.default_alerts_without_time),
    ] {
        if let Some(map) = alerts.as_ref().filter(|map| !map.is_empty()) {
            let alerts = default_alerts(with_time, map)?;
            values.push(alarm_value(with_time, alerts.to_valarms()));
        }
    }
    if let Some(zone) = &create.time_zone {
        values.extend(zone_values(zone.as_str())?);
    }
    Ok(values)
}

/// The bridge's vendor properties need its capability in `using`.
fn needs_bridge(ctx: &Ctx<'_>, property: &'static str) -> Result<(), SetError> {
    if ctx.uses_bridge {
        Ok(())
    } else {
        Err(SetError::invalid_properties([property]).with_description(
            "this property needs the https://helixnebula.tech/jmap/bridge capability",
        ))
    }
}

/// A subscription: only a name, a
/// colour and the URL, checked for shape only (`https`, `webcal(s)`, a
/// host, no credentials): the DAV server fetches it, never the bridge.
async fn subscribe_one(
    dav: &GroupwareAccount,
    create: &CalendarCreate,
    url: &str,
) -> Result<Collection, SetError> {
    let unsupported = create.components.is_some()
        || create.is_published.is_some()
        || create
            .share_with
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || create.time_zone.is_some()
        || create
            .default_alerts_with_time
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || create
            .default_alerts_without_time
            .as_ref()
            .is_some_and(|map| !map.is_empty());
    if unsupported {
        return Err(
            SetError::invalid_properties([SUBSCRIPTION_URL_PROPERTY]).with_description(
                "a subscription only takes a name, a description, a color and its URL",
            ),
        );
    }
    if !dav.nextcloud_support().subscriptions {
        return Err(forbidden(
            "the calendar server does not offer subscriptions",
        ));
    }
    let source = ExternalUrl::subscription(url).ok_or_else(|| {
        SetError::invalid_properties([SUBSCRIPTION_URL_PROPERTY])
            .with_description("must be an https, webcal or webcals URL with a host and no password")
    })?;
    let mut values = vec![PropValue::DisplayName(create.name.clone())];
    if let Some(description) = &create.description {
        values.push(PropValue::CalendarDescription(description.clone()));
    }
    if let Some(text) = &create.color {
        values.push(PropValue::CalendarColor(color(text)?));
    }
    let name = ResourceName::try_from(uuid()?.as_str()).map_err(|_| forbidden("name"))?;
    dav.subscribe(&name, &source, &values)
        .await
        .map_err(|e| set_error(&e))
}

/// Creates the calendar on the server (a subscription, or MKCALENDAR with
/// its defaults, sharing and publishing, all or nothing).
async fn make_calendar(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    create: &CalendarCreate,
) -> Result<Collection, SetError> {
    if let Some(url) = &create.subscription_url {
        needs_bridge(ctx, SUBSCRIPTION_URL_PROPERTY)?;
        return subscribe_one(dav, create, url).await;
    }
    if create.is_published.is_some() {
        needs_bridge(ctx, IS_PUBLISHED_PROPERTY)?;
    }
    let (values, later): (Vec<PropValue>, Vec<PropValue>) = create_values(create, ctx.uses_bridge)?
        .into_iter()
        .partition(|value| !is_extension(value));
    let share_with = create.share_with.clone().unwrap_or_default();
    crate::sharing::share_with::check_new(ctx, &share_with).await?;
    let name = ResourceName::try_from(uuid()?.as_str()).map_err(|_| forbidden("name"))?;
    let mut calendar = dav
        .create_collection(CollectionKind::Calendar, &name, &values)
        .await
        .map_err(|e| set_error(&e))?;
    // All or nothing: the calendar goes again if its defaults or its
    // sharing don't work.
    let mut finished = match dav.update_collection(calendar.id, &later, &[]).await {
        Ok(()) => {
            crate::sharing::share_with::share_new(
                ctx,
                calendar.id,
                &share_with,
                crate::sharing::share_with::calendar_bits,
            )
            .await
        }
        Err(error) => Err(set_error(&error)),
    };
    let publish = create.is_published == Some(true);
    if finished.is_ok() && publish {
        finished = match fresh_calendar(ctx, calendar.id).await {
            Some(fresh) => dav.set_published(&fresh, true).await.map_err(|e| {
                if matches!(e, GroupwareError::Unsupported) {
                    SetError::invalid_properties([IS_PUBLISHED_PROPERTY])
                        .with_description("this calendar can't be published")
                } else {
                    set_error(&e)
                }
            }),
            None => Err(SetError::not_found()),
        };
    }
    if let Err(error) = finished {
        if let Err(cleanup) = dav.delete_collection(calendar.id, Contents::Remove).await {
            tracing::warn!(error = %cleanup, "calendars: a half-made calendar stays");
        }
        return Err(error);
    }
    if (!share_with.is_empty() || publish)
        && let Some(fresh) = fresh_calendar(ctx, calendar.id).await
    {
        calendar = fresh;
    }
    Ok(calendar)
}

/// The calendar as the store has it now.
async fn fresh_calendar(ctx: &Ctx<'_>, id: CollectionId) -> Option<Collection> {
    calendars(ctx)
        .await
        .ok()
        .and_then(|all| all.into_iter().find(|c| c.id == id))
}

async fn create_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    object: Map<String, Value>,
) -> Result<(CollectionId, Calendar), SetError> {
    let create = CalendarCreate::parse(object)?;
    if ctx.scope.is_shared() {
        return Err(forbidden(
            "calendars can't be created in someone else's account",
        ));
    }
    let mut calendar = make_calendar(ctx, dav, &create).await?;
    if create.is_subscribed == Some(false) {
        ctx.store()
            .set_collection_subscribed(ctx.id(), calendar.id, false)
            .await
            .map_err(|e| store_set_error(&e))?;
        calendar.is_subscribed = false;
    }
    if create.is_visible == Some(false) {
        ctx.store()
            .set_collection_visible(ctx.id(), calendar.id, false)
            .await
            .map_err(|e| store_set_error(&e))?;
        calendar.is_visible = false;
    }
    let object = to_jmap(
        &calendar,
        Some(&[
            CalendarProperty::IsDefault,
            CalendarProperty::IsSubscribed,
            CalendarProperty::IsVisible,
            CalendarProperty::MyRights,
            CalendarProperty::ShareWith,
            CalendarProperty::IncludeInAvailability,
            CalendarProperty::Components,
            CalendarProperty::IsPublished,
            CalendarProperty::PublicUrl,
            CalendarProperty::SubscriptionUrl,
        ]),
        ctx.uses_bridge,
        &dav.calendar_links(&calendar),
    )
    .map_err(|_| forbidden("id"))?;
    let mut object = object;
    if calendar.props.sharing.mode.is_some() {
        object.share_with = Some(
            crate::sharing::share_with::share_with(ctx, &calendar, |bits| {
                crate::sharing::share_with::calendar_rights(bits, bits.contains(Rights::WRITE_ACL))
            })
            .await,
        );
    }
    Ok((calendar.id, object))
}

/// The PROPPATCH of an update: what to set and what to remove.
fn patch_values(
    patch: &CalendarPatch,
    current: &CollectionProps,
) -> Result<(Vec<PropValue>, Vec<Property>), SetError> {
    let current_order = current.sort_order;
    let mut set = Vec::new();
    let mut remove = Vec::new();
    if let Some(name) = &patch.name {
        set.push(PropValue::DisplayName(name.clone()));
    }
    match &patch.description {
        Some(Some(description)) => set.push(PropValue::CalendarDescription(description.clone())),
        Some(None) => remove.push(Property::CalendarDescription),
        None => {}
    }
    match &patch.color {
        Some(Some(text)) => set.push(PropValue::CalendarColor(color(text)?)),
        Some(None) => remove.push(Property::CalendarColor),
        None => {}
    }
    if let Some(sort_order) = patch.sort_order {
        let wanted = order(sort_order)?;
        if current_order.unwrap_or(0) != i64::from(wanted) {
            set.push(PropValue::CalendarOrder(wanted));
        }
    }
    for (with_time, alerts_patch, stored) in [
        (
            true,
            &patch.default_alerts_with_time,
            &current.default_alarms_datetime,
        ),
        (
            false,
            &patch.default_alerts_without_time,
            &current.default_alarms_date,
        ),
    ] {
        let Some(alerts_patch) = alerts_patch else {
            continue;
        };
        let before = stored
            .as_deref()
            .map(DefaultAlerts::from_valarms)
            .unwrap_or_default();
        let after = patched_alerts(with_time, &before, alerts_patch)?;
        if after != before {
            // An empty value, not a removal: "no default alarms" (the
            // draft), never the home's.
            set.push(alarm_value(with_time, after.to_valarms()));
        }
    }
    match &patch.time_zone {
        Some(Some(zone)) => {
            let values = zone_values(zone.as_str())?;
            let unchanged = canonical_zone(zone.as_str()) == current.time_zone;
            if !unchanged {
                set.extend(values);
            }
        }
        Some(None) if current.time_zone.is_some() => {
            remove.extend([Property::CalendarTimezone, Property::CalendarTimezoneId]);
        }
        _ => {}
    }
    Ok((set, remove))
}

async fn update_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    all: &[Collection],
    id: CollectionId,
    patch: &PatchObject,
) -> Result<(), SetError> {
    let patch = CalendarPatch::parse(patch)?;
    let calendar = all
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(SetError::not_found)?;
    let (set, remove) = patch_values(&patch, &calendar.props)?;
    if !(set.is_empty() && remove.is_empty()) {
        if !allows(calendar, Rights::WRITE_PROPERTIES) {
            return Err(forbidden("this calendar can't be changed"));
        }
        dav.update_collection(id, &set, &remove)
            .await
            .map_err(|e| set_error(&e))?;
    }
    if let Some(subscribed) = patch.is_subscribed {
        ctx.store()
            .set_collection_subscribed(ctx.id(), id, subscribed)
            .await
            .map_err(|e| store_set_error(&e))?;
    }
    if let Some(visible) = patch.is_visible {
        ctx.store()
            .set_collection_visible(ctx.id(), id, visible)
            .await
            .map_err(|e| store_set_error(&e))?;
    }
    if let Some(share) = &patch.share_with {
        crate::sharing::share_with::update(
            ctx,
            calendar,
            share,
            crate::sharing::share_with::calendar_bits,
        )
        .await?;
    }
    if let Some(publish) = patch.is_published {
        needs_bridge(ctx, IS_PUBLISHED_PROPERTY)?;
        if !calendar.props.extras.publishable {
            return Err(forbidden("this calendar can't be published"));
        }
        if calendar.props.extras.publish_url.is_some() != publish {
            dav.set_published(calendar, publish)
                .await
                .map_err(|e| set_error(&e))?;
        }
    }
    Ok(())
}

async fn destroy_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    all: &[Collection],
    id: CollectionId,
    remove_events: bool,
) -> Result<(), SetError> {
    let Some(calendar) = all.iter().find(|c| c.id == id) else {
        return Err(SetError::not_found());
    };
    // Unsubscribing: its events are the server's copy of someone else's.
    if calendar.props.extras.is_subscription {
        return dav
            .delete_collection(id, Contents::Remove)
            .await
            .map_err(|e| set_error(&e));
    }
    // Quick check on the local copy (an event that doesn't parse counts);
    // then the server is asked, as the copy may miss events.
    let contents = if remove_events {
        Contents::Remove
    } else {
        let stored = ctx
            .store()
            .dav_resources(ctx.id(), id)
            .await
            .map_err(|e| store_set_error(&e))?;
        if has_contents(&stored) {
            return Err(SetError::new(SetErrorType::CalendarHasEvent));
        }
        Contents::MustBeEmpty
    };
    dav.delete_collection(id, contents)
        .await
        .map_err(|e| match e {
            GroupwareError::HasContents => SetError::new(SetErrorType::CalendarHasEvent),
            other => set_error(&other),
        })
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: CalendarSetArguments,
) -> Result<MethodResponse, CallError> {
    let set = args.set;
    ctx.check_account(&set.account_id)?;
    let dav = super::account(ctx)?;
    let old_state = ctx
        .check_state(DataType::Calendar, set.if_in_state.as_ref())
        .await?;
    let mut created: BTreeMap<CreationId, Calendar> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in set.create {
        match create_one(ctx, &dav, object).await {
            Ok((id, object)) => {
                ctx.created
                    .insert(creation_id.clone(), ids::calendar_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let all = calendars(ctx).await?;
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &set.update {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_calendar) {
            Some(_) if set.destroy.iter().any(|d| ctx.resolve(d) == id) => {
                Err(SetError::new(SetErrorType::WillDestroy))
            }
            Some(row) => update_one(ctx, &dav, &all, row, patch).await,
            None => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => {
                updated.insert(id, None);
            }
            (Ok(()), None) => {}
            (Err(error), _) => {
                not_updated.insert(key.clone(), error);
            }
        }
    }
    let mut destroyed = Vec::new();
    let mut not_destroyed = BTreeMap::new();
    for key in &set.destroy {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_calendar) {
            Some(row) => destroy_one(ctx, &dav, &all, row, args.on_destroy_remove_events).await,
            None => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => destroyed.push(id),
            (Ok(()), None) => {}
            (Err(error), _) => {
                not_destroyed.insert(key.clone(), error);
            }
        }
    }
    let all_ok = not_created.is_empty() && not_updated.is_empty() && not_destroyed.is_empty();
    if all_ok
        && let Some(default) = &args.on_success_set_is_default
        && let Some(id) = ctx.resolve(default)
        && let Some(row) = ids::store_calendar(&id)
        && default_target(ctx, CollectionKind::Calendar, row).await?
    {
        // Only a live calendar of the user's own (not an address book,
        // not someone else's); anything else is ignored.
        match ctx
            .store()
            .set_default_collection(ctx.id(), row, CollectionKind::Calendar)
            .await
        {
            Ok(_) => {
                if let Some(object) = created
                    .values_mut()
                    .find(|calendar| calendar.id.as_ref() == Some(&id))
                {
                    object.is_default = Some(true);
                } else {
                    updated.entry(id).or_insert(None);
                }
            }
            Err(error) => tracing::debug!(%error, "onSuccessSetIsDefault: not a live calendar"),
        }
    }
    let new_state = ctx.state(DataType::Calendar).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::CalendarSet(response))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn create(value: &Value) -> CalendarCreate {
        CalendarCreate::parse(value.as_object().unwrap().clone()).unwrap()
    }

    fn props(sort_order: Option<i64>) -> CollectionProps {
        CollectionProps {
            kind: CollectionKind::Calendar,
            href: nebula_store::DavHref::new("https://dav.example/cal/alice/work/").unwrap(),
            name: Some("Work".to_owned()),
            color: None,
            description: None,
            sort_order,
            time_zone: None,
            default_alarms_datetime: None,
            default_alarms_date: None,
            rights: None,
            components: None,
            vcard_versions: None,
            supports_sync: true,
            sharing: nebula_store::CollectionSharing::default(),
            extras: nebula_store::CalendarExtras::default(),
        }
    }

    /// Default alerts and the zone of a new calendar: CalDAV's
    /// `default-alarm-vevent-*`, `calendar-timezone` then
    /// `calendar-timezone-id`; anything but an `OffsetTrigger` or an
    /// unknown zone is refused.
    #[test]
    fn defaults_of_a_new_calendar() {
        let values = create_values(
            &create(&json!({
                "name": "X", "timeZone": "Europe/Madrid",
                "defaultAlertsWithTime": {"a": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT10M"}}},
                "defaultAlertsWithoutTime": {}
            })),
            false,
        )
        .unwrap();
        let PropValue::DefaultAlarmVeventDatetime(text) = &values[2] else {
            panic!("{values:?}");
        };
        assert!(text.contains("UID:a\r\n") && text.contains("TRIGGER:-PT10M\r\n"));
        assert!(
            matches!(&values[3], PropValue::CalendarTimezone(tz) if tz.contains("TZID:Europe/Madrid"))
        );
        assert_eq!(
            values[4],
            PropValue::CalendarTimezoneId("Europe/Madrid".into())
        );
        assert_eq!(values.len(), 5);
        let absolute = create(
            &json!({"name": "X", "defaultAlertsWithoutTime": {"a": {"trigger": {"@type": "AbsoluteTrigger", "when": "2026-01-01T00:00:00Z"}}}}),
        );
        assert_eq!(
            create_values(&absolute, false).unwrap_err().properties(),
            ["defaultAlertsWithoutTime"]
        );
        let zone = create(&json!({"name": "X", "timeZone": "Mars/Olympus"}));
        assert_eq!(
            create_values(&zone, false).unwrap_err().properties(),
            ["timeZone"]
        );
    }

    /// Patches of the defaults: one alert, all of them, the zone (and
    /// nothing when it does not change).
    #[test]
    fn defaults_of_an_update() {
        let mut current = props(None);
        current.default_alarms_datetime = Some(
            DefaultAlerts::from_jmap(
                json!({"a": {"trigger": {"offset": "-PT10M"}}})
                    .as_object()
                    .unwrap(),
            )
            .unwrap()
            .to_valarms(),
        );
        current.time_zone = Some("Europe/Madrid".into());
        let (set, remove) = patch_values(
            &patch(json!({"defaultAlertsWithTime/b": {"trigger": {"offset": "PT0S"}, "action": "email"}})),
            &current,
        )
        .unwrap();
        let [PropValue::DefaultAlarmVeventDatetime(text)] = set.as_slice() else {
            panic!("{set:?}");
        };
        assert!(text.contains("UID:a\r\n") && text.contains("UID:b\r\n"));
        assert!(remove.is_empty());
        let (set, _) =
            patch_values(&patch(json!({"defaultAlertsWithTime/a": null})), &current).unwrap();
        assert_eq!(set, [PropValue::DefaultAlarmVeventDatetime(String::new())]);
        let (set, _) =
            patch_values(&patch(json!({"defaultAlertsWithoutTime": null})), &current).unwrap();
        assert!(set.is_empty());
        let (set, _) =
            patch_values(&patch(json!({"timeZone": "Europe/Madrid"})), &current).unwrap();
        assert!(set.is_empty());
        let (set, _) =
            patch_values(&patch(json!({"timeZone": "America/New_York"})), &current).unwrap();
        assert_eq!(set.len(), 2);
        let (set, remove) = patch_values(&patch(json!({"timeZone": null})), &current).unwrap();
        assert!(set.is_empty());
        assert_eq!(
            remove,
            [Property::CalendarTimezone, Property::CalendarTimezoneId]
        );
        assert_eq!(
            patch_values(
                &patch(json!({"defaultAlertsWithTime/x": {"trigger": {"offset": "1 hour"}}})),
                &current
            )
            .unwrap_err()
            .properties(),
            ["defaultAlertsWithTime"]
        );
    }

    fn patch(value: Value) -> CalendarPatch {
        CalendarPatch::parse(&serde_json::from_value(value).unwrap()).unwrap()
    }

    /// `createCalendar` of the web.
    #[test]
    fn dav_properties_of_a_new_calendar() {
        let values = create_values(
            &create(&json!({
                "name": "Trabajo", "color": "#3366cc", "isSubscribed": true, "isVisible": true
            })),
            false,
        )
        .unwrap();
        assert_eq!(
            values,
            [
                PropValue::DisplayName("Trabajo".to_owned()),
                PropValue::SupportedComponents(ComponentSet::new(&[
                    Component::Event,
                    Component::Todo
                ])),
                PropValue::CalendarColor(CalendarColor::parse("#3366CC").unwrap()),
            ]
        );
        // shareWith is not a DAV property: it is shared after the create.
        let shared = create(
            &json!({"name": "X", "shareWith": {"p1": {"mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": false, "mayWriteOwn": false, "mayUpdatePrivate": false, "mayRSVP": false, "mayShare": false, "mayDelete": false}}}),
        );
        assert_eq!(create_values(&shared, true).unwrap().len(), 2);
        let bad = create(&json!({"name": "X", "color": "red"}));
        assert_eq!(
            create_values(&bad, true).unwrap_err().properties(),
            ["color"]
        );
    }

    /// A task list from the web's Tasks app.
    #[test]
    fn a_task_list_takes_only_tasks() {
        let list = create(&json!({"name": "Recados", COMPONENTS_PROPERTY: ["VTODO"]}));
        assert_eq!(
            create_values(&list, true).unwrap()[1],
            PropValue::SupportedComponents(ComponentSet::new(&[Component::Todo]))
        );
        // Without the capability in `using`, the property is unknown.
        assert_eq!(
            create_values(&list, false).unwrap_err().properties(),
            [COMPONENTS_PROPERTY]
        );
    }

    /// `updateCalendar` of the web: name and color by PROPPATCH,
    /// `isVisible` not (it goes to the store).
    #[test]
    fn dav_properties_of_an_update() {
        let (set, remove) = patch_values(
            &patch(json!({"name": "Casa", "color": "#ff0000", "isVisible": false})),
            &props(None),
        )
        .unwrap();
        assert_eq!(
            set,
            [
                PropValue::DisplayName("Casa".to_owned()),
                PropValue::CalendarColor(CalendarColor::parse("#FF0000").unwrap()),
            ]
        );
        assert!(remove.is_empty());
        let (set, remove) = patch_values(
            &patch(json!({"description": null, "color": null})),
            &props(Some(0)),
        )
        .unwrap();
        assert!(set.is_empty());
        assert_eq!(
            remove,
            [Property::CalendarDescription, Property::CalendarColor]
        );
        let (set, _) = patch_values(&patch(json!({"isVisible": true})), &props(None)).unwrap();
        assert!(set.is_empty());
        // shareWith is not a DAV property: it goes to sharing.
        let (set, remove) = patch_values(&patch(json!({"shareWith/p1": {"mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": false, "mayWriteOwn": false, "mayUpdatePrivate": false, "mayRSVP": false, "mayShare": false, "mayDelete": false}})), &props(None)).unwrap();
        assert!(set.is_empty() && remove.is_empty());
        assert!(patch_values(&patch(json!({"shareWith": null})), &props(None)).is_ok());
    }
}
