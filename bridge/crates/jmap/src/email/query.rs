// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/query` (RFC 8621 §4.4) and `Email/queryChanges`.
//!
//! Folder, keyword, date, size and attachment conditions run on the local
//! metadata. Text conditions (`text`, `from`, `to`, `cc`, `bcc`,
//! `subject`, `body`) run on the IMAP server (`UID SEARCH`) and come back
//! as a list of ids that the local query intersects. A condition we can't
//! answer exactly (`header`) is `unsupportedFilter`, never a partial
//! result.
//!
//! `Email/queryChanges` (RFC 8620 §5.6) uses the Email change log: every
//! email created, updated or destroyed since `sinceQueryState` may have
//! entered, left or moved in the results (all sort keys are immutable, but
//! keywords and mailboxes are filters). Those ids go to `removed` (except
//! new ones, which weren't in the old results), and the ones in the
//! current results go to `added` with their index; the RFC allows such
//! extra ids. When a result depends on the other emails of the thread
//! (`collapseThreads`, `*InThread*` filters) the changed emails' threads
//! are included. A destroyed email's thread is no longer known, but the
//! threads it left were logged in the same change: the Thread log since
//! the matching state (`Store::state_alongside`) gives them, maybe with a
//! few changed later (extra ids, allowed). Only when that log no longer
//! goes back far enough is it `cannotCalculateChanges`. `upToId` is
//! ignored (allowed).

use std::collections::BTreeSet;

use nebula_jmap_proto::method::{
    AddedItem, Comparator, Filter, Operator, QueryChangesResponse, QueryResponse,
};
use nebula_jmap_proto::object::email::{
    EmailFilterCondition, EmailQueryArguments, EmailQueryChangesArguments, EmailSort,
};
use nebula_jmap_proto::{Keyword, MethodError, MethodErrorType, MethodResponse, UnsignedInt};
use nebula_mail::{TextCondition, TextField};
use nebula_store::{
    ChangesSince, DataType, Direction, EmailFilter, EmailId, EmailQuery, MailboxId, QueryStart,
    Seq, SortKey, StoreError, ThreadId, UnixTime,
};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::CallError;

/// The sort properties we support (`emailQuerySortOptions`).
pub(crate) const SORT_OPTIONS: &[&str] = &["receivedAt", "sentAt", "size", "subject", "from"];

/// Most ids in one page, whatever the client asks for.
const MAX_LIMIT: u64 = 1_000;

/// Most changed emails `Email/queryChanges` works through; beyond that the
/// client reloads (`cannotCalculateChanges`), or gets `tooManyChanges` if
/// it set a smaller `maxChanges`.
const MAX_QUERY_CHANGES: usize = 10_000;

/// A filter with the server-side searches still to run.
#[derive(Debug)]
enum Node {
    Local(EmailFilter),
    /// Index into the list of searches.
    Search(usize),
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Vec<Node>),
}

/// One `UID SEARCH`: in one mailbox, or in all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Search {
    scope: Option<MailboxId>,
    conditions: Vec<TextCondition>,
}

/// Most distinct `UID SEARCH`es one filter may need: each one runs on the
/// IMAP server, over every mailbox when it has no scope. Repeated
/// conditions share one search. More is `unsupportedFilter`. The filter
/// preview of the web (`settings/filters/preview.ts`) needs one per rule
/// condition, two for a recipient.
const MAX_TEXT_SEARCHES: usize = 32;

/// Most words or phrases of one `text` value that are searched (Stalwart
/// 1.0 keeps the first 8 too).
pub(super) const MAX_TEXT_TERMS: usize = 8;

/// The phrase at the start of `text` (which starts with `quote`) and what
/// follows it, if it closes before a space or the end: `"a b" c`.
fn split_phrase(text: &str, quote: char) -> Option<(&str, &str)> {
    let body = text.strip_prefix(quote)?;
    body.match_indices(quote).find_map(|(at, _)| {
        let phrase = body.get(..at)?;
        let rest = body.get(at..)?.strip_prefix(quote)?;
        rest.chars()
            .next()
            .is_none_or(char::is_whitespace)
            .then_some((phrase, rest))
    })
}

