// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/set` (draft-ietf-jmap-calendars §5.6) over CalDAV, as
//! Stalwart 0.16 answers the web (`apps/web/src/calendar/api.ts`):
//!
//! - **create**: the JSCalendar event (a `uid` if it has none, returned)
//!   as iCalendar, PUT as `{calendar}/{uuid}.ics` with `If-None-Match: *`.
//! - **update** of the stored event: stored iCalendar → JSCalendar
//!   (calcard keeps what it does not convert) → the patch → iCalendar, PUT
//!   with `If-Match`. A 412 reads it again and applies the same patch once
//!   more; a second one is `forbidden` ("it changed meanwhile"). A new
//!   `calendarIds` is a MOVE. A patch that changes nothing writes nothing.
//! - **update by occurrence id** (`v{n}_{date}`): the patch goes into the
//!   series' `recurrenceOverrides` under that occurrence's original start
//!   (`RECURRENCE-ID`; `VALUE=DATE` for all-day series); for an event
//!   without repetition the id names the event itself (Stalwart).
//! - **destroy**: DELETE with `If-Match`; by occurrence id, `excluded:
//!   true` in the series (`EXDATE`).
//!
//! **Invitations** (`sendSchedulingMessages`,
//! false by default as in Stalwart):
//!
//! - Server that schedules (`calendar-auto-schedule`, `scheduling =
//!   "auto"`, or `"server"`): the write goes as is and the server sends
//!   the REQUEST/CANCEL. `SCHEDULE-AGENT=CLIENT` (RFC 6638 §7.1) where the
//!   server must not send: on the attendees when the organizer writes
//!   without `sendSchedulingMessages` or changes one occurrence (the
//!   bridge mails that REQUEST itself, as below, since Stalwart sends
//!   nothing for it), on the organizer when the user is an attendee (the
//!   web mails its own REPLY; COUNTER goes by `counter.rs`). A delete that
//!   must not notify first writes the mark.
//! - Otherwise, and for a change to one occurrence always, the bridge
//!   sends, as the organizer, with `sendSchedulingMessages`: REQUEST on
//!   create and on changes (only the changed occurrence when only it
//!   changed), CANCEL to removed attendees, on delete and for a deleted
//!   occurrence; nothing for events that already ended (as Stalwart). Attendees are always marked
//!   `SCHEDULE-AGENT=CLIENT`, so a scheduling server reached later does not
//!   send them again. At most [`MAX_RECIPIENTS`](super::set_itip::MAX_RECIPIENTS) recipients.
//!
//! **Tasks** (`@type: "Task"`, a `VTODO`) go the
//! same way (create, patch, occurrences by synthetic id, move, destroy).
//! **Assigned tasks** (participants)
//!  are scheduled like events: with `sendSchedulingMessages`
//! the organizer's REQUEST (assign, update) and CANCEL (take back, delete)
//! go out by the server or the bridge with the same rules, except that a
//! task past its due date still notifies (it is still to be done). The
//! assignee's REPLY is sent by the web, as for events. Completing writes
//! `COMPLETED` and `PERCENT-COMPLETE:100` (`nebula_groupware::model`).
//!
//! Each write is in the store when the response goes out (new etag, new
//! state) and kicks a sync round.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nebula_groupware::model::calendar::{ItemKind, ParsedCalendar, RecurrenceKey};
use nebula_groupware::model::convert::icalendar_index;
use nebula_groupware::model::defaults::DefaultAlerts;
use nebula_groupware::model::edit::{
    EditableEvent, OccurrenceTarget, new_icalendar, occurrence_target,
};
use nebula_groupware::model::itip::{
    self, Agent, ItipMessage, ItipPlan, Role, organizer_messages, set_schedule_agent,
};
use nebula_groupware::{ConvertError, GroupwareAccount, GroupwareError, ResourceText};
use nebula_jmap_proto::method::{PatchObject, SetResponse};
use nebula_jmap_proto::object::js_body::{BodyPatch, JsBody, JsCalendar};
use nebula_jmap_proto::object::{
    CalendarEvent, CalendarEventCreate, CalendarEventPatch, CalendarEventSetArguments,
};
use nebula_jmap_proto::{
    CalendarEventId, CalendarId, CreationId, IdOrRef, MethodResponse, SetError, SetErrorType,
};
use nebula_store::{
    Collection, CollectionId, CollectionKind, DataType, Identity, ResourceId, Rights,
    StoredResource,
};
use serde_json::{Map, Value};

use super::calendar::calendars;
use super::ids::{self, EventRef};
use super::set_error::{convert_error, forbidden, set_error};
use super::set_itip::{Occasion, check_outgoing, send_all, sender};
use super::set_patch::{exclude_occurrence, patch_occurrence, patch_series};
use crate::contacts::convert::uuid;
use crate::core::context::Ctx;
use crate::error::CallError;

/// Who schedules, for this request.
struct Scheduling {
    /// `sendSchedulingMessages`.
    notify: bool,
    /// The DAV server sends invitations (else the bridge does).
    server: bool,
    /// The user's calendar addresses.
    own: Arc<Vec<String>>,
}

/// What kind of write, for the scheduling rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    Create,
    /// A change to the stored event (or the deletion of one occurrence).
    Series,
    /// A change to one occurrence: the bridge mails that REQUEST itself,
    /// whoever schedules the rest.
    Occurrence,
}

