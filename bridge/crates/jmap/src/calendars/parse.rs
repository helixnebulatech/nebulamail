// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/parse` (`urn:ietf:params:jmap:calendars:parse`): every
//! event of each blob, a list per blob as Stalwart answers (a series with
//! its overrides, then other events), with the unconverted properties
//! under `iCalendar`. A `RECURRENCE-ID;VALUE=DATE` becomes `recurrenceId`
//! (the day's midnight), as in Stalwart.
//! Any blob works: an upload, a message or one of its parts (invitations
//! in mail). It reads no calendar, so it needs no DAV server; it is still
//! announced only with `calendars` (the web uses both together).

use std::collections::BTreeMap;

use nebula_groupware::model::calendar::parse_events;
use nebula_groupware::model::convert::MAX_ICALENDAR_BYTES;
use nebula_jmap_proto::object::{
    CalendarEvent, CalendarEventParseArguments, CalendarEventParseResponse, CalendarEventProperty,
};
use nebula_jmap_proto::{BlobId, MethodResponse};

use super::event::body;
use crate::core::context::Ctx;
use crate::email::build::blob_bytes;
use crate::error::CallError;

pub(crate) async fn parse(
    ctx: &Ctx<'_>,
    args: CalendarEventParseArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let names: Option<Vec<String>> = args.properties.map(|list| {
        list.iter()
            .filter_map(|p| match p {
                CalendarEventProperty::Body(name) => Some(name.as_str().to_owned()),
                _ => None,
            })
            .collect()
    });
    let mut parsed: BTreeMap<BlobId, Vec<CalendarEvent>> = BTreeMap::new();
    let mut not_parsable = Vec::new();
    let mut not_found = Vec::new();
    for blob in args.blob_ids {
        if parsed.contains_key(&blob) || not_parsable.contains(&blob) || not_found.contains(&blob) {
            continue;
        }
        // Held while this blob is: its memory is counted.
        let Ok(mut lease) = crate::email::build::lease(ctx) else {
            not_found.push(blob);
            continue;
        };
        let Ok(Some(bytes)) = blob_bytes(ctx, &blob, &mut lease).await else {
            not_found.push(blob);
            continue;
        };
        // Larger than an event may be: refused before a blocking task.
        if bytes.len() > MAX_ICALENDAR_BYTES {
            not_parsable.push(blob);
            continue;
        }
        let Ok(events) = super::blocking(move || parse_events(&bytes)).await? else {
            not_parsable.push(blob);
            continue;
        };
        let events = events
            .into_iter()
            .map(|members| {
                let full = body(members);
                let body = match &names {
                    None => full,
                    Some(names) => full.select(names.iter().map(String::as_str)),
                };
                CalendarEvent {
                    body,
                    ..CalendarEvent::default()
                }
            })
            .collect();
        parsed.insert(blob, events);
    }
    Ok(MethodResponse::CalendarEventParse(
        CalendarEventParseResponse {
            account_id: ctx.jmap_id(),
            parsed,
            not_parsable,
            not_found,
        },
    ))
}