/// The words and quoted phrases of a `text` value, as Stalwart 1.0 reads
/// it (`email/query/text.rs`): each one must be somewhere in the message,
/// not the whole value in one place. A phrase is searched without its
/// quotes, as one IMAP string.
fn text_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut rest = text.trim_start();
    while let Some(first) = rest.chars().next() {
        if terms.len() == MAX_TEXT_TERMS {
            break;
        }
        let (term, next) = matches!(first, '"' | '\'')
            .then(|| split_phrase(rest, first))
            .flatten()
            .unwrap_or_else(|| rest.split_once(char::is_whitespace).unwrap_or((rest, "")));
        let term = term.trim();
        if !term.is_empty() {
            terms.push(term.to_owned());
        }
        rest = next.trim_start();
    }
    terms
}

fn text_conditions(condition: &EmailFilterCondition) -> Vec<TextCondition> {
    [
        (TextField::Text, &condition.text),
        (TextField::From, &condition.from),
        (TextField::To, &condition.to),
        (TextField::Cc, &condition.cc),
        (TextField::Bcc, &condition.bcc),
        (TextField::Subject, &condition.subject),
        (TextField::Body, &condition.body),
    ]
    .into_iter()
    .flat_map(|(field, value)| {
        let value = value.as_deref().map_or("", str::trim);
        let values = match field {
            TextField::Text => text_terms(value),
            _ if value.is_empty() => Vec::new(),
            _ => vec![value.to_owned()],
        };
        values
            .into_iter()
            .map(move |value| TextCondition { field, value })
    })
    .collect()
}

fn time(date: nebula_jmap_proto::UtcDate) -> UnixTime {
    UnixTime::from_secs(date.unix_timestamp())
}

fn local_conditions(condition: &EmailFilterCondition) -> Vec<EmailFilter> {
    let mut out = Vec::new();
    if let Some(mailbox) = &condition.in_mailbox {
        out.push(match ids::store_mailbox(mailbox) {
            Some(row) => EmailFilter::InMailbox(row),
            // An id that isn't ours: no email is in it.
            None => EmailFilter::Ids(Vec::new()),
        });
    }
    if let Some(list) = &condition.in_mailbox_other_than {
        out.push(EmailFilter::InMailboxOtherThan(
            list.iter().filter_map(ids::store_mailbox).collect(),
        ));
    }
    if let Some(date) = condition.before {
        out.push(EmailFilter::Before(time(date)));
    }
    if let Some(date) = condition.after {
        out.push(EmailFilter::After(time(date)));
    }
    if let Some(size) = condition.min_size {
        out.push(EmailFilter::MinSize(size.get()));
    }
    if let Some(size) = condition.max_size {
        out.push(EmailFilter::MaxSize(size.get()));
    }
    let keyword_filters: [(&Option<Keyword>, MakeFilter); 5] = [
        (&condition.has_keyword, EmailFilter::HasKeyword),
        (&condition.not_keyword, EmailFilter::NotKeyword),
        (
            &condition.all_in_thread_have_keyword,
            EmailFilter::AllInThreadHaveKeyword,
        ),
        (
            &condition.some_in_thread_have_keyword,
            EmailFilter::SomeInThreadHaveKeyword,
        ),
        (
            &condition.none_in_thread_have_keyword,
            EmailFilter::NoneInThreadHaveKeyword,
        ),
    ];
    for (keyword, make) in keyword_filters {
        if let Some(keyword) = keyword {
            out.push(make(keyword.clone()));
        }
    }
    if let Some(has) = condition.has_attachment {
        out.push(EmailFilter::HasAttachment(has));
    }
    out
}

/// Builds a keyword condition.
type MakeFilter = fn(Keyword) -> EmailFilter;

/// The mailbox an AND of these filters is limited to, if any: text
/// searches below it only need to look there.
fn and_scope(conditions: &[Filter<EmailFilterCondition>]) -> Option<MailboxId> {
    conditions.iter().find_map(|filter| match filter {
        Filter::Condition(condition) => condition.in_mailbox.as_ref().and_then(ids::store_mailbox),
        _ => None,
    })
}

