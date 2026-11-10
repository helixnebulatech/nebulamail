// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Principal/query` (RFC 9670 §3.3) over what the DAV server shows.
//!
//! The candidates are the user's own principal, the principals found for
//! each `email` in the filter (`nebula_groupware`: an address search or
//! the listing) and, unless the filter only selects by `email`, the
//! server's whole listing. The filter is then evaluated here: `email`
//! exact without case (as Stalwart), `name` and `text` contained without
//! case, `type` exact, `accountIds` the user's own account, `timeZone` the
//! principal's (see `Principal/get`: from their calendars; unknown never
//! matches). Default order: as the
//! server lists them (own first); `sort` by `name`, `email` or `type`.
//!
//! Two additions for servers that do not show everyone:
//!
//! - **By name on the server:** for each `text` or `name` (2 to 64
//!   characters, at most [`MAX_TEXTS`]), a search of `displayname` in
//!   the principal collections the server would not list (Nextcloud: its
//!   groups have no address, so the "listing" misses them). The web asks
//!   this as the user types in the share dialog.
//! - **Addresses without a principal:** on a server that shares by
//!   address (sabre/dav, Apple; Baïkal shows nobody else), an `email`
//!   the server knows no principal for is a candidate of its own
//!   (`super::Who::Address`), so the web can share with it.

use std::cmp::Ordering;
use std::collections::HashSet;

use nebula_dav::proto::CalendarAddress;
use nebula_groupware::GroupwareAccount;
use nebula_groupware::principals::PrincipalZones;
use nebula_jmap_proto::method::{Filter, FilterOperator, Operator, QueryResponse};
use nebula_jmap_proto::object::{PrincipalFilterCondition, PrincipalQueryArguments, PrincipalSort};
use nebula_jmap_proto::{MethodError, MethodResponse, UnsignedInt};

use super::{Who, ids};
use crate::collation::CheckedSort;
use crate::core::context::Ctx;
use crate::error::CallError;
use crate::paging::page;

/// Most different addresses one query may look up (each may ask the
/// server).
const MAX_EMAILS: usize = 16;
/// Most different `text`/`name` values searched on the server.
const MAX_TEXTS: usize = 4;

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    args: PrincipalQueryArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let sort = CheckedSort::new(&args.sort)?;
    let running = super::account(ctx)?;
    let (emails, texts) = match &args.filter {
        Some(filter) => (
            collect_emails(filter, MAX_EMAILS + 1),
            collect_texts(filter, MAX_TEXTS),
        ),
        None => (Vec::new(), Vec::new()),
    };
    if emails.len() > MAX_EMAILS {
        return Err(MethodError::unsupported_filter(format!(
            "more than {MAX_EMAILS} email conditions"
        ))
        .into());
    }
    let mut candidates = candidates(ctx, &running, &args, &emails, &texts).await?;
    let zones = if args.filter.as_ref().is_some_and(names_time_zone) {
        running
            .principal_time_zones()
            .await
            .map_err(|e| super::groupware_error(&e))?
    } else {
        PrincipalZones::default()
    };
    // Terms in lower case once per query, and each candidate's name and
    // address once per candidate.
    let filter = args.filter.as_ref().map(lowered);
    candidates.retain(|principal| {
        filter.as_ref().is_none_or(|filter| {
            let text = CandidateText::of(ctx, principal);
            matches(ctx, &zones, principal, &text, filter)
        })
    });
    if !sort.is_empty() {
        candidates.sort_by(|a, b| compare(ctx, a, b, &sort));
    }
    let numbers: Vec<i64> = candidates.iter().map(Who::number).collect();
    let anchor = match &args.anchor {
        Some(id) => Some((
            ids::principal_number(id).ok_or(CallError::from(
                nebula_jmap_proto::MethodErrorType::AnchorNotFound,
            ))?,
            args.anchor_offset.get(),
        )),
        None => None,
    };
    let window = page(
        &numbers,
        args.position.get(),
        anchor,
        args.limit.map(UnsignedInt::get),
    )?;
    let ids = window
        .ids
        .into_iter()
        .map(ids::principal_id)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MethodResponse::PrincipalQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: super::state_of(ctx, &running.known_principals()),
        can_calculate_changes: false,
        position: UnsignedInt::saturating(window.position),
        ids,
        total: args
            .calculate_total
            .then(|| UnsignedInt::saturating(u64::try_from(numbers.len()).unwrap_or(u64::MAX))),
        limit: None,
    }))
}

