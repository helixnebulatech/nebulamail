// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `ContactCard/query` (RFC 9610 §3.3). The web loads every card and
//! searches in the browser, so it only pages through all of them; other
//! clients may filter. `inAddressBook` and `uid` on the clear-text
//! indexes; every other filter of the RFC (`text`, `name`, `name/given`,
//! `name/surname`, `name/surname2`, `nickname`, `organization`, `email`,
//! `phone`, `onlineService`, `address`, `note`: substrings without case or
//! accents; `kind`, `hasMember`: exact; `created*`/`updated*`: the card's
//! `CREATED`/`REV`) over the opened copies, only when the filter has one
//! (`nebula_groupware::model::search`). With `AND`, `OR`, `NOT`. Any sort
//! is `unsupportedSort` (the order is by the sort name — surname, then
//! given name — then by id).
//! `ContactCard/queryChanges` from the change log.

use std::cmp::Ordering;
use std::sync::Arc;

use nebula_jmap_proto::method::{Comparator, Filter, Operator, QueryResponse};
use nebula_jmap_proto::object::{
    ContactCardFilterCondition, ContactCardQueryArguments, ContactCardQueryChangesArguments,
    ContactCardSort,
};
use nebula_jmap_proto::{MethodError, MethodErrorType, MethodResponse, UnsignedInt};
use nebula_store::{CollectionId, DataType, ResourceId, StoredResource};

use nebula_groupware::model::search::{CardField, CardText, Needle};

use super::card::{all_cards, books_by_id};
use super::ids;
use crate::changes::QueryLog;
use crate::core::context::Ctx;
use crate::error::CallError;

/// The conditions on a card's text (or kind, members and dates), with
/// what each compares, prepared once per query (needles built, terms
/// owned) so that the check per card does no setup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TextCondition {
    Field(CardField, Needle),
    Kind(String),
    Member(String),
    Created(Ordering, i64),
    Updated(Ordering, i64),
}

fn text_conditions(c: &ContactCardFilterCondition) -> Vec<TextCondition> {
    let mut out: Vec<TextCondition> = [
        (&c.text, CardField::Text),
        (&c.name, CardField::Name),
        (&c.name_given, CardField::Given),
        (&c.name_surname, CardField::Surname),
        (&c.name_surname2, CardField::Surname2),
        (&c.nickname, CardField::Nickname),
        (&c.organization, CardField::Organization),
        (&c.email, CardField::Email),
        (&c.phone, CardField::Phone),
        (&c.online_service, CardField::OnlineService),
        (&c.address, CardField::Address),
        (&c.note, CardField::Note),
    ]
    .into_iter()
    .filter_map(|(term, field)| {
        term.as_deref()
            .map(|term| TextCondition::Field(field, Needle::new(term)))
    })
    .collect();
    if let Some(kind) = &c.kind {
        out.push(TextCondition::Kind(kind.clone()));
    }
    if let Some(member) = &c.has_member {
        out.push(TextCondition::Member(member.clone()));
    }
    // `…Before`: strictly before; `…After`: at or after (RFC 9610 §3.3).
    let dates = [
        (&c.created_before, true, Ordering::Less),
        (&c.created_after, true, Ordering::Greater),
        (&c.updated_before, false, Ordering::Less),
        (&c.updated_after, false, Ordering::Greater),
    ];
    for (date, created, order) in dates {
        if let Some(date) = date {
            let at = date.unix_timestamp();
            out.push(if created {
                TextCondition::Created(order, at)
            } else {
                TextCondition::Updated(order, at)
            });
        }
    }
    out
}

fn text_matches(text: Option<&CardText>, condition: &TextCondition) -> bool {
    let Some(text) = text else {
        return false;
    };
    let compare = |at: Option<i64>, order: Ordering, date: i64| {
        at.is_some_and(|at| match order {
            Ordering::Less => at < date,
            _ => at >= date,
        })
    };
    match condition {
        TextCondition::Field(field, needle) => text.matches(*field, needle),
        TextCondition::Kind(kind) => text.is_kind(kind),
        TextCondition::Member(member) => text.has_member(member),
        TextCondition::Created(order, date) => compare(text.created, *order, *date),
        TextCondition::Updated(order, date) => compare(text.updated, *order, *date),
    }
}