fn translate(
    filter: &Filter<EmailFilterCondition>,
    scope: Option<MailboxId>,
    searches: &mut Vec<Search>,
) -> Result<Node, MethodError> {
    match filter {
        Filter::Condition(condition) => {
            if condition.header.is_some() {
                return Err(MethodError::unsupported_filter(
                    "the header condition is not supported",
                ));
            }
            let mut parts: Vec<Node> = local_conditions(condition)
                .into_iter()
                .map(Node::Local)
                .collect();
            let text = text_conditions(condition);
            if !text.is_empty() {
                let scope = condition
                    .in_mailbox
                    .as_ref()
                    .and_then(ids::store_mailbox)
                    .or(scope);
                let search = Search {
                    scope,
                    conditions: text,
                };
                let index = match searches.iter().position(|known| *known == search) {
                    Some(index) => index,
                    None if searches.len() >= MAX_TEXT_SEARCHES => {
                        return Err(MethodError::unsupported_filter(
                            "too many different text conditions",
                        ));
                    }
                    None => {
                        searches.push(search);
                        searches.len().saturating_sub(1)
                    }
                };
                parts.push(Node::Search(index));
            }
            Ok(Node::And(parts))
        }
        Filter::Operator(operator) => {
            let scope = match operator.operator {
                Operator::And => and_scope(&operator.conditions).or(scope),
                _ => scope,
            };
            let children = operator
                .conditions
                .iter()
                .map(|child| translate(child, scope, searches))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match operator.operator {
                Operator::And => Node::And(children),
                Operator::Or => Node::Or(children),
                Operator::Not => Node::Not(children),
                _ => return Err(MethodError::unsupported_filter("unknown operator")),
            })
        }
        _ => Err(MethodError::unsupported_filter("unknown filter")),
    }
}

/// The store filter, with each search replaced by what it found. A search
/// can be shared by several nodes (repeated conditions), so each one gets
/// its own copy.
fn resolve(node: Node, results: &[Vec<EmailId>]) -> EmailFilter {
    let all = |nodes: Vec<Node>, results: &[Vec<EmailId>]| {
        nodes
            .into_iter()
            .map(|n| resolve(n, results))
            .collect::<Vec<_>>()
    };
    match node {
        Node::Local(filter) => filter,
        Node::Search(index) => EmailFilter::Ids(results.get(index).cloned().unwrap_or_default()),
        Node::And(nodes) => EmailFilter::And(all(nodes, results)),
        Node::Or(nodes) => EmailFilter::Or(all(nodes, results)),
        Node::Not(nodes) => EmailFilter::Not(all(nodes, results)),
    }
}

/// The store filter, running the text searches on the server.
pub(crate) async fn store_filter(
    ctx: &Ctx<'_>,
    filter: Option<&Filter<EmailFilterCondition>>,
) -> Result<Option<EmailFilter>, CallError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let mut searches = Vec::new();
    let node = translate(filter, None, &mut searches)?;
    let mut results = Vec::with_capacity(searches.len());
    if !searches.is_empty() {
        let mail = ctx.mail()?;
        for search in &searches {
            let found = mail.search(search.scope, &search.conditions).await?;
            results.push(found.into_iter().collect());
        }
    }
    Ok(Some(resolve(node, &results)))
}