/// A text ready to write and the invitations to send after.
struct Outgoing {
    text: String,
    messages: Vec<ItipMessage>,
    sender: Option<Identity>,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, SetError> + Send + 'static,
) -> Result<T, SetError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| forbidden("the event could not be converted"))?
}

fn now_secs() -> Result<i64, SetError> {
    Ctx::now()
        .map(nebula_store::UnixTime::secs)
        .map_err(|_| forbidden("clock"))
}

const fn allows(calendar: &Collection, right: Rights) -> bool {
    match calendar.props.rights {
        Some(rights) => rights.contains(right),
        None => true,
    }
}

/// Whether the event's last occurrence ended before `now` (open-ended
/// and unknown: no).
fn has_ended(text: &str, now: i64) -> bool {
    icalendar_index(text.as_bytes(), 1000)
        .ok()
        .and_then(|index| index.utc_end)
        .is_some_and(|end| end < now)
}

/// Whether the object is a task (`VTODO`).
fn is_task(text: &str) -> bool {
    ParsedCalendar::parse(text.as_bytes())
        .ok()
        .and_then(|parsed| parsed.kind())
        == Some(ItemKind::Task)
}

/// What an organizer's write does: the mark on the attendees and whether
/// the bridge sends the messages. A server that schedules sends them
/// (`SCHEDULE-AGENT=CLIENT` without `sendSchedulingMessages`), except for
/// a change to one occurrence: Stalwart sends nothing for it, so the
/// bridge does, and marks the attendees so the server never sends twice.
/// Without such a server the bridge always sends (marked, for one that
/// comes later).
fn organizer_plan(server: bool, notify: bool, write: Write) -> (Agent, bool) {
    if server && write != Write::Occurrence {
        (if notify { Agent::Server } else { Agent::Client }, false)
    } else {
        (Agent::Client, notify)
    }
}

/// `plan`, if the bridge may send it ([`check_outgoing`]: at most
/// [`MAX_RECIPIENTS`](super::set_itip::MAX_RECIPIENTS) recipients and
/// `MAX_EMAILS` emails for everything it mails: invitations, updates and
/// the cancellations of a deletion). Saving without notifying has no
/// limit: large events come from the server or other clients and stay
/// editable.
fn sendable(plan: ItipPlan, too_many: impl FnOnce() -> SetError) -> Result<ItipPlan, SetError> {
    check_outgoing(&plan.messages).map_err(|_| too_many())?;
    Ok(plan)
}

/// A deletion that would cancel for too many recipients. The web offers
/// to destroy without `sendSchedulingMessages` then.
fn too_many_to_cancel() -> SetError {
    SetError::new(SetErrorType::Forbidden)
        .with_description("too many participants to notify; destroy without sendSchedulingMessages")
}

/// The scheduling marks and, when the bridge sends, the messages for a
/// write from `old` (`None`: new) to `new`. Events and tasks alike; a
/// task does not "end" (an overdue task is still assigned).
async fn schedule(
    ctx: &Ctx<'_>,
    sched: &Scheduling,
    old: Option<String>,
    new: String,
    write: Write,
) -> Result<Outgoing, SetError> {
    let own = Arc::clone(&sched.own);
    let (notify, server) = (sched.notify, sched.server);
    let now = now_secs()?;
    let (text, messages) = blocking(move || {
        let ended = !is_task(&new) && has_ended(&new, now);
        let role = itip::role(new.as_bytes(), &own).map_err(|e| convert_error(&e))?;
        let mark = |text: String, role: Role, agent: Agent| -> Result<String, SetError> {
            Ok(set_schedule_agent(text.as_bytes(), role, agent)
                .map_err(|e| convert_error(&e))?
                .unwrap_or(text))
        };
        match role {
            Role::Organizer => {
                let (agent, bridge_sends) = organizer_plan(server, notify, write);
                if !bridge_sends {
                    return Ok((mark(new, role, agent)?, Vec::new()));
                }
                let send = !ended;
                let (text, messages) = if send {
                    let plan = organizer_messages(
                        old.as_deref().map(str::as_bytes),
                        Some(new.as_bytes()),
                        &own,
                        now,
                    )
                    .map_err(|e| convert_error(&e))?;
                    check_outgoing(&plan.messages)?;
                    (plan.text.unwrap_or(new), plan.messages)
                } else {
                    (new, Vec::new())
                };
                Ok((mark(text, role, Agent::Client)?, messages))
            }
            Role::Attendee => Ok((mark(new, role, Agent::Client)?, Vec::new())),
            _ => Ok((new, Vec::new())),
        }
    })
    .await?;
    let sender = match messages.first() {
        Some(first) => Some(sender(ctx, &first.organizer).await?),
        None => None,
    };
    Ok(Outgoing {
        text,
        messages,
        sender,
    })
}

async fn deliver(ctx: &Ctx<'_>, outgoing: &Outgoing, occasion: Occasion) {
    if let Some(from) = &outgoing.sender
        && !outgoing.messages.is_empty()
    {
        send_all(ctx, from, &outgoing.messages, occasion).await;
    }
}

