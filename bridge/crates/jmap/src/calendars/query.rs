// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/query` (draft-ietf-jmap-calendars §5.8), as Stalwart
//! answers the web:
//!
//! - **`expandRecurrences`**: one id per occurrence between `after` and
//!   `before` (both required, read in `timeZone`), each with its
//!   `baseEventId`; an event that doesn't repeat comes under its own id,
//!   as Stalwart 1.0 ([`ParsedCalendar::is_series`]); at most
//!   [`MAX_EXPANDED_RESULTS`](super::MAX_EXPANDED_RESULTS) occurrences
//!   (`cannotCalculateOccurrences` beyond, as Stalwart 1.0) and a range of
//!   at most `maxExpandedQueryDuration` (`expandDurationTooLarge`).
//!   An occurrence matches if it starts before `before` and ends after
//!   `after`.
//! - Without it, stored events: `after`/`before` match if any occurrence
//!   does.
//! - Filters `inCalendar`, `uid` (exact), `after`, `before` and the text
//!   ones (`text`, `title`, `description`, `location`, `owner`,
//!   `attendee`, `participant`: substrings without case or accents, over
//!   the opened copy, `nebula_groupware::model::search`), with AND, OR
//!   and NOT. Sorts `start`, `recurrenceId`
//!   (the same here), `uid` and `id`; `created`/`updated`:
//!   `unsupportedSort` (Stalwart 0.16 too).
//!
//! - **Tasks** (`VTODO`, `@type: "Task"`) come
//!   with the events: without dates every task of the calendar (also
//!   those without `due` or `start`); with `after`/`before` a task (each
//!   occurrence of a recurring one) matches when its `due` (or its
//!   `start` if it has no `due`) is in `[after, before)`, a point in
//!   time, and sorts by it; a task without either never matches a range.
//!
//! Candidates come from the clear-text index (UTC range widened by 14 h for
//! floating times); only those are opened and expanded.

use std::cmp::Ordering;

use nebula_groupware::model::calendar::{Instance, ParsedCalendar, Zone};
use nebula_groupware::model::search::{EventField, EventText, Needle};
use nebula_jmap_proto::method::{Comparator, Filter, Operator, QueryResponse};
use nebula_jmap_proto::object::{
    CalendarEventFilterCondition, CalendarEventQueryArguments, CalendarEventQueryChangesArguments,
    CalendarEventSort,
};
use nebula_jmap_proto::types::LocalDateTime;
use nebula_jmap_proto::{
    CalendarEventId, MethodError, MethodErrorType, MethodResponse, UnsignedInt,
};
use nebula_store::{
    CollectionId, CollectionKind, DataType, Parsed, ResourceId, ResourceQuery, StoredResource,
    UnixTime,
};

use super::{MAX_EXPANDED_RESULTS, ids};
use crate::changes::QueryLog;
use crate::collation::CheckedSort;
use crate::core::context::Ctx;
use crate::error::CallError;

/// How far a floating time may be from UTC: the index reads them as UTC.
const FLOATING_MARGIN: i64 = 14 * 3600;
/// Most ids one query returns without `expandRecurrences`.
const MAX_RESULTS: u64 = 5000;

/// A stored event that may match, with its occurrences when the filter
/// needs them.
struct Candidate {
    id: ResourceId,
    collection: CollectionId,
    uid: Option<String>,
    first_start: Option<i64>,
    instances: Vec<Instance>,
    /// Its occurrences have ids of their own ([`ParsedCalendar::is_series`]).
    series: bool,
    /// Its searchable text, when the filter has a text condition (`None`
    /// also when it does not parse: no text condition matches).
    text: Option<EventText>,
}

/// One result, before sorting.
struct Hit {
    resource: ResourceId,
    instance: Option<Instance>,
    start: i64,
    uid: Option<String>,
}

/// Every condition of the filter, depth first (bounded by the parser).
fn conditions(filter: &Filter<CalendarEventFilterCondition>) -> Vec<&CalendarEventFilterCondition> {
    let mut out = Vec::new();
    let mut stack = vec![filter];
    while let Some(filter) = stack.pop() {
        match filter {
            Filter::Condition(condition) => out.push(condition),
            Filter::Operator(operator) => stack.extend(operator.conditions.iter()),
            _ => {}
        }
    }
    out
}