fn sort_key(comparator: &Comparator<EmailSort>) -> Result<nebula_store::EmailSort, MethodError> {
    let key = match comparator.property {
        EmailSort::ReceivedAt => SortKey::ReceivedAt,
        EmailSort::SentAt => SortKey::SentAt,
        EmailSort::Size => SortKey::Size,
        EmailSort::Subject => SortKey::Subject,
        EmailSort::From => SortKey::From,
        ref other => {
            return Err(MethodError::unsupported_sort(format!(
                "cannot sort by {}",
                other.as_str()
            )));
        }
    };
    // RFC 8620 §5.5: a collation the session doesn't offer is
    // `unsupportedSort`. The stored subject and sender keys are folded
    // for `i;unicode-casemap` (the default) and can't be compared any
    // other way, so another collation for them is `unsupportedSort` too
    // (it has no effect on dates and sizes).
    let collate = crate::collation::Collate::of(comparator.collation)?;
    if matches!(key, SortKey::Subject | SortKey::From)
        && collate != crate::collation::Collate::UnicodeCasemap
    {
        return Err(MethodError::unsupported_sort(format!(
            "{} sorts only with i;unicode-casemap",
            comparator.property.as_str()
        )));
    }
    Ok(nebula_store::EmailSort {
        key,
        direction: if comparator.is_ascending {
            Direction::Ascending
        } else {
            Direction::Descending
        },
    })
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    args: EmailQueryArguments,
) -> Result<MethodResponse, CallError> {
    let query = args.query;
    ctx.check_account(&query.account_id)?;
    let sort = query
        .sort
        .iter()
        .map(sort_key)
        .collect::<Result<Vec<_>, _>>()?;
    let state = ctx.state(DataType::Email).await?;
    let filter = store_filter(ctx, query.filter.as_ref()).await?;
    let asked = query.limit.map(UnsignedInt::get);
    let limit = asked.map_or(MAX_LIMIT, |n| n.min(MAX_LIMIT));
    let start = match &query.anchor {
        Some(anchor) => QueryStart::Anchor {
            id: ids::store_email(anchor).ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
            offset: query.anchor_offset.get(),
        },
        None => QueryStart::Position(query.position.get()),
    };
    let result = ctx
        .store()
        .query_emails(
            ctx.id(),
            EmailQuery {
                filter,
                sort,
                collapse_threads: args.collapse_threads,
                start,
                limit: Some(limit),
            },
        )
        .await
        .map_err(|error| match error {
            StoreError::NotFound("anchor") => MethodErrorType::AnchorNotFound.into(),
            other => CallError::from(other),
        })?;
    Ok(MethodResponse::EmailQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: state,
        can_calculate_changes: true,
        position: UnsignedInt::saturating(result.position),
        ids: result
            .ids
            .into_iter()
            .map(ids::email_id)
            .collect::<Result<_, _>>()?,
        total: query
            .calculate_total
            .then(|| UnsignedInt::saturating(result.total)),
        limit: (asked != Some(limit)).then(|| UnsignedInt::saturating(limit)),
    }))
}

/// Whether an email being in the results depends on the other emails
/// of its thread.
fn depends_on_thread(filter: Option<&EmailFilter>) -> bool {
    match filter {
        Some(
            EmailFilter::AllInThreadHaveKeyword(_)
            | EmailFilter::SomeInThreadHaveKeyword(_)
            | EmailFilter::NoneInThreadHaveKeyword(_),
        ) => true,
        Some(EmailFilter::And(list) | EmailFilter::Or(list) | EmailFilter::Not(list)) => {
            list.iter().any(|filter| depends_on_thread(Some(filter)))
        }
        _ => false,
    }
}

/// All visible emails of the threads of `emails`.
async fn thread_mates(
    ctx: &Ctx<'_>,
    emails: &BTreeSet<EmailId>,
) -> Result<BTreeSet<EmailId>, CallError> {
    let mut mates = BTreeSet::new();
    if emails.is_empty() {
        return Ok(mates);
    }
    let found = ctx
        .store()
        .emails(ctx.id(), emails.iter().copied().collect())
        .await?;
    let threads: Vec<_> = found
        .iter()
        .map(|email| email.thread)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for (_, list) in ctx.store().thread_emails(ctx.id(), threads).await? {
        mates.extend(list);
    }
    Ok(mates)
}

/// The emails of the changed emails' threads; with `destroyed` (an email
/// was destroyed since that Email state, with that limit), also those of
/// the threads changed since then: a destroyed email's own thread is gone.
async fn mates_of(
    ctx: &Ctx<'_>,
    touched: &BTreeSet<EmailId>,
    destroyed: Option<(Seq, usize)>,
) -> Result<BTreeSet<EmailId>, CallError> {
    let mut mates = thread_mates(ctx, touched).await?;
    if let Some((since, max_changes)) = destroyed {
        let threads = changed_threads(ctx, since, max_changes)
            .await?
            .ok_or_else(|| CallError::from(MethodErrorType::CannotCalculateChanges))?;
        for (_, list) in ctx.store().thread_emails(ctx.id(), threads).await? {
            mates.extend(list);
        }
    }
    Ok(mates)
}