/// The one calendar of `calendarIds`: an event is one CalDAV resource.
fn one_calendar<'a>(
    ctx: &Ctx<'_>,
    all: &'a [Collection],
    wanted: &BTreeSet<IdOrRef<CalendarId>>,
) -> Result<&'a Collection, SetError> {
    let mut iter = wanted.iter();
    let (Some(only), None) = (iter.next(), iter.next()) else {
        return Err(SetError::invalid_properties(["calendarIds"])
            .with_description("an event is in exactly one calendar here"));
    };
    ctx.resolve(only)
        .as_ref()
        .and_then(ids::store_calendar)
        .and_then(|row| all.iter().find(|c| c.id == row))
        .ok_or_else(|| {
            SetError::invalid_properties(["calendarIds"]).with_description("unknown calendar")
        })
}

/// A new event's body: `@type`, and its own `uid` (or a new one, which
/// the response returns).
fn new_body(body: JsBody<JsCalendar>) -> Result<(Map<String, Value>, Option<String>), SetError> {
    let mut members = body.into_map();
    members
        .entry("@type")
        .or_insert_with(|| Value::from("Event"));
    let generated = if members.contains_key("uid") {
        None
    } else {
        let uid = uuid()?;
        members.insert("uid".to_owned(), Value::from(uid.clone()));
        Some(uid)
    };
    Ok((members, generated))
}

/// `useDefaultAlerts: true` on a new event (draft-ietf-jmap-calendars:
/// "use the user's default alerts and ignore the value of the alerts
/// property"): the calendar's default alerts for events with or without a
/// time become the event's `alerts`, written into the iCalendar as
/// `VALARM`s (CalDAV has no such flag; the alerts then fire like any
/// other, and `/get` says `useDefaultAlerts: false`). Tasks keep theirs:
/// CalDAV's defaults are for `VEVENT`s only.
fn apply_default_alerts(members: &mut Map<String, Value>, calendar: &Collection) {
    if members.get("@type").and_then(Value::as_str) != Some("Event") {
        return;
    }
    let all_day = members
        .get("showWithoutTime")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stored = if all_day {
        calendar.props.default_alarms_date.as_deref()
    } else {
        calendar.props.default_alarms_datetime.as_deref()
    };
    let alerts = stored.map(DefaultAlerts::from_valarms).unwrap_or_default();
    if alerts.is_empty() {
        members.remove("alerts");
    } else {
        members.insert("alerts".to_owned(), Value::Object(alerts.to_jmap()));
    }
}

fn new_name() -> Result<nebula_dav::proto::ResourceName, SetError> {
    nebula_dav::proto::ResourceName::try_from(format!("{}.ics", uuid()?).as_str())
        .map_err(|_| forbidden("name"))
}

async fn create_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    sched: &Scheduling,
    all: &[Collection],
    object: Map<String, Value>,
) -> Result<(ResourceId, CalendarEvent), SetError> {
    let create = CalendarEventCreate::parse(object)?;
    let calendar = one_calendar(ctx, all, create.calendar_ids.as_set())?;
    if !allows(calendar, Rights::BIND) {
        return Err(forbidden("this calendar is read-only"));
    }
    let (mut members, generated) = new_body(create.body)?;
    if create.use_default_alerts == Some(true) {
        apply_default_alerts(&mut members, calendar);
    }
    let now = now_secs()?;
    let text =
        blocking(move || new_icalendar(&members, now).map_err(|e| convert_error(&e))).await?;
    let draft = Scheduling {
        notify: sched.notify && !create.is_draft,
        server: sched.server,
        own: Arc::clone(&sched.own),
    };
    let outgoing = schedule(ctx, &draft, None, text, Write::Create).await?;
    let stored = dav
        .create_resource(calendar.id, &new_name()?, outgoing.text.clone())
        .await
        .map_err(|e| set_error(&e))?;
    deliver(ctx, &outgoing, Occasion::Created).await;
    let mut created = CalendarEvent {
        id: Some(ids::event_id(stored.id).map_err(|_| forbidden("id"))?),
        ..CalendarEvent::default()
    };
    if let Some(uid) = generated {
        let mut body = Map::new();
        body.insert("uid".to_owned(), Value::from(uid));
        created.body = JsBody::new(body).map_err(|e| e.to_set_error())?;
    }
    Ok((stored.id, created))
}

/// The text of a stored event (read again from the server if the local
/// copy does not open).
pub(super) async fn event_text(
    dav: &GroupwareAccount,
    event: &StoredResource,
) -> Result<ResourceText, SetError> {
    crate::core::text::readable_text(dav, event)
        .await
        .map_err(|e| set_error(&e))?
        .ok_or_else(SetError::not_found)
}

/// What a change does to the event of a text.
#[derive(Clone)]
enum Change {
    /// The patch, on the stored event.
    Series(Arc<Vec<BodyPatch>>),
    /// The patch, on one occurrence.
    Occurrence(RecurrenceKey, Arc<Vec<BodyPatch>>),
    /// One occurrence excluded.
    Exclude(RecurrenceKey),
}