/// One condition of the filter, prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Condition {
    text: Vec<TextCondition>,
    /// `inAddressBook`.
    in_address_book: BookCondition,
    uid: Option<String>,
}

/// An `inAddressBook` condition, prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BookCondition {
    /// No condition.
    Any,
    /// In this book.
    In(CollectionId),
    /// An id that names no book: nothing matches.
    NoBook,
}

/// The filter, prepared once per query (the parser bounds its size,
/// `MAX_FILTER_NODES`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Prepared {
    Condition(Condition),
    Operator(Operator, Vec<Prepared>),
    /// A filter kind this version doesn't know: matches nothing.
    Never,
}

impl Prepared {
    fn new(filter: &Filter<ContactCardFilterCondition>) -> Self {
        match filter {
            Filter::Condition(c) => Self::Condition(Condition {
                text: text_conditions(c),
                in_address_book: match &c.in_address_book {
                    None => BookCondition::Any,
                    Some(id) => {
                        ids::store_book(id).map_or(BookCondition::NoBook, BookCondition::In)
                    }
                },
                uid: c.uid.clone(),
            }),
            Filter::Operator(operator) => Self::Operator(
                operator.operator,
                operator.conditions.iter().map(Self::new).collect(),
            ),
            _ => Self::Never,
        }
    }

    /// Whether the filter needs the cards' text.
    fn needs_text(&self) -> bool {
        match self {
            Self::Condition(condition) => !condition.text.is_empty(),
            Self::Operator(_, filters) => filters.iter().any(Self::needs_text),
            Self::Never => false,
        }
    }
}

/// A card, and its text when the filter needs it.
struct Candidate {
    card: StoredResource,
    text: Option<CardText>,
}

fn condition_matches(candidate: &Candidate, condition: &Condition) -> bool {
    let card = &candidate.card;
    if !condition
        .text
        .iter()
        .all(|c| text_matches(candidate.text.as_ref(), c))
    {
        return false;
    }
    match condition.in_address_book {
        BookCondition::Any => {}
        BookCondition::In(book) if book == card.collection => {}
        BookCondition::In(_) | BookCondition::NoBook => return false,
    }
    if let Some(uid) = &condition.uid {
        let index = match &card.parsed {
            nebula_store::Parsed::Valid(index) => index.uid.as_deref(),
            nebula_store::Parsed::Invalid => None,
        };
        if index != Some(uid.as_str()) {
            return false;
        }
    }
    true
}

fn matches(card: &Candidate, filter: &Prepared) -> bool {
    match filter {
        Prepared::Condition(condition) => condition_matches(card, condition),
        Prepared::Operator(operator, filters) => {
            let mut results = filters.iter().map(|f| matches(card, f));
            match operator {
                Operator::And => results.all(|r| r),
                Operator::Or => results.any(|r| r),
                Operator::Not => !results.any(|r| r),
                _ => false,
            }
        }
        Prepared::Never => false,
    }
}

fn sort_name(card: &StoredResource) -> Option<&str> {
    match &card.parsed {
        nebula_store::Parsed::Valid(index) => index.sort_name.as_deref(),
        nebula_store::Parsed::Invalid => None,
    }
}

/// By sort name (cards without one last), then by id: stable pages.
fn compare(a: &StoredResource, b: &StoredResource) -> Ordering {
    match (sort_name(a), sort_name(b)) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then(a.id.cmp(&b.id))
}

/// Cards whose text is read, parsed and checked together: at most this
/// many clear texts are in memory at once.
const TEXT_BATCH: usize = 128;