/// The text conditions of one filter condition, with their fields.
fn text_conditions(c: &CalendarEventFilterCondition) -> Vec<(&str, EventField)> {
    [
        (&c.text, EventField::Text),
        (&c.title, EventField::Title),
        (&c.description, EventField::Description),
        (&c.location, EventField::Location),
        (&c.owner, EventField::Owner),
        (&c.attendee, EventField::Attendee),
        (&c.participant, EventField::Participant),
    ]
    .into_iter()
    .filter_map(|(term, field)| term.as_deref().map(|term| (term, field)))
    .collect()
}

fn needs_text(filter: Option<&Filter<CalendarEventFilterCondition>>) -> bool {
    filter
        .map(conditions)
        .unwrap_or_default()
        .iter()
        .any(|c| !text_conditions(c).is_empty())
}

/// The checked `sort` (RFC 8620 §5.5), before any work: `created` and
/// `updated` are `unsupportedSort`, and so is a collation the session
/// doesn't offer.
fn check_sort(
    sort: &[Comparator<CalendarEventSort>],
) -> Result<CheckedSort<'_, CalendarEventSort>, MethodError> {
    if sort.iter().any(|comparator| {
        matches!(
            comparator.property,
            CalendarEventSort::Created | CalendarEventSort::Updated
        )
    }) {
        return Err(MethodError::unsupported_sort(
            "cannot sort events by created or updated",
        ));
    }
    CheckedSort::new(sort)
}

/// Seconds since the epoch of a local date-time, as if it were UTC.
fn naive(local: LocalDateTime) -> i64 {
    local.to_primitive().assume_utc().unix_timestamp()
}

fn to_utc(zone: Zone, local: LocalDateTime) -> Result<i64, CallError> {
    zone.to_utc(naive(local)).ok_or_else(|| {
        MethodError::invalid_arguments("a date does not exist in that timeZone").into()
    })
}