/// Own first, then those found by address, then (if the filter needs it)
/// the listing and the name searches; without duplicates.
async fn candidates(
    ctx: &Ctx<'_>,
    running: &GroupwareAccount,
    args: &PrincipalQueryArguments,
    emails: &[String],
    texts: &[String],
) -> Result<Vec<Who>, CallError> {
    let mut all = Unique::default();
    if let Some(own) = running.own_principal() {
        all.add(Who::Dav(own));
    }
    for email in emails {
        let own = all
            .list
            .first()
            .is_some_and(|p| super::has_email(ctx, p, email));
        // An address that cannot be one matches nothing (no request).
        let Some(address) = CalendarAddress::parse(email).filter(|_| !own) else {
            continue;
        };
        match running
            .principal_for_address(&address)
            .await
            .map_err(|e| super::groupware_error(&e))?
        {
            Some(found) => all.add(Who::Dav(found)),
            None => {
                if running.address_principal(&address).is_some() {
                    all.add(Who::Address(address));
                }
            }
        }
    }
    let only_by_email = args.filter.as_ref().is_some_and(selects_by_email);
    if !only_by_email {
        for principal in running
            .principals()
            .await
            .map_err(|e| super::groupware_error(&e))?
        {
            all.add(Who::Dav(principal));
        }
        for text in texts {
            for principal in running
                .search_by_name(text)
                .await
                .map_err(|e| super::groupware_error(&e))?
            {
                all.add(Who::Dav(principal));
            }
        }
    }
    Ok(all.list)
}

/// Candidates in the order found, each once (by number, computed once).
#[derive(Default)]
struct Unique {
    list: Vec<Who>,
    seen: HashSet<i64>,
}

impl Unique {
    fn add(&mut self, who: Who) {
        if self.seen.insert(who.number()) {
            self.list.push(who);
        }
    }
}

/// The first `max` `text` and `name` values in the filter, without
/// duplicates.
fn collect_texts(filter: &Filter<PrincipalFilterCondition>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![filter];
    while let Some(next) = stack.pop() {
        if out.len() >= max {
            break;
        }
        match next {
            Filter::Condition(condition) => {
                for text in [&condition.text, &condition.name].into_iter().flatten() {
                    let text = text.trim();
                    if out.len() < max
                        && !text.is_empty()
                        && !out.iter().any(|known| known.eq_ignore_ascii_case(text))
                    {
                        out.push(text.to_owned());
                    }
                }
            }
            Filter::Operator(operator) => stack.extend(operator.conditions.iter()),
            _ => {}
        }
    }
    out
}

/// The first `max` `email` values in the filter, without duplicates (the
/// caller asks for one more than it accepts, to tell "too many").
fn collect_emails(filter: &Filter<PrincipalFilterCondition>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![filter];
    while let Some(next) = stack.pop() {
        if out.len() >= max {
            break;
        }
        match next {
            Filter::Condition(condition) => {
                if let Some(email) = &condition.email
                    && !out.iter().any(|known| known.eq_ignore_ascii_case(email))
                {
                    out.push(email.trim().to_owned());
                }
            }
            Filter::Operator(operator) => stack.extend(operator.conditions.iter()),
            _ => {}
        }
    }
    out
}

/// Every match of the filter has an address it names (so the lookups
/// are enough, no listing): a condition with `email`, an `AND` with one
/// such part, or an `OR` of only such parts.
fn selects_by_email(filter: &Filter<PrincipalFilterCondition>) -> bool {
    match filter {
        Filter::Condition(condition) => condition.email.is_some(),
        Filter::Operator(operator) => match operator.operator {
            Operator::And => operator.conditions.iter().any(selects_by_email),
            Operator::Or => {
                !operator.conditions.is_empty() && operator.conditions.iter().all(selects_by_email)
            }
            _ => false,
        },
        _ => false,
    }
}