/// The threads changed since the Email state `since` (and maybe a few
/// more, changed later), or `None` when the log no longer tells: too
/// many, or trimmed.
async fn changed_threads(
    ctx: &Ctx<'_>,
    since: Seq,
    max_changes: usize,
) -> Result<Option<Vec<ThreadId>>, CallError> {
    let Some(thread_since) = ctx
        .store()
        .state_alongside(ctx.id(), DataType::Email, since, DataType::Thread)
        .await?
    else {
        return Ok(None);
    };
    let ChangesSince::Changes(log) = ctx
        .store()
        .changes_since(ctx.id(), DataType::Thread, thread_since, max_changes)
        .await?
    else {
        return Ok(None);
    };
    if log.has_more_changes {
        return Ok(None);
    }
    Ok(Some(
        log.created
            .iter()
            .chain(&log.updated)
            .chain(&log.destroyed)
            .filter_map(|&raw| ThreadId::from_raw(raw))
            .collect(),
    ))
}

/// `Email/queryChanges` (see the module comment).
pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: EmailQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    let collapse_threads = args.collapse_threads;
    let args = args.query_changes;
    ctx.check_account(&args.account_id)?;
    let cannot = || CallError::from(MethodErrorType::CannotCalculateChanges);
    let since = ids::parse_state(&args.since_query_state).ok_or_else(cannot)?;
    let sort = args
        .sort
        .iter()
        .map(sort_key)
        .collect::<Result<Vec<_>, _>>()?;
    let max_changes = args.max_changes.map_or(MAX_QUERY_CHANGES, |n| {
        n.get().to_usize_saturating().min(MAX_QUERY_CHANGES)
    });
    let found = ctx
        .store()
        .changes_since(ctx.id(), DataType::Email, since, max_changes)
        .await?;
    let log = match found {
        ChangesSince::Changes(log) => log,
        // RFC 8620 §5.6: over the client's `maxChanges`.
        ChangesSince::TooMany if args.max_changes.is_some() => {
            return Err(MethodErrorType::TooManyChanges.into());
        }
        ChangesSince::TooMany | ChangesSince::CannotCalculate => return Err(cannot()),
    };
    let count = log
        .created
        .len()
        .saturating_add(log.updated.len())
        .saturating_add(log.destroyed.len());
    if log.has_more_changes || count > max_changes {
        return Err(if args.max_changes.is_some() {
            MethodErrorType::TooManyChanges.into()
        } else {
            cannot()
        });
    }
    let filter = store_filter(ctx, args.filter.as_ref()).await?;
    let by_thread = collapse_threads || depends_on_thread(filter.as_ref());
    let to_ids = |list: &[i64]| -> BTreeSet<EmailId> {
        list.iter()
            .filter_map(|&row| EmailId::from_raw(row))
            .collect()
    };
    let created = to_ids(&log.created);
    let mut touched = created.clone();
    touched.extend(to_ids(&log.updated));
    touched.extend(to_ids(&log.destroyed));
    // Emails whose place depends on the changed ones: their thread mates.
    let mates = if by_thread {
        let destroyed = !log.destroyed.is_empty();
        mates_of(ctx, &touched, destroyed.then_some((since, max_changes))).await?
    } else {
        BTreeSet::new()
    };
    let results = ctx
        .store()
        .query_emails(
            ctx.id(),
            EmailQuery {
                filter,
                sort,
                collapse_threads,
                start: QueryStart::Position(0),
                limit: None,
            },
        )
        .await?;
    // New emails weren't in the old results (unless they are the mate of
    // another changed one: then they may have been).
    let removed: BTreeSet<EmailId> = touched
        .iter()
        .chain(mates.iter())
        .filter(|id| !created.contains(id) || mates.contains(id))
        .copied()
        .collect();
    let mut added = Vec::new();
    for (index, id) in results.ids.iter().enumerate() {
        if touched.contains(id) || mates.contains(id) {
            added.push(AddedItem {
                id: ids::email_id(*id)?,
                index: UnsignedInt::saturating(u64::try_from(index).unwrap_or(u64::MAX)),
            });
        }
    }
    Ok(MethodResponse::EmailQueryChanges(QueryChangesResponse {
        account_id: ctx.jmap_id(),
        old_query_state: args.since_query_state,
        new_query_state: ids::state(log.new_state),
        total: args
            .calculate_total
            .then(|| UnsignedInt::saturating(results.total)),
        removed: removed
            .into_iter()
            .map(ids::email_id)
            .collect::<Result<_, _>>()?,
        added,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: serde_json::Value) -> Filter<EmailFilterCondition> {
        Filter::parse(value).unwrap()
    }

    #[test]
    fn local_and_text_conditions() {
        let filter = parse(json!({"operator": "AND", "conditions": [
            {"inMailbox": "m3"}, {"text": "informe", "from": "ana"}, {"notKeyword": "$seen"}
        ]}));
        let mut searches = Vec::new();
        let node = translate(&filter, None, &mut searches).unwrap();
        assert_eq!(searches.len(), 1);
        // The search is limited to the mailbox of the AND.
        assert_eq!(searches[0].scope, MailboxId::from_raw(3));
        assert_eq!(searches[0].conditions.len(), 2);
        let found = vec![EmailId::from_raw(9).unwrap()];
        let resolved = resolve(node, std::slice::from_ref(&found));
        let EmailFilter::And(parts) = resolved else {
            panic!("not an AND")
        };
        assert_eq!(parts.len(), 3);
        assert!(
            matches!(&parts[1], EmailFilter::And(inner) if inner == &[EmailFilter::Ids(found)])
        );
    }

    #[test]
    fn text_is_split_into_words_and_phrases() {
        assert_eq!(
            text_terms(r#"  informe "plan anual" 'q3 2026' O'Brien "sin cerrar"#),
            [
                "informe",
                "plan anual",
                "q3 2026",
                "O'Brien",
                "\"sin",
                "cerrar"
            ]
        );
        assert_eq!(text_terms(r#""" '' x"#), ["x"]);
        assert!(text_terms("   ").is_empty());
        assert_eq!(text_terms("a b c d e f g h i j").len(), MAX_TEXT_TERMS);
        let filter = parse(json!({"text": "informe \"plan anual\"", "from": "ana"}));
        let mut searches = Vec::new();
        translate(&filter, None, &mut searches).unwrap();
        let values: Vec<_> = searches[0]
            .conditions
            .iter()
            .map(|c| (c.field, c.value.as_str()))
            .collect();
        assert_eq!(
            values,
            [
                (TextField::Text, "informe"),
                (TextField::Text, "plan anual"),
                (TextField::From, "ana"),
            ]
        );
    }

    #[test]
    fn repeated_text_conditions_share_one_search() {
        let same = |count: usize| {
            let conditions: Vec<_> = (0..count).map(|_| json!({"subject": "a"})).collect();
            parse(json!({"operator": "OR", "conditions": conditions}))
        };
        let mut searches = Vec::new();
        translate(&same(40), None, &mut searches).unwrap();
        assert_eq!(searches.len(), 1);
        let different = |count: usize| {
            let conditions: Vec<_> = (0..count)
                .map(|n| json!({"subject": format!("s{n}")}))
                .collect();
            parse(json!({"operator": "OR", "conditions": conditions}))
        };
        let mut searches = Vec::new();
        translate(&different(MAX_TEXT_SEARCHES), None, &mut searches).unwrap();
        assert_eq!(searches.len(), MAX_TEXT_SEARCHES);
        let error =
            translate(&different(MAX_TEXT_SEARCHES + 1), None, &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), MethodErrorType::UnsupportedFilter);
    }

    #[test]
    fn a_shared_search_reaches_every_node() {
        // Same subject, different local conditions: one search, two nodes.
        let filter = parse(json!({"operator": "OR", "conditions": [
            {"subject": "a", "hasKeyword": "$seen"},
            {"subject": "a", "notKeyword": "$seen"}
        ]}));
        let mut searches = Vec::new();
        let node = translate(&filter, None, &mut searches).unwrap();
        assert_eq!(searches.len(), 1);
        let found = vec![EmailId::from_raw(4).unwrap(), EmailId::from_raw(7).unwrap()];
        let EmailFilter::Or(branches) = resolve(node, std::slice::from_ref(&found)) else {
            panic!("not an OR")
        };
        assert_eq!(branches.len(), 2);
        for branch in branches {
            let EmailFilter::And(parts) = branch else {
                panic!("not an AND")
            };
            assert_eq!(parts.last(), Some(&EmailFilter::Ids(found.clone())));
        }
    }

    /// The widest filter `Filter::parse` lets through, each condition with
    /// many properties, still runs in the store: the store's own cap
    /// (a defence) counts the translated nodes, several per condition.
    #[tokio::test]
    async fn the_widest_filter_runs_in_the_store() {
        let conditions: Vec<_> = (0..nebula_jmap_proto::method::MAX_FILTER_NODES - 1)
            .map(|n| {
                json!({
                    "inMailbox": format!("m{}", n + 1),
                    "inMailboxOtherThan": ["m1", "m2"],
                    "before": "2030-01-01T00:00:00Z",
                    "after": "2000-01-01T00:00:00Z",
                    "minSize": 1, "maxSize": 1_000_000,
                    "hasKeyword": "$seen", "notKeyword": "$draft",
                    "allInThreadHaveKeyword": "$flagged",
                    "someInThreadHaveKeyword": "$answered",
                    "noneInThreadHaveKeyword": "$junk",
                    "hasAttachment": false
                })
            })
            .collect();
        let filter = parse(json!({"operator": "OR", "conditions": conditions}));
        let mut searches = Vec::new();
        let node = translate(&filter, None, &mut searches).unwrap();
        assert!(searches.is_empty());
        let filter = resolve(node, &[]);
        let dir = tempfile::tempdir().unwrap();
        let store = nebula_store::Store::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        let account = nebula_store::AccountId::from_raw(1).unwrap();
        let result = store
            .query_emails(
                account,
                EmailQuery {
                    filter: Some(filter),
                    sort: Vec::new(),
                    collapse_threads: false,
                    start: QueryStart::Position(0),
                    limit: Some(10),
                },
            )
            .await
            .unwrap();
        assert!(result.ids.is_empty());
    }

    #[test]
    fn thread_dependent_filters() {
        let keyword = Keyword::seen();
        assert!(!depends_on_thread(None));
        assert!(!depends_on_thread(Some(&EmailFilter::HasKeyword(
            keyword.clone()
        ))));
        assert!(depends_on_thread(Some(&EmailFilter::Not(vec![
            EmailFilter::And(vec![EmailFilter::SomeInThreadHaveKeyword(keyword)])
        ]))));
    }

    #[test]
    fn unsupported() {
        let filter = parse(json!({"header": ["X-Spam"]}));
        let error = translate(&filter, None, &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), MethodErrorType::UnsupportedFilter);
        let comparator = Comparator {
            property: EmailSort::To,
            is_ascending: true,
            collation: None,
        };
        assert_eq!(
            sort_key(&comparator).unwrap_err().kind(),
            MethodErrorType::UnsupportedSort
        );
    }

    /// RFC 8620 §5.5: a collation is honoured or `unsupportedSort`, never
    /// accepted and ignored.
    #[test]
    fn collations() {
        use nebula_jmap_proto::method::Collation;
        let sort = |property, collation| {
            sort_key(&Comparator {
                property,
                is_ascending: true,
                collation,
            })
            .map_err(|error| error.kind())
        };
        assert!(sort(EmailSort::Subject, None).is_ok());
        assert!(sort(EmailSort::Subject, Some(Collation::UnicodeCasemap)).is_ok());
        for other in [
            Collation::Octet,
            Collation::AsciiCasemap,
            Collation::AsciiNumeric,
        ] {
            assert_eq!(
                sort(EmailSort::From, Some(other)),
                Err(MethodErrorType::UnsupportedSort)
            );
        }
        // Dates and sizes don't compare text: an offered collation does
        // nothing, one not offered is still refused.
        assert!(sort(EmailSort::ReceivedAt, Some(Collation::AsciiNumeric)).is_ok());
        assert_eq!(
            sort(EmailSort::Size, Some(Collation::Octet)),
            Err(MethodErrorType::UnsupportedSort)
        );
    }
}