/// The new text after `change` (`None`: nothing changes). An occurrence
/// of an event without repetition is the event (Stalwart).
fn changed_text(text: &str, change: &Change, now: i64) -> Result<Option<String>, SetError> {
    let editable = EditableEvent::open(text.as_bytes()).map_err(|_| {
        SetError::new(SetErrorType::Forbidden).with_description("this event can't be edited here")
    })?;
    let target = |key: RecurrenceKey| -> Result<OccurrenceTarget, SetError> {
        occurrence_target(text.as_bytes(), key)
            .map_err(|e| convert_error(&e))?
            .ok_or_else(SetError::not_found)
    };
    let event = editable.event.clone();
    let patched = match change {
        Change::Series(patches) => patch_series(event, patches)?,
        Change::Occurrence(key, patches) => {
            let target = target(*key)?;
            if target.recurring {
                patch_occurrence(event, &target, patches)?
            } else {
                patch_series(event, patches)?
            }
        }
        Change::Exclude(key) => Some(exclude_occurrence(event, &target(*key)?)?),
    };
    let Some(patched) = patched else {
        return Ok(None);
    };
    let mut editable = editable;
    editable.event = patched;
    editable
        .to_icalendar(now)
        .map(Some)
        .map_err(|e| convert_error(&e))
}

/// What an occurrence id names in a stored event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Occurrence {
    /// The whole event (it doesn't repeat).
    Whole,
    /// One occurrence of a series.
    One(RecurrenceKey),
    /// Nothing: the event has no such occurrence.
    Missing,
}

/// What the occurrence `key` of `text` is. calcard work: only called from
/// a `blocking` closure (or through [`occurrence_kind`]).
///
/// # Errors
/// The event does not parse.
fn occurrence_of(text: &str, key: RecurrenceKey) -> Result<Occurrence, ConvertError> {
    Ok(match occurrence_target(text.as_bytes(), key)? {
        Some(target) if !target.recurring => Occurrence::Whole,
        Some(_) => Occurrence::One(key),
        None => Occurrence::Missing,
    })
}

/// [`occurrence_of`], off the async threads.
pub(super) async fn occurrence_kind(
    text: String,
    key: RecurrenceKey,
) -> Result<Result<Occurrence, ConvertError>, CallError> {
    super::blocking(move || occurrence_of(&text, key)).await
}

/// Writes `change` to `event`, retried once after a 412, on the
/// account's [write turn](GroupwareAccount::write_turn) (released before
/// the invitations go out). Returns the stored resource.
async fn write_change(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    sched: &Scheduling,
    event: StoredResource,
    change: Change,
) -> Result<StoredResource, SetError> {
    let turn = dav.write_turn().await;
    let mut current = event_text(dav, &event).await?;
    for attempt in 0..2 {
        let text = current.text.clone();
        let now = now_secs()?;
        let work = change.clone();
        let Some((new_text, kind)) = blocking(move || {
            let Some(new_text) = changed_text(&text, &work, now)? else {
                return Ok(None);
            };
            let kind = match &work {
                Change::Occurrence(key, _) => {
                    match occurrence_of(&text, *key).map_err(|e| convert_error(&e))? {
                        Occurrence::One(_) => Write::Occurrence,
                        Occurrence::Whole => Write::Series,
                        Occurrence::Missing => return Err(SetError::not_found()),
                    }
                }
                _ => Write::Series,
            };
            Ok(Some((new_text, kind)))
        })
        .await?
        else {
            return Ok(current.resource);
        };
        let outgoing = schedule(ctx, sched, Some(current.text.clone()), new_text, kind).await?;
        match dav
            .replace_resource(&current.resource, outgoing.text.clone())
            .await
        {
            Ok(stored) => {
                drop(turn);
                deliver(ctx, &outgoing, Occasion::Changed).await;
                return Ok(stored);
            }
            Err(error) if error.is_conflict() && attempt == 0 => {
                tracing::debug!("calendars: event changed meanwhile; applying the patch again");
                current = dav
                    .refetch(&current.resource)
                    .await
                    .map_err(|e| set_error(&e))?
                    .ok_or_else(SetError::not_found)?;
            }
            Err(error) => return Err(set_error(&error)),
        }
    }
    Err(set_error(&GroupwareError::Dav(
        nebula_dav::DavError::PreconditionFailed,
    )))
}

/// The calendar the event moves to, if `calendarIds` changes.
fn target_calendar<'a>(
    ctx: &Ctx<'_>,
    all: &'a [Collection],
    current: CollectionId,
    patch: &CalendarEventPatch,
) -> Result<Option<&'a Collection>, SetError> {
    let Some(change) = &patch.calendar_ids else {
        return Ok(None);
    };
    let here = ids::calendar_id(current).map_err(|_| forbidden("id"))?;
    let mut set = BTreeSet::from([IdOrRef::Id(here)]);
    change.apply(&mut set);
    let resolved: BTreeSet<IdOrRef<CalendarId>> = set
        .iter()
        .filter_map(|id| ctx.resolve(id))
        .map(IdOrRef::Id)
        .collect();
    let target = one_calendar(ctx, all, &resolved)?;
    Ok((target.id != current).then_some(target))
}

async fn move_event(
    dav: &GroupwareAccount,
    event: StoredResource,
    to: CollectionId,
) -> Result<StoredResource, SetError> {
    match dav.move_resource(&event, to).await {
        Ok(moved) => Ok(moved),
        Err(error) if error.is_conflict() => {
            let fresh = dav
                .refetch(&event)
                .await
                .map_err(|e| set_error(&e))?
                .ok_or_else(SetError::not_found)?;
            dav.move_resource(&fresh.resource, to)
                .await
                .map_err(|e| set_error(&e))
        }
        Err(error) => Err(set_error(&error)),
    }
}