/// The filter with its `name` and `text` terms trimmed and in lower case,
/// as [`CandidateText`] compares them.
fn lowered(filter: &Filter<PrincipalFilterCondition>) -> Filter<PrincipalFilterCondition> {
    match filter {
        Filter::Condition(condition) => {
            let mut condition = condition.clone();
            for term in [&mut condition.name, &mut condition.text]
                .into_iter()
                .flatten()
            {
                *term = term.trim().to_lowercase();
            }
            Filter::Condition(condition)
        }
        Filter::Operator(operator) => Filter::Operator(FilterOperator {
            operator: operator.operator,
            conditions: operator.conditions.iter().map(lowered).collect(),
        }),
        other => other.clone(),
    }
}

/// A candidate's name and address in lower case, computed once for the
/// whole filter.
struct CandidateText {
    name: String,
    email: String,
}

impl CandidateText {
    fn of(ctx: &Ctx<'_>, principal: &Who) -> Self {
        Self {
            name: super::name_of(ctx, principal).to_lowercase(),
            email: super::email_of(ctx, principal)
                .unwrap_or_default()
                .to_lowercase(),
        }
    }
}

/// Whether a `timeZone` condition is anywhere in the filter.
fn names_time_zone(filter: &Filter<PrincipalFilterCondition>) -> bool {
    let mut stack = vec![filter];
    while let Some(next) = stack.pop() {
        match next {
            Filter::Condition(condition) if condition.time_zone.is_some() => return true,
            Filter::Operator(operator) => stack.extend(operator.conditions.iter()),
            _ => {}
        }
    }
    false
}

fn matches(
    ctx: &Ctx<'_>,
    zones: &PrincipalZones,
    principal: &Who,
    text: &CandidateText,
    filter: &Filter<PrincipalFilterCondition>,
) -> bool {
    // Recursive: the parser limits filters to 16 levels and 256 nodes.
    match filter {
        Filter::Condition(condition) => condition_matches(ctx, zones, principal, text, condition),
        Filter::Operator(operator) => {
            let mut results = operator
                .conditions
                .iter()
                .map(|f| matches(ctx, zones, principal, text, f));
            match operator.operator {
                Operator::And => results.all(|r| r),
                Operator::Or => results.any(|r| r),
                _ => !results.any(|r| r),
            }
        }
        _ => false,
    }
}

fn condition_matches(
    ctx: &Ctx<'_>,
    zones: &PrincipalZones,
    principal: &Who,
    candidate: &CandidateText,
    condition: &PrincipalFilterCondition,
) -> bool {
    let PrincipalFilterCondition {
        account_ids,
        email,
        name,
        text,
        kind,
        time_zone,
    } = condition;
    account_ids
        .as_ref()
        .is_none_or(|accounts| principal.is_own() && accounts.contains(&ctx.jmap_id()))
        && email
            .as_ref()
            .is_none_or(|email| super::has_email(ctx, principal, email))
        && name
            .as_ref()
            .is_none_or(|name| candidate.name.contains(name.as_str()))
        && text.as_ref().is_none_or(|text| {
            candidate.name.contains(text.as_str()) || candidate.email.contains(text.as_str())
        })
        && kind.is_none_or(|kind| super::kind_of(principal) == kind)
        && time_zone.as_ref().is_none_or(|zone| {
            let found = match principal {
                Who::Dav(dav) => zones.of(principal.number(), dav.own),
                Who::Address(_) => None,
            };
            found.is_some_and(|found| found.eq_ignore_ascii_case(zone.trim()))
        })
}

/// `name` and `email` with the comparator's collation (RFC 8620 §5.5;
/// case-insensitive by default), `type` by its value; then by number.
fn compare(ctx: &Ctx<'_>, a: &Who, b: &Who, sort: &CheckedSort<'_, PrincipalSort>) -> Ordering {
    compare_by(
        sort,
        a,
        b,
        |who| super::name_of(ctx, who),
        |who| super::email_of(ctx, who).unwrap_or_default(),
    )
}