/// The ids of the cards that match, in order (filter and sort checked).
async fn results(
    ctx: &Ctx<'_>,
    filter: Option<&Filter<ContactCardFilterCondition>>,
    sort: &[Comparator<ContactCardSort>],
) -> Result<Vec<ResourceId>, CallError> {
    if !sort.is_empty() {
        return Err(MethodError::unsupported_sort(
            "cards come sorted by name; other orders are not supported here",
        )
        .into());
    }
    let books = books_by_id(ctx).await?;
    let mut cards = all_cards(ctx, &books).await?;
    if let Some(filter) = filter {
        let filter = Arc::new(Prepared::new(filter));
        cards = if filter.needs_text() {
            matching_texts(ctx, cards, &filter).await?
        } else {
            blocking(move || {
                cards
                    .into_iter()
                    .map(|card| Candidate { card, text: None })
                    .filter(|candidate| matches(candidate, &filter))
                    .map(|candidate| candidate.card)
                    .collect()
            })
            .await?
        };
    }
    cards.sort_by(compare);
    Ok(cards.iter().map(|card| card.id).collect())
}

/// The cards that match a filter on their text, in order. The copies are
/// opened [`TEXT_BATCH`] at a time; each batch is parsed and checked off
/// the async threads and its texts dropped before the next (`None`: the
/// copy does not open or parse; no text condition matches it).
async fn matching_texts(
    ctx: &Ctx<'_>,
    cards: Vec<StoredResource>,
    filter: &Arc<Prepared>,
) -> Result<Vec<StoredResource>, CallError> {
    let running = super::dav_account(ctx)?;
    let mut out = Vec::new();
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut batch = Vec::with_capacity(TEXT_BATCH);
        for card in cards.by_ref().take(TEXT_BATCH) {
            let text = match running.resource_bytes(card.id).await {
                Ok(text) => text,
                Err(nebula_groupware::GroupwareError::Unreadable) => None,
                Err(error) => return Err(super::error::call_error(error)),
            };
            batch.push((card, text));
        }
        let filter = Arc::clone(filter);
        out.extend(
            blocking(move || {
                batch
                    .into_iter()
                    .map(|(card, text)| Candidate {
                        card,
                        text: text.and_then(|text| CardText::parse(&text).ok()),
                    })
                    .filter(|candidate| matches(candidate, &filter))
                    .map(|candidate| candidate.card)
                    .collect::<Vec<_>>()
            })
            .await?,
        );
    }
    Ok(out)
}

/// Runs CPU work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CallError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| crate::error::Bug("contacts task").into())
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    args: ContactCardQueryArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    super::dav_account(ctx)?;
    let state = ctx.state(DataType::ContactCard).await?;
    let all = results(ctx, args.filter.as_ref(), &args.sort).await?;
    let anchor = match &args.anchor {
        Some(anchor) => {
            Some(ids::store_card(anchor).ok_or(CallError::from(MethodErrorType::AnchorNotFound))?)
        }
        None => None,
    };
    let page = crate::paging::page(
        &all,
        args.position.get(),
        anchor.map(|a| (a, args.anchor_offset.get())),
        args.limit.map(UnsignedInt::get),
    )?;
    Ok(MethodResponse::ContactCardQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: state,
        can_calculate_changes: true,
        position: UnsignedInt::saturating(page.position),
        ids: page
            .ids
            .into_iter()
            .map(ids::card_id)
            .collect::<Result<_, _>>()?,
        total: args
            .calculate_total
            .then(|| UnsignedInt::try_from(all.len()).unwrap_or(UnsignedInt::MAX)),
        limit: None,
    }))
}