async fn stored_event(
    ctx: &Ctx<'_>,
    all: &[Collection],
    id: ResourceId,
) -> Result<(StoredResource, Collection), SetError> {
    let event = ctx
        .store()
        .visible_resource(ctx.id(), CollectionKind::Calendar, id)
        .await
        .map_err(|_| forbidden("database error"))?
        .ok_or_else(SetError::not_found)?;
    let calendar = all
        .iter()
        .find(|c| c.id == event.collection)
        .cloned()
        .ok_or_else(SetError::not_found)?;
    Ok((event, calendar))
}

async fn update_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    sched: &Scheduling,
    all: &[Collection],
    which: EventRef,
    patch: &PatchObject,
) -> Result<(), SetError> {
    let patch = CalendarEventPatch::parse(patch)?;
    let (event, from) = stored_event(ctx, all, which.resource()).await?;
    let target = target_calendar(ctx, all, event.collection, &patch)?;
    let change = match which {
        EventRef::Base(_) => Change::Series(Arc::new(patch.body)),
        EventRef::Occurrence(_, key) => {
            if target.is_some() {
                return Err(SetError::invalid_properties(["calendarIds"])
                    .with_description("one occurrence can't move to another calendar"));
            }
            Change::Occurrence(key, Arc::new(patch.body))
        }
    };
    let has_body = match &change {
        Change::Series(p) | Change::Occurrence(_, p) => !p.is_empty(),
        Change::Exclude(_) => true,
    };
    if has_body && !allows(&from, Rights::WRITE_CONTENT) {
        return Err(forbidden("this calendar is read-only"));
    }
    if let Some(target) = target
        && (!allows(&from, Rights::UNBIND) || !allows(target, Rights::BIND))
    {
        return Err(forbidden(
            "the event can't be moved between these calendars",
        ));
    }
    let event = if has_body {
        write_change(ctx, dav, sched, event, change).await?
    } else {
        event
    };
    if let Some(target) = target {
        move_event(dav, event, target.id).await?;
    }
    Ok(())
}

/// Deletes a stored event: the mark first when a scheduling server must
/// not tell anyone, the invitations after when the bridge does.
async fn delete_event(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    sched: &Scheduling,
    event: StoredResource,
) -> Result<(), SetError> {
    let current = event_text(dav, &event).await?;
    let own = Arc::clone(&sched.own);
    let (notify, server) = (sched.notify, sched.server);
    let now = now_secs()?;
    let old = current.text.clone();
    let (marked, messages) = blocking(move || {
        let ended = !is_task(&old) && has_ended(&old, now);
        let role = itip::role(old.as_bytes(), &own).map_err(|e| convert_error(&e))?;
        match role {
            Role::Organizer | Role::Attendee if server => {
                let agent = if notify && role == Role::Organizer {
                    Agent::Server
                } else {
                    Agent::Client
                };
                let marked = set_schedule_agent(old.as_bytes(), role, agent)
                    .map_err(|e| convert_error(&e))?;
                Ok((marked, Vec::new()))
            }
            Role::Organizer if notify && !ended => {
                let plan = organizer_messages(Some(old.as_bytes()), None, &own, now)
                    .map_err(|e| convert_error(&e))?;
                // Refused before anything is deleted (the client may repeat
                // it without `sendSchedulingMessages`).
                let plan = sendable(plan, too_many_to_cancel)?;
                Ok((None, plan.messages))
            }
            _ => Ok((None, Vec::new())),
        }
    })
    .await?;
    let sender = match messages.first() {
        Some(first) => Some(sender(ctx, &first.organizer).await?),
        None => None,
    };
    let mut resource = current.resource;
    if let Some(marked) = marked {
        resource = dav
            .replace_resource(&resource, marked)
            .await
            .map_err(|e| set_error(&e))?;
    }
    let result = match dav.delete_resource(&resource).await {
        Ok(()) => Ok(()),
        Err(error) if error.is_conflict() => {
            // Changed elsewhere meanwhile: still destroy what is there now.
            match dav.refetch(&resource).await.map_err(|e| set_error(&e))? {
                Some(fresh) => dav
                    .delete_resource(&fresh.resource)
                    .await
                    .map_err(|e| set_error(&e)),
                None => Ok(()),
            }
        }
        Err(error) => Err(set_error(&error)),
    };
    result?;
    deliver(
        ctx,
        &Outgoing {
            text: String::new(),
            messages,
            sender,
        },
        Occasion::Changed,
    )
    .await;
    Ok(())
}