fn condition_matches(
    candidate: &Candidate,
    condition: &CalendarEventFilterCondition,
    zone: Zone,
) -> Result<bool, CallError> {
    if let Some(calendar) = &condition.in_calendar
        && ids::store_calendar(calendar) != Some(candidate.collection)
    {
        return Ok(false);
    }
    if let Some(uid) = &condition.uid
        && candidate.uid.as_deref() != Some(uid.as_str())
    {
        return Ok(false);
    }
    for (term, field) in text_conditions(condition) {
        let found = candidate
            .text
            .as_ref()
            .is_some_and(|text| text.matches(field, &Needle::new(term)));
        if !found {
            return Ok(false);
        }
    }
    if let Some(after) = condition.after {
        let after = to_utc(zone, after)?;
        if !candidate
            .instances
            .iter()
            .any(|i| i.overlaps(after, i64::MAX))
        {
            return Ok(false);
        }
    }
    if let Some(before) = condition.before {
        let before = to_utc(zone, before)?;
        if !candidate
            .instances
            .iter()
            .any(|i| i.overlaps(i64::MIN, before))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn matches(
    candidate: &Candidate,
    filter: &Filter<CalendarEventFilterCondition>,
    zone: Zone,
) -> Result<bool, CallError> {
    Ok(match filter {
        Filter::Condition(condition) => condition_matches(candidate, condition, zone)?,
        Filter::Operator(operator) => {
            let mut results = Vec::with_capacity(operator.conditions.len());
            for sub in &operator.conditions {
                results.push(matches(candidate, sub, zone)?);
            }
            match operator.operator {
                Operator::And => results.iter().all(|r| *r),
                Operator::Or => results.iter().any(|r| *r),
                Operator::Not => !results.iter().any(|r| *r),
                _ => false,
            }
        }
        _ => false,
    })
}

/// What the index can narrow down: only for a filter that is one
/// condition (always the case with `expandRecurrences`). `None`: nothing
/// can match (a calendar id that is not ours).
fn prefilter(
    filter: Option<&Filter<CalendarEventFilterCondition>>,
    zone: Zone,
) -> Result<Option<ResourceQuery>, CallError> {
    let mut query = ResourceQuery::all(CollectionKind::Calendar);
    let Some(Filter::Condition(condition)) = filter else {
        return Ok(Some(query));
    };
    if let Some(calendar) = &condition.in_calendar {
        let Some(collection) = ids::store_calendar(calendar) else {
            return Ok(None);
        };
        query.collection = Some(collection);
    }
    query.uid.clone_from(&condition.uid);
    if condition.after.is_some() || condition.before.is_some() {
        let after = condition
            .after
            .map(|after| to_utc(zone, after))
            .transpose()?
            .map_or(i64::MIN, |t| t.saturating_sub(FLOATING_MARGIN));
        let before = condition
            .before
            .map(|before| to_utc(zone, before))
            .transpose()?
            .map_or(i64::MAX, |t| t.saturating_add(FLOATING_MARGIN));
        query.overlapping = Some((UnixTime::from_secs(after), UnixTime::from_secs(before)));
    }
    Ok(Some(query))
}

fn needs_instances(filter: Option<&Filter<CalendarEventFilterCondition>>, expand: bool) -> bool {
    expand
        || filter
            .map(conditions)
            .unwrap_or_default()
            .iter()
            .any(|c| c.after.is_some() || c.before.is_some())
}

/// The candidates, opened (and expanded, or read for a text filter) when
/// needed.
async fn candidates(
    ctx: &Ctx<'_>,
    stored: Vec<StoredResource>,
    zone: Zone,
    with_instances: bool,
    filter: Option<&Filter<CalendarEventFilterCondition>>,
) -> Result<Vec<Candidate>, CallError> {
    let running = super::account(ctx)?;
    let with_text = needs_text(filter);
    let open = with_instances || with_text;
    let mut texts = Vec::new();
    let mut out = Vec::with_capacity(stored.len());
    for resource in stored {
        let (uid, first_start) = match &resource.parsed {
            Parsed::Valid(index) => (index.uid.clone(), index.utc_start.map(UnixTime::secs)),
            Parsed::Invalid => (None, None),
        };
        if open {
            // An unreadable copy has no text here (no rereads in bulk
            // during a query); `CalendarEvent/get` reads it again.
            let text = match running.resource_bytes(resource.id).await {
                Ok(text) => text,
                Err(nebula_groupware::GroupwareError::Unreadable) => None,
                Err(error) => return Err(super::groupware_error(&error)),
            };
            texts.push(text);
        }
        out.push(Candidate {
            id: resource.id,
            collection: resource.collection,
            uid,
            first_start,
            instances: Vec::new(),
            series: false,
            text: None,
        });
    }
    if open {
        let read = super::blocking(move || {
            texts
                .into_iter()
                .map(|text| {
                    let parsed = text
                        .as_deref()
                        .filter(|_| with_instances)
                        .and_then(|text| ParsedCalendar::parse(text).ok())
                        .filter(ParsedCalendar::has_events);
                    let series = parsed.as_ref().is_some_and(ParsedCalendar::is_series);
                    let instances = parsed
                        .and_then(|parsed| parsed.instances(zone).ok())
                        .map(|instances| instances.list)
                        .unwrap_or_default();
                    let searchable = text
                        .as_deref()
                        .filter(|_| with_text)
                        .and_then(|text| EventText::parse(text).ok());
                    (instances, series, searchable)
                })
                .collect::<Vec<_>>()
        })
        .await?;
        for (candidate, (instances, series, text)) in out.iter_mut().zip(read) {
            candidate.instances = instances;
            candidate.series = series;
            candidate.text = text;
        }
    }
    Ok(out)
}

/// `uid` with the comparator's collation (RFC 8620 §5.5;
/// case-insensitive by default; events without one first); `start`,
/// `recurrenceId` and `id` are not text and ignore it.
fn compare(a: &Hit, b: &Hit, sort: &CheckedSort<'_, CalendarEventSort>) -> Ordering {
    sort.compare(a, b, |property, collate, a, b| match property {
        CalendarEventSort::Start | CalendarEventSort::RecurrenceId => a.start.cmp(&b.start),
        CalendarEventSort::Uid => match (&a.uid, &b.uid) {
            (Some(x), Some(y)) => collate.compare(x, y),
            (x, y) => x.is_some().cmp(&y.is_some()),
        },
        CalendarEventSort::Id => {
            (a.resource, a.instance.map(|i| i.key)).cmp(&(b.resource, b.instance.map(|i| i.key)))
        }
        _ => Ordering::Equal,
    })
}

/// The occurrences of an expanded query, in index order.
fn expanded_hits(
    candidates: Vec<Candidate>,
    filter: &CalendarEventFilterCondition,
    zone: Zone,
    (after, before): (i64, i64),
) -> Result<Vec<Hit>, CallError> {
    let mut hits = Vec::new();
    for candidate in candidates {
        // Everything but the range (checked per occurrence below).
        let resource_filter = CalendarEventFilterCondition {
            after: None,
            before: None,
            ..filter.clone()
        };
        if !condition_matches(&candidate, &resource_filter, zone)? {
            continue;
        }
        for instance in &candidate.instances {
            if instance.overlaps(after, before) {
                if hits.len() >= MAX_EXPANDED_RESULTS {
                    return Err(
                        MethodError::new(MethodErrorType::CannotCalculateOccurrences)
                            .with_description(
                                "the number of expanded recurrences exceeds the server limit",
                            )
                            .into(),
                    );
                }
                hits.push(Hit {
                    resource: candidate.id,
                    instance: candidate.series.then_some(*instance),
                    start: instance.placed_at(),
                    uid: candidate.uid.clone(),
                });
            }
        }
    }
    Ok(hits)
}

/// The UTC range of an expanded query, checked against
/// `maxExpandedQueryDuration`; `None` without `expandRecurrences`.
fn expansion_window(
    args: &CalendarEventQueryArguments,
    zone: Zone,
) -> Result<Option<(i64, i64)>, CallError> {
    if !args.expand_recurrences {
        return Ok(None);
    }
    let Some((after, before)) = args.expansion_range() else {
        return Err(MethodError::invalid_arguments(
            "expandRecurrences needs a filter condition with both after and before",
        )
        .into());
    };
    let longest = i64::try_from(super::max_expanded_seconds()?).unwrap_or(i64::MAX);
    if naive(before).saturating_sub(naive(after)) > longest {
        return Err(MethodError::new(MethodErrorType::ExpandDurationTooLarge)
            .with_description("the time between after and before exceeds maxExpandedQueryDuration")
            .into());
    }
    Ok(Some((to_utc(zone, after)?, to_utc(zone, before)?)))
}

/// The results of a query, sorted (filter and sort checked; `window`:
/// the range of an expanded one).
async fn results(
    ctx: &Ctx<'_>,
    filter: Option<&Filter<CalendarEventFilterCondition>>,
    sort: &CheckedSort<'_, CalendarEventSort>,
    window: Option<(i64, i64)>,
    zone: Zone,
) -> Result<Vec<Hit>, CallError> {
    let mut stored = match prefilter(filter, zone)? {
        Some(narrowed) => ctx.store().visible_resources(ctx.id(), narrowed).await?,
        None => Vec::new(),
    };
    crate::sharing::scope::retain(ctx, CollectionKind::Calendar, &mut stored).await?;
    let with_instances = needs_instances(filter, window.is_some());
    let candidates = candidates(ctx, stored, zone, with_instances, filter).await?;

    let mut hits = match (window, filter) {
        (Some(window), Some(Filter::Condition(condition))) => {
            expanded_hits(candidates, condition, zone, window)?
        }
        (Some(_), _) => Vec::new(),
        (None, _) => {
            let mut hits = Vec::new();
            for candidate in candidates {
                if let Some(filter) = filter
                    && !matches(&candidate, filter, zone)?
                {
                    continue;
                }
                hits.push(Hit {
                    resource: candidate.id,
                    instance: None,
                    start: candidate.first_start.unwrap_or(i64::MAX),
                    uid: candidate.uid,
                });
            }
            hits
        }
    };
    // Stable: equal ones keep the index order, as in Stalwart.
    hits.sort_by(|a, b| compare(a, b, sort));
    Ok(hits)
}

fn hit_id(hit: &Hit) -> Result<CalendarEventId, crate::error::Bug> {
    match hit.instance {
        Some(instance) => ids::occurrence_id(hit.resource, instance.key),
        None => ids::event_id(hit.resource),
    }
}

/// [`hit_id`], leaving out (and logging) a hit without an id instead of
/// failing the whole query: one broken event must not hide the rest.
fn listed_id(hit: &Hit) -> Option<CalendarEventId> {
    hit_id(hit)
        .inspect_err(|error| {
            tracing::warn!(resource = hit.resource.get(), %error, "calendars: a hit without an id left out");
        })
        .ok()
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    args: CalendarEventQueryArguments,
) -> Result<MethodResponse, CallError> {
    let query = &args.query;
    ctx.check_account(&query.account_id)?;
    super::account(ctx)?;
    let zone = super::zone(&args.time_zone)?;
    let filter = query.filter.as_ref();
    let sort = check_sort(&query.sort)?;
    let state = ctx.state(DataType::CalendarEvent).await?;

    let window = expansion_window(&args, zone)?;
    let hits = results(ctx, filter, &sort, window, zone).await?;

    let all: Vec<CalendarEventId> = hits.iter().filter_map(listed_id).collect();
    let indexes: Vec<u32> = (0..u32::try_from(all.len()).unwrap_or(u32::MAX)).collect();
    let anchor = match &query.anchor {
        Some(anchor) => Some(
            all.iter()
                .position(|id| id == anchor)
                .and_then(|index| u32::try_from(index).ok())
                .ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
        ),
        None => None,
    };
    let asked = query.limit.map(UnsignedInt::get);
    let limit = asked.map_or(MAX_RESULTS, |n| n.min(MAX_RESULTS));
    let page = crate::paging::page(
        &indexes,
        query.position.get(),
        anchor.map(|a| (a, query.anchor_offset.get())),
        Some(limit),
    )?;
    let ids = page
        .ids
        .iter()
        .filter_map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|i| all.get(i))
                .cloned()
        })
        .collect();
    Ok(MethodResponse::CalendarEventQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: state,
        // Occurrences have no change log (Stalwart neither).
        can_calculate_changes: !args.expand_recurrences,
        position: UnsignedInt::saturating(page.position),
        ids,
        total: query
            .calculate_total
            .then(|| UnsignedInt::try_from(all.len()).unwrap_or(UnsignedInt::MAX)),
        limit: (asked != Some(limit)).then(|| UnsignedInt::saturating(limit)),
    }))
}