/// `ContactCard/queryChanges` (RFC 8620 §5.6) from the `ContactCard`
/// change log ([`crate::changes::QueryLog`]).
pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: ContactCardQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    super::dav_account(ctx)?;
    let log = QueryLog::since(
        ctx,
        DataType::ContactCard,
        &args.since_query_state,
        args.max_changes,
    )
    .await?;
    let current = results(ctx, args.filter.as_ref(), &args.sort)
        .await?
        .into_iter()
        .map(|id| Ok((id.get(), ids::card_id(id)?)))
        .collect::<Result<Vec<_>, crate::error::Bug>>()?;
    Ok(MethodResponse::ContactCardQueryChanges(log.response(
        ctx,
        args.since_query_state,
        &current,
        args.calculate_total,
        ids::card_row,
    )?))
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::object::ContactCardQueryArguments;
    use nebula_store::{DavHref, Parsed, ResourceIndex, UnixTime};
    use serde_json::json;

    use super::*;

    fn card(id: i64, book: i64, uid: &str, sort: Option<&str>) -> StoredResource {
        StoredResource {
            id: ResourceId::from_raw(id).unwrap(),
            collection: CollectionId::from_raw(book).unwrap(),
            href: DavHref::new(&format!("https://dav.example/b{book}/{id}.vcf")).unwrap(),
            etag: None,
            size: 1,
            parsed: Parsed::Valid(ResourceIndex {
                uid: Some(uid.to_owned()),
                sort_name: sort.map(str::to_owned),
                ..ResourceIndex::default()
            }),
            updated_at: UnixTime::from_secs(1),
            orphaned_at: None,
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "reads better with json! in the calls"
    )]
    fn filter(value: serde_json::Value) -> Prepared {
        let parsed = ContactCardQueryArguments::parse(
            json!({"accountId": "c", "filter": value})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap()
        .filter
        .unwrap();
        Prepared::new(&parsed)
    }

    #[test]
    fn filters() {
        let plain = |card| Candidate { card, text: None };
        let a = plain(card(1, 2, "u1", Some("pérez ana")));
        let b = plain(card(2, 3, "u2", None));
        let in_two = filter(json!({"inAddressBook": "a2"}));
        assert!(matches(&a, &in_two) && !matches(&b, &in_two));
        assert!(!in_two.needs_text());
        let uid = filter(json!({"operator": "OR", "conditions": [{"uid": "u2"}, {"uid": "x"}]}));
        assert!(!matches(&a, &uid) && matches(&b, &uid));
        let not = filter(json!({"operator": "NOT", "conditions": [{"inAddressBook": "a2"}]}));
        assert!(!matches(&a, &not) && matches(&b, &not));
    }

    /// The text filters, over the card's text (a card whose copy does not
    /// open matches none).
    #[test]
    fn text_filters() {
        let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u1\r\nFN:Ana Pérez\r\nN:Pérez;Ana;;;\r\n\
            EMAIL:ana@example.com\r\nTEL:+34 600 11 22 33\r\nREV:20261020T080000Z\r\nEND:VCARD\r\n";
        let ana = Candidate {
            card: card(1, 2, "u1", Some("pérez ana")),
            text: Some(CardText::parse(vcard.as_bytes()).unwrap()),
        };
        let unread = Candidate {
            card: card(2, 2, "u2", None),
            text: None,
        };
        for (value, expected) in [
            (json!({"text": "perez"}), true),
            (json!({"name/given": "ANA"}), true),
            (json!({"name/surname": "ana"}), false),
            (json!({"email": "example.com"}), true),
            (json!({"phone": "600112233"}), true),
            (json!({"kind": "individual"}), true),
            (json!({"kind": "group"}), false),
            (json!({"hasMember": "urn:uuid:x"}), false),
            (json!({"updatedAfter": "2026-10-20T08:00:00Z"}), true),
            (json!({"updatedBefore": "2026-10-20T08:00:00Z"}), false),
            (json!({"createdAfter": "2000-01-01T00:00:00Z"}), false),
            (json!({"inAddressBook": "a2", "note": "x"}), false),
            (
                json!({"operator": "NOT", "conditions": [{"text": "bob"}]}),
                true,
            ),
        ] {
            let filter = filter(value.clone());
            assert!(filter.needs_text(), "{value}");
            assert_eq!(matches(&ana, &filter), expected, "{value}");
            assert!(
                !matches(&unread, &filter) || value.get("operator").is_some(),
                "{value}"
            );
        }
        let unknown = ContactCardQueryArguments::parse(
            json!({"accountId": "c", "filter": {"surname": "x"}})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            unknown.unwrap_err().kind(),
            MethodErrorType::UnsupportedFilter
        );
    }

    #[test]
    fn order() {
        let mut cards = [
            card(3, 1, "c", None),
            card(2, 1, "b", Some("zeta")),
            card(1, 1, "a", Some("alfa")),
            card(4, 1, "d", None),
        ];
        cards.sort_by(compare);
        let order: Vec<i64> = cards.iter().map(|c| c.id.get()).collect();
        assert_eq!(order, [1, 2, 3, 4]);
    }
}