/// [`compare`] with the name and address of a principal given.
fn compare_by(
    sort: &CheckedSort<'_, PrincipalSort>,
    a: &Who,
    b: &Who,
    name: impl Fn(&Who) -> String,
    email: impl Fn(&Who) -> String,
) -> Ordering {
    sort.compare(a, b, |property, collate, a, b| match property {
        PrincipalSort::Name => collate.compare(&name(a), &name(b)),
        PrincipalSort::Email => collate.compare(&email(a), &email(b)),
        PrincipalSort::Type => super::kind_of(a).as_str().cmp(super::kind_of(b).as_str()),
        _ => Ordering::Equal,
    })
    .then_with(|| a.number().cmp(&b.number()))
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::method::Comparator;
    use nebula_jmap_proto::object::PrincipalQueryArguments;
    use serde_json::json;

    use super::*;

    fn filter(value: &serde_json::Value) -> Filter<PrincipalFilterCondition> {
        PrincipalQueryArguments::parse(
            json!({"accountId": "a", "filter": value})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap()
        .filter
        .unwrap()
    }

    /// The lists stop growing at their limit while walking the filter.
    #[test]
    fn collected_terms_are_bounded() {
        let many: Vec<_> = (0..200)
            .map(|i| json!({"email": format!("u{i}@example.com"), "text": format!("t{i}")}))
            .collect();
        let filter = filter(&json!({"operator": "OR", "conditions": many}));
        assert_eq!(
            collect_emails(&filter, MAX_EMAILS + 1).len(),
            MAX_EMAILS + 1
        );
        assert_eq!(collect_texts(&filter, MAX_TEXTS).len(), MAX_TEXTS);
        let lowered = lowered(&filter);
        let Filter::Operator(operator) = &lowered else {
            panic!("an operator")
        };
        assert_eq!(operator.conditions.len(), 200);
    }

    #[test]
    fn terms_are_lowered_once() {
        let Filter::Condition(condition) =
            lowered(&filter(&json!({"name": "  ÁNA ", "text": "Pérez"})))
        else {
            panic!("a condition")
        };
        assert_eq!(condition.name.as_deref(), Some("ána"));
        assert_eq!(condition.text.as_deref(), Some("pérez"));
    }

    /// RFC 8620 §5.5: `name` and `email` sort with the comparator's
    /// collation (case-insensitive by default); one the session doesn't
    /// offer is `unsupportedSort`.
    #[test]
    fn sorts_honour_collations() {
        let who = |user: &str| {
            Who::Address(CalendarAddress::parse(&format!("{user}@example.com")).unwrap())
        };
        let list = [who("zed"), who("Bob"), who("alice"), who("10"), who("9")];
        let names = ["zed", "Bob", "alice", "10", "9"];
        let name = |w: &Who| {
            list.iter()
                .position(|x| x.number() == w.number())
                .and_then(|i| names.get(i))
                .map(|n| (*n).to_owned())
                .unwrap_or_default()
        };
        let email = |w: &Who| format!("{}@example.com", name(w));
        let order = |value: serde_json::Value| -> Vec<&str> {
            let comparators: Vec<Comparator<PrincipalSort>> =
                Comparator::parse_list(value).unwrap();
            let sort = CheckedSort::new(&comparators).unwrap();
            let mut found: Vec<&Who> = list.iter().collect();
            found.sort_by(|a, b| compare_by(&sort, a, b, name, email));
            found
                .iter()
                .filter_map(|w| list.iter().position(|x| x.number() == w.number()))
                .filter_map(|i| names.get(i).copied())
                .collect()
        };
        assert_eq!(
            order(json!([{"property": "name"}])),
            ["10", "9", "alice", "Bob", "zed"]
        );
        assert_eq!(
            order(json!([{"property": "email", "isAscending": false}])),
            ["zed", "Bob", "alice", "9", "10"]
        );
        assert_eq!(
            order(json!([{"property": "name", "collation": "i;ascii-numeric"}]))[..2],
            ["9", "10"]
        );
        let octet: Vec<Comparator<PrincipalSort>> =
            Comparator::parse_list(json!([{"property": "name", "collation": "i;octet"}])).unwrap();
        assert_eq!(
            CheckedSort::new(&octet).err().map(|e| e.kind()),
            Some(nebula_jmap_proto::MethodErrorType::UnsupportedSort)
        );
    }

    #[test]
    fn candidates_are_unique_in_order() {
        let who = |user: &str| {
            Who::Address(CalendarAddress::parse(&format!("{user}@example.com")).unwrap())
        };
        let mut all = Unique::default();
        for user in ["ana", "bob", "ana", "bob"] {
            all.add(who(user));
        }
        let numbers: Vec<i64> = all.list.iter().map(Who::number).collect();
        assert_eq!(numbers, [who("ana").number(), who("bob").number()]);
    }
}