/// `CalendarEvent/queryChanges` (RFC 8620 §5.6): from the
/// `CalendarEvent` change log ([`crate::changes::QueryLog`]) for a query
/// of stored events; an expanded one is `cannotCalculateChanges` (the log
/// names events, not occurrences; Stalwart does not do it either).
pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: CalendarEventQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    let query = &args.query_changes;
    ctx.check_account(&query.account_id)?;
    super::account(ctx)?;
    if args.expand_recurrences {
        return Err(MethodError::new(MethodErrorType::CannotCalculateChanges)
            .with_description("changes of a query with expandRecurrences are not calculated")
            .into());
    }
    let zone = super::zone(&args.time_zone)?;
    let filter = query.filter.as_ref();
    let sort = check_sort(&query.sort)?;
    let log = QueryLog::since(
        ctx,
        DataType::CalendarEvent,
        &query.since_query_state,
        query.max_changes,
    )
    .await?;
    let current = results(ctx, filter, &sort, None, zone)
        .await?
        .iter()
        .filter_map(|hit| Some((hit.resource.get(), listed_id(hit)?)))
        .collect::<Vec<_>>();
    Ok(MethodResponse::CalendarEventQueryChanges(log.response(
        ctx,
        query.since_query_state.clone(),
        &current,
        query.calculate_total,
        ids::event_row,
    )?))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn hit(resource: i64, start: i64, uid: Option<&str>) -> Hit {
        Hit {
            resource: ResourceId::from_raw(resource).unwrap(),
            instance: None,
            start,
            uid: uid.map(str::to_owned),
        }
    }

    fn order(hits: &[Hit], value: serde_json::Value) -> Result<Vec<i64>, MethodErrorType> {
        let comparators: Vec<Comparator<CalendarEventSort>> =
            Comparator::parse_list(value).unwrap();
        let sort = check_sort(&comparators).map_err(|e| e.kind())?;
        let mut found: Vec<&Hit> = hits.iter().collect();
        found.sort_by(|a, b| compare(a, b, &sort));
        Ok(found.iter().map(|h| h.resource.get()).collect())
    }

    /// RFC 8620 §5.5: `uid` sorts with the comparator's collation
    /// (case-insensitive by default); dates ignore it; one the session
    /// doesn't offer is `unsupportedSort` (also on a date).
    #[test]
    fn sorts_honour_collations() {
        let hits = [
            hit(1, 30, Some("beta")),
            hit(2, 10, Some("Alpha")),
            hit(3, 20, None),
            hit(4, 40, Some("10")),
            hit(5, 50, Some("9")),
        ];
        assert_eq!(
            order(&hits, json!([{"property": "uid"}])),
            Ok(vec![3, 4, 5, 2, 1])
        );
        assert_eq!(
            order(
                &hits,
                json!([{"property": "uid", "collation": "i;ascii-numeric"}])
            ),
            Ok(vec![3, 5, 4, 1, 2])
        );
        assert_eq!(
            order(
                &hits,
                json!([{"property": "start", "collation": "i;ascii-casemap"}])
            ),
            Ok(vec![2, 3, 1, 4, 5])
        );
        for property in ["uid", "start"] {
            assert_eq!(
                order(
                    &hits,
                    json!([{"property": property, "collation": "i;octet"}])
                ),
                Err(MethodErrorType::UnsupportedSort)
            );
        }
        assert_eq!(
            order(&hits, json!([{"property": "created"}])),
            Err(MethodErrorType::UnsupportedSort)
        );
    }
}