async fn destroy_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    sched: &Scheduling,
    all: &[Collection],
    which: EventRef,
) -> Result<(), SetError> {
    let (event, calendar) = stored_event(ctx, all, which.resource()).await?;
    match which {
        EventRef::Base(_) => {
            if !allows(&calendar, Rights::UNBIND) {
                return Err(forbidden("this calendar is read-only"));
            }
            delete_event(ctx, dav, sched, event).await
        }
        EventRef::Occurrence(_, key) => {
            let text = event_text(dav, &event).await?;
            let probe = text.text.clone();
            match blocking(move || occurrence_of(&probe, key).map_err(|e| convert_error(&e)))
                .await?
            {
                Occurrence::Whole => {
                    if !allows(&calendar, Rights::UNBIND) {
                        return Err(forbidden("this calendar is read-only"));
                    }
                    return delete_event(ctx, dav, sched, text.resource).await;
                }
                Occurrence::Missing => return Err(SetError::not_found()),
                Occurrence::One(_) => {}
            }
            if !allows(&calendar, Rights::WRITE_CONTENT) {
                return Err(forbidden("this calendar is read-only"));
            }
            write_change(ctx, dav, sched, text.resource, Change::Exclude(key))
                .await
                .map(|_| ())
        }
    }
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: CalendarEventSetArguments,
) -> Result<MethodResponse, CallError> {
    let set = args.set;
    ctx.check_account(&set.account_id)?;
    let dav = super::account(ctx)?;
    let old_state = ctx
        .check_state(DataType::CalendarEvent, set.if_in_state.as_ref())
        .await?;
    let all = calendars(ctx).await?;
    let sched = Scheduling {
        notify: args.send_scheduling_messages,
        server: dav.server_schedules().unwrap_or(false),
        own: Arc::new(super::own_addresses(ctx, &dav)),
    };
    let mut created: BTreeMap<CreationId, CalendarEvent> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in set.create {
        match create_one(ctx, &dav, &sched, &all, object).await {
            Ok((id, object)) => {
                ctx.created.insert(creation_id.clone(), ids::event_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let destroying: Vec<Option<CalendarEventId>> =
        set.destroy.iter().map(|key| ctx.resolve(key)).collect();
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &set.update {
        let id = ctx.resolve(key);
        let which = id.as_ref().and_then(ids::parse_event);
        let result = match which {
            // Stalwart: an update of something this request destroys.
            Some(which)
                if destroying.iter().flatten().any(|d| {
                    ids::parse_event(d).is_some_and(|gone| {
                        gone == which || gone == EventRef::Base(which.resource())
                    })
                }) =>
            {
                Err(SetError::new(SetErrorType::WillDestroy))
            }
            Some(which) => update_one(ctx, &dav, &sched, &all, which, patch).await,
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
    for (key, id) in set.destroy.iter().zip(destroying) {
        let result = match id.as_ref().and_then(ids::parse_event) {
            Some(which) => destroy_one(ctx, &dav, &sched, &all, which).await,
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
    let new_state = ctx.state(DataType::CalendarEvent).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::CalendarEventSet(response))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::calendars::set_itip::MAX_RECIPIENTS;

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
        UID:w@example.com\r\nDTSTAMP:20261001T000000Z\r\nX-KEEP:yes\r\n\
        DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDTEND;TZID=Europe/Madrid:20261019T103000\r\n\
        RRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const ALL_DAY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
        UID:d@example.com\r\nDTSTAMP:20261001T000000Z\r\nDTSTART;VALUE=DATE:20261019\r\n\
        DTEND;VALUE=DATE:20261020\r\nRRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Day\r\nEND:VEVENT\r\n\
        END:VCALENDAR\r\n";

    const SINGLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:s@example.com\r\n\
        DTSTAMP:20261001T000000Z\r\nDTSTART:20261019T100000Z\r\nDURATION:PT1H\r\n\
        SUMMARY:Once\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// An event the user organizes with `guests` attendees, in the future.
    fn with_guests(guests: usize) -> String {
        let mut text = String::from(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
             UID:[EMAIL]\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:20991019T100000Z\r\n\
             DURATION:PT1H\r\nSUMMARY:Big\r\nORGANIZER:mailto:[EMAIL]\r\n",
        );
        for guest in 0..guests {
            text.push_str("ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:guest");
            text.push_str(&guest.to_string());
            text.push_str("@example.com\r\n");
        }
        text.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        text
    }

    /// The cancellations of a deletion have the same limit as the rest
    ///more than 20 is refused (`forbidden`, before
    /// anything is deleted); 20 go out.
    #[test]
    fn cancellations_have_the_recipient_limit() {
        let own = ["[EMAIL]".to_owned()];
        let cancel = |guests: usize| {
            let old = with_guests(guests);
            let plan = organizer_messages(Some(old.as_bytes()), None, &own, 0).unwrap();
            sendable(plan, too_many_to_cancel)
        };
        let refused = cancel(MAX_RECIPIENTS + 5).unwrap_err();
        assert_eq!(refused.kind(), SetErrorType::Forbidden);
        let sent = cancel(MAX_RECIPIENTS).unwrap();
        assert_eq!(sent.recipients().len(), MAX_RECIPIENTS);
        assert!(
            sent.messages
                .iter()
                .all(|m| m.method == itip::Method::Cancel),
            "{sent:?}"
        );
    }

    fn patches(value: Value) -> Arc<Vec<BodyPatch>> {
        Arc::new(
            CalendarEventPatch::parse(&serde_json::from_value(value).unwrap())
                .unwrap()
                .body,
        )
    }

    /// Who sends: the server for the whole event, the
    /// bridge for one occurrence (2026-10-29) and without such a server.
    #[test]
    fn who_sends() {
        for write in [Write::Create, Write::Series] {
            assert_eq!(organizer_plan(true, true, write), (Agent::Server, false));
            assert_eq!(organizer_plan(true, false, write), (Agent::Client, false));
            assert_eq!(organizer_plan(false, true, write), (Agent::Client, true));
            assert_eq!(organizer_plan(false, false, write), (Agent::Client, false));
        }
        for server in [true, false] {
            assert_eq!(
                organizer_plan(server, true, Write::Occurrence),
                (Agent::Client, true)
            );
            assert_eq!(
                organizer_plan(server, false, Write::Occurrence),
                (Agent::Client, false)
            );
        }
    }

    /// The key of the occurrence id `v1_{id}`.
    fn key(id: &str) -> RecurrenceKey {
        match ids::parse_event(&format!("v1_{id}").parse().unwrap()).unwrap() {
            EventRef::Occurrence(_, key) => key,
            EventRef::Base(_) => panic!("not an occurrence"),
        }
    }

    #[test]
    fn one_occurrence_becomes_an_override() {
        let change = Change::Occurrence(
            key("20261026T100000"),
            patches(json!({"title": "Moved", "start": "2026-10-26T12:00:00"})),
        );
        let text = changed_text(WEEKLY, &change, 0).unwrap().unwrap();
        assert!(
            text.contains("RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000"),
            "{text}"
        );
        assert!(text.contains("DTSTART;TZID=Europe/Madrid:20261026T120000"));
        assert!(text.contains("SUMMARY:Moved") && text.contains("SUMMARY:Weekly"));
        assert!(text.contains("X-KEEP:yes"));
        let gone = changed_text(WEEKLY, &Change::Exclude(key("20261102T100000")), 0)
            .unwrap()
            .unwrap();
        assert!(
            gone.contains("EXDATE;TZID=Europe/Madrid:20261102T100000"),
            "{gone}"
        );
        assert_eq!(
            changed_text(WEEKLY, &Change::Exclude(key("20261102T110000")), 0)
                .unwrap_err()
                .kind(),
            SetErrorType::NotFound
        );
    }

    /// Changing one day of an all-day series (what Stalwart 0.16.27 gets
    /// wrong): an override with `RECURRENCE-ID;VALUE=DATE`,
    /// and the series stays.
    #[test]
    fn one_day_of_an_all_day_series() {
        let change = Change::Occurrence(
            key("20261021T000000"),
            patches(json!({"title": "Changed day", "start": "2026-10-22T00:00:00"})),
        );
        let text = changed_text(ALL_DAY, &change, 0).unwrap().unwrap();
        assert!(text.contains("RECURRENCE-ID;VALUE=DATE:20261021"), "{text}");
        assert!(text.contains("DTSTART;VALUE=DATE:20261022"), "{text}");
        assert!(text.contains("RRULE:FREQ=DAILY;COUNT=5"), "{text}");
        assert_eq!(text.matches("BEGIN:VEVENT").count(), 2);
        let gone = changed_text(ALL_DAY, &Change::Exclude(key("20261023T000000")), 0)
            .unwrap()
            .unwrap();
        assert!(gone.contains("EXDATE;VALUE=DATE:20261023"), "{gone}");
    }

    /// An occurrence id of an event without repetition names the event;
    /// one the event doesn't have names nothing; a broken text is an
    /// error (not a made-up occurrence).
    #[test]
    fn single_events() {
        assert_eq!(
            occurrence_of(SINGLE, key("20261019T100000")).unwrap(),
            Occurrence::Whole
        );
        assert_eq!(
            occurrence_of(WEEKLY, key("20261019T100000")).unwrap(),
            Occurrence::One(key("20261019T100000"))
        );
        assert_eq!(
            occurrence_of(SINGLE, key("20301231T100000")).unwrap(),
            Occurrence::Missing
        );
        // Larger than an event may be: an error, not "missing".
        let huge = "x".repeat(nebula_groupware::model::convert::MAX_ICALENDAR_BYTES + 1);
        assert!(occurrence_of(&huge, key("20261019T100000")).is_err());
        let change =
            Change::Occurrence(key("20261019T100000"), patches(json!({"title": "Renamed"})));
        let text = changed_text(SINGLE, &change, 0).unwrap().unwrap();
        assert!(text.contains("SUMMARY:Renamed") && !text.contains("RECURRENCE-ID"));
        let same = Change::Series(patches(json!({"title": "Once"})));
        assert!(changed_text(SINGLE, &same, 0).unwrap().is_none());
    }

    #[test]
    fn new_events_get_a_uid() {
        let body = JsBody::new(
            json!({"title": "x", "start": "2026-10-19T10:00:00"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let (members, uid) = new_body(body).unwrap();
        assert_eq!(members["@type"], "Event");
        assert_eq!(members["uid"], uid.unwrap().as_str());
    }

    /// A weekly task due on Mondays (tasks.org style: only `DUE`).
    const WEEKLY_TASK: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n\
        BEGIN:VTODO\r\nUID:t@example.com\r\nDTSTAMP:20261001T000000Z\r\nX-APPLE-SORT-ORDER:3\r\n\
        DUE;VALUE=DATE:20261019\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Plants\r\n\
        STATUS:NEEDS-ACTION\r\nATTENDEE:mailto:bob@example.com\r\n\
        ORGANIZER:mailto:alice@example.com\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// Tasks: the web completes one occurrence
    /// by its synthetic id, then the whole task; then reopens it.
    #[test]
    fn tasks_complete_by_occurrence_and_whole() {
        assert!(is_task(WEEKLY_TASK) && !is_task(WEEKLY));
        let one = Change::Occurrence(
            key("20261026T000000"),
            patches(json!({"progress": "completed"})),
        );
        let text = changed_text(WEEKLY_TASK, &one, 1_793_000_000)
            .unwrap()
            .unwrap();
        assert_eq!(text.matches("BEGIN:VTODO").count(), 2, "{text}");
        assert!(text.contains("RECURRENCE-ID;VALUE=DATE:20261026"), "{text}");
        assert!(text.contains("DUE;VALUE=DATE:20261026"), "{text}");
        assert!(text.contains("COMPLETED:20261026T073320Z"), "{text}");
        assert_eq!(text.matches("STATUS:NEEDS-ACTION").count(), 1, "{text}");
        assert!(text.contains("X-APPLE-SORT-ORDER:3"), "{text}");
        assert!(!text.contains("VEVENT"), "{text}");

        let whole = Change::Series(patches(json!({"progress": "completed"})));
        let done = changed_text(WEEKLY_TASK, &whole, 1_793_000_000)
            .unwrap()
            .unwrap();
        assert!(done.contains("PERCENT-COMPLETE:100"), "{done}");
        let reopen = Change::Series(patches(json!({"progress": "needs-action"})));
        let open = changed_text(&done, &reopen, 1_793_000_060)
            .unwrap()
            .unwrap();
        assert!(!open.contains("COMPLETED:2026"), "{open}");
        assert!(!open.contains("PERCENT-COMPLETE"), "{open}");

        // A task may lose its due and start; an event can't lose its start.
        let single = WEEKLY_TASK.replace("RRULE:FREQ=WEEKLY;COUNT=4\r\n", "");
        let undated = Change::Series(patches(json!({"due": null, "showWithoutTime": null})));
        let text = changed_text(&single, &undated, 0).unwrap().unwrap();
        assert!(!text.contains("DUE") && !text.contains("JSPROP"), "{text}");
        let no_start = Change::Series(patches(json!({"start": null})));
        assert_eq!(
            changed_text(WEEKLY, &no_start, 0).unwrap_err().properties(),
            ["start"]
        );
        // An event does not become a task.
        assert!(
            CalendarEventPatch::parse(&serde_json::from_value(json!({"@type": "Task"})).unwrap())
                .is_err()
        );
    }

    /// `useDefaultAlerts: true`: the calendar's defaults (with or without
    /// time) replace `alerts` and are written as `VALARM`s; a task keeps
    /// its own.
    #[test]
    fn default_alerts_on_create() {
        let defaults = |value: Value| {
            Some(
                DefaultAlerts::from_jmap(value.as_object().unwrap())
                    .unwrap()
                    .to_valarms(),
            )
        };
        let mut calendar = Collection {
            id: CollectionId::from_raw(1).unwrap(),
            props: nebula_store::CollectionProps {
                kind: CollectionKind::Calendar,
                href: nebula_store::DavHref::new("https://dav.example/cal/a/w/").unwrap(),
                name: None,
                color: None,
                description: None,
                sort_order: None,
                time_zone: None,
                default_alarms_datetime: defaults(json!({"w": {"trigger": {"offset": "-PT10M"}}})),
                default_alarms_date: defaults(json!({"d": {"trigger": {"offset": "-PT15H"}}})),
                rights: None,
                components: None,
                vcard_versions: None,
                supports_sync: true,
                sharing: nebula_store::CollectionSharing::default(),
                extras: nebula_store::CalendarExtras::default(),
            },
            sync: nebula_store::CollectionSync::default(),
            orphaned_at: None,
            is_visible: true,
            is_subscribed: true,
            is_default: false,
        };
        let mut timed = json!({"@type": "Event", "uid": "u", "start": "2026-11-02T10:00:00",
            "timeZone": "Europe/Madrid", "duration": "PT1H",
            "alerts": {"mine": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT1M"}}}})
        .as_object()
        .unwrap()
        .clone();
        apply_default_alerts(&mut timed, &calendar);
        let text = new_icalendar(&timed, 0).unwrap();
        assert!(text.contains(":-PT10M\r\nJSID:w"), "{text}");
        assert!(!text.contains("-PT1M"), "{text}");
        let mut day = json!({"@type": "Event", "uid": "u", "start": "2026-11-02T00:00:00",
            "showWithoutTime": true, "duration": "P1D"})
        .as_object()
        .unwrap()
        .clone();
        apply_default_alerts(&mut day, &calendar);
        assert!(new_icalendar(&day, 0).unwrap().contains(":-PT15H\r\n"));
        calendar.props.default_alarms_datetime = None;
        apply_default_alerts(&mut timed, &calendar);
        assert!(!new_icalendar(&timed, 0).unwrap().contains("VALARM"));
        let mut task = json!({"@type": "Task", "uid": "t",
            "alerts": {"mine": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT1M"}}}})
        .as_object()
        .unwrap()
        .clone();
        apply_default_alerts(&mut task, &calendar);
        assert!(task.contains_key("alerts"));
    }

    #[test]
    fn new_tasks_are_tasks() {
        let body = JsBody::new(
            json!({"@type": "Task", "title": "x", "due": "2026-10-30T00:00:00"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let (members, _) = new_body(body).unwrap();
        assert_eq!(members["@type"], "Task");
        let text = new_icalendar(&members, 0).unwrap();
        assert!(text.contains("BEGIN:VTODO") && is_task(&text), "{text}");
    }
}
