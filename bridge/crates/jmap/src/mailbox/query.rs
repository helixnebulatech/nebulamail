// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Mailbox/query` (RFC 8621 §2.3), in memory. The filter is compiled
//! once per query and each mailbox name is put in lower case once, so the
//! cost is mailboxes × filter nodes, and the nodes are capped by
//! `Filter::parse` (`MAX_FILTER_NODES`).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use nebula_jmap_proto::method::{Comparator, Filter, Operator, QueryResponse};
use nebula_jmap_proto::object::{
    MailboxFilterCondition, MailboxQueryArguments, MailboxQueryChangesArguments, MailboxRole,
    MailboxSort,
};
use nebula_jmap_proto::{MethodError, MethodErrorType, MethodResponse, UnsignedInt};
use nebula_store::{DataType, Mailbox, MailboxId, Role};

use super::jmap_role;
use crate::changes::QueryLog;
use crate::collation::CheckedSort;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

/// What a condition looks at in a mailbox, with the name already in
/// lower case: done once per mailbox, not once per condition.
struct View {
    parent: Option<MailboxId>,
    name: String,
    role: Option<Role>,
    subscribed: bool,
}

impl View {
    fn of(mailbox: &Mailbox) -> Self {
        Self {
            parent: mailbox.parent,
            name: mailbox.name.to_lowercase(),
            role: mailbox.role,
            subscribed: mailbox.subscribed,
        }
    }
}

/// A condition ready to run: the name in lower case and the parent as a
/// row id, done once per query, not once per mailbox.
struct Condition {
    parent: Option<Parent>,
    name: Option<String>,
    role: Option<RoleIs>,
    has_any_role: Option<bool>,
    is_subscribed: Option<bool>,
}

/// The `parentId` a condition wants.
enum Parent {
    /// `null`: a top-level mailbox.
    Top,
    /// This one.
    Row(MailboxId),
    /// An id that can't exist: matches nothing.
    Unknown,
}

/// The `role` a condition wants.
enum RoleIs {
    /// `null`: no role.
    None,
    Role(MailboxRole),
}

/// A filter compiled once per query ([`compile`]).
enum Compiled {
    Condition(Condition),
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Vec<Self>),
    /// An operator added to the protocol later: matches nothing.
    Never,
}

fn compile(filter: &Filter<MailboxFilterCondition>) -> Compiled {
    match filter {
        Filter::Condition(condition) => Compiled::Condition(Condition {
            parent: condition.parent_id.as_ref().map(|parent| match parent {
                None => Parent::Top,
                Some(id) => ids::store_mailbox(id).map_or(Parent::Unknown, Parent::Row),
            }),
            name: condition.name.as_ref().map(|name| name.to_lowercase()),
            role: condition
                .role
                .map(|role| role.map_or(RoleIs::None, RoleIs::Role)),
            has_any_role: condition.has_any_role,
            is_subscribed: condition.is_subscribed,
        }),
        Filter::Operator(operator) => {
            let conditions = operator.conditions.iter().map(compile).collect();
            match operator.operator {
                Operator::And => Compiled::And(conditions),
                Operator::Or => Compiled::Or(conditions),
                Operator::Not => Compiled::Not(conditions),
                _ => Compiled::Never,
            }
        }
        _ => Compiled::Never,
    }
}

fn condition_matches(mailbox: &View, condition: &Condition) -> bool {
    if let Some(wanted) = &condition.parent {
        let matches = match wanted {
            Parent::Top => mailbox.parent.is_none(),
            Parent::Row(row) => mailbox.parent == Some(*row),
            Parent::Unknown => false,
        };
        if !matches {
            return false;
        }
    }
    if let Some(name) = &condition.name
        && !mailbox.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(wanted) = &condition.role {
        let role = mailbox.role.and_then(jmap_role);
        let matches = match wanted {
            RoleIs::None => role.is_none(),
            RoleIs::Role(wanted) => role == Some(*wanted),
        };
        if !matches {
            return false;
        }
    }
    if let Some(any) = condition.has_any_role
        && mailbox.role.is_some() != any
    {
        return false;
    }
    if let Some(subscribed) = condition.is_subscribed
        && mailbox.subscribed != subscribed
    {
        return false;
    }
    true
}

fn matches(mailbox: &View, filter: &Compiled) -> bool {
    match filter {
        Compiled::Condition(condition) => condition_matches(mailbox, condition),
        Compiled::And(all) => all.iter().all(|f| matches(mailbox, f)),
        Compiled::Or(any) => any.iter().any(|f| matches(mailbox, f)),
        Compiled::Not(none) => !none.iter().any(|f| matches(mailbox, f)),
        Compiled::Never => false,
    }
}

/// The order without a `sort`: `sortOrder`, then `name`.
static DEFAULT_SORT: [Comparator<MailboxSort>; 2] = [
    Comparator {
        property: MailboxSort::SortOrder,
        is_ascending: true,
        collation: None,
    },
    Comparator {
        property: MailboxSort::Name,
        is_ascending: true,
        collation: None,
    },
];

/// The checked `sort` of a query (RFC 8620 §5.5: an unknown collation is
/// `unsupportedSort` before any work); [`DEFAULT_SORT`] if empty.
fn checked_sort(
    sort: &[Comparator<MailboxSort>],
) -> Result<CheckedSort<'_, MailboxSort>, MethodError> {
    CheckedSort::new(if sort.is_empty() {
        &DEFAULT_SORT[..]
    } else {
        sort
    })
}

/// `name` with the comparator's collation (case-insensitive by default),
/// `sortOrder` as a number; then by id.
fn compare(a: &Mailbox, b: &Mailbox, sort: &CheckedSort<'_, MailboxSort>) -> Ordering {
    sort.compare(a, b, |property, collate, a, b| match property {
        MailboxSort::SortOrder => a.sort_order.cmp(&b.sort_order),
        MailboxSort::Name => collate.compare(&a.name, &b.name),
        _ => Ordering::Equal,
    })
    .then_with(|| a.id.cmp(&b.id))
}

/// Parents before their children, siblings in sort order (`sortAsTree`).
fn tree_order<'a>(list: &[&'a Mailbox], sort: &CheckedSort<'_, MailboxSort>) -> Vec<&'a Mailbox> {
    let present: BTreeSet<MailboxId> = list.iter().map(|m| m.id).collect();
    let mut children: BTreeMap<Option<MailboxId>, Vec<&'a Mailbox>> = BTreeMap::new();
    for mailbox in list {
        // A mailbox whose parent was filtered out goes to the top.
        let parent = mailbox.parent.filter(|p| present.contains(p));
        children.entry(parent).or_default().push(mailbox);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| compare(a, b, sort));
    }
    let mut out = Vec::with_capacity(list.len());
    let mut stack: Vec<&'a Mailbox> = children
        .get(&None)
        .map(|top| top.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(mailbox) = stack.pop() {
        out.push(mailbox);
        if let Some(kids) = children.get(&Some(mailbox.id)) {
            stack.extend(kids.iter().rev().copied());
        }
    }
    out
}

/// Whether every ancestor matches too (`filterAsTree`).
fn ancestors_match(mailbox: &View, all: &BTreeMap<MailboxId, View>, filter: &Compiled) -> bool {
    let mut parent = mailbox.parent;
    let mut steps = 0_usize;
    while let Some(id) = parent {
        let Some(ancestor) = all.get(&id) else {
            return true;
        };
        if !matches(ancestor, filter) {
            return false;
        }
        parent = ancestor.parent;
        steps = steps.saturating_add(1);
        if steps > all.len() {
            // A loop in the stored hierarchy: stop rather than spin.
            return false;
        }
    }
    true
}

/// The ids that match, in order: what `Mailbox/query` pages and
/// `Mailbox/queryChanges` compares against.
fn results(
    mailboxes: &[Mailbox],
    filter: Option<&Filter<MailboxFilterCondition>>,
    sort: &CheckedSort<'_, MailboxSort>,
    sort_as_tree: bool,
    filter_as_tree: bool,
) -> Vec<MailboxId> {
    let mut found: Vec<&Mailbox> = match filter {
        None => mailboxes.iter().collect(),
        Some(filter) => {
            let filter = compile(filter);
            let views: BTreeMap<MailboxId, View> =
                mailboxes.iter().map(|m| (m.id, View::of(m))).collect();
            mailboxes
                .iter()
                .filter(|m| {
                    views.get(&m.id).is_some_and(|view| {
                        matches(view, &filter)
                            && (!filter_as_tree || ancestors_match(view, &views, &filter))
                    })
                })
                .collect()
        }
    };
    if sort_as_tree {
        found = tree_order(&found, sort);
    } else {
        found.sort_by(|a, b| compare(a, b, sort));
    }
    found.iter().map(|m| m.id).collect()
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    args: MailboxQueryArguments,
) -> Result<MethodResponse, CallError> {
    let query = args.query;
    ctx.check_account(&query.account_id)?;
    let sort = checked_sort(&query.sort)?;
    let state = ctx.state(DataType::Mailbox).await?;
    let mailboxes = ctx.store().mailboxes(ctx.id()).await?;
    let all = results(
        &mailboxes,
        query.filter.as_ref(),
        &sort,
        args.sort_as_tree,
        args.filter_as_tree,
    );
    let anchor = match &query.anchor {
        Some(anchor) => Some(
            ids::store_mailbox(anchor).ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
        ),
        None => None,
    };
    let page = crate::paging::page(
        &all,
        query.position.get(),
        anchor.map(|a| (a, query.anchor_offset.get())),
        query.limit.map(UnsignedInt::get),
    )?;
    Ok(MethodResponse::MailboxQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: state,
        // RFC 8621 §2.4: `Mailbox/queryChanges` is implemented.
        can_calculate_changes: true,
        position: UnsignedInt::saturating(page.position),
        ids: page
            .ids
            .into_iter()
            .map(ids::mailbox_id)
            .collect::<Result<_, _>>()?,
        total: query
            .calculate_total
            .then(|| UnsignedInt::try_from(all.len()).unwrap_or(UnsignedInt::MAX)),
        limit: None,
    }))
}

/// Every mailbox below one of `rows` (in the current tree).
fn descendants(mailboxes: &[Mailbox], rows: &BTreeSet<i64>) -> BTreeSet<i64> {
    let mut found: BTreeSet<i64> = BTreeSet::new();
    let mut frontier: BTreeSet<i64> = rows.clone();
    // Each round goes one level down; a loop in the stored tree stops
    // when nothing new is found.
    while !frontier.is_empty() {
        let next: BTreeSet<i64> = mailboxes
            .iter()
            .filter(|m| m.parent.is_some_and(|p| frontier.contains(&p.get())))
            .map(|m| m.id.get())
            .filter(|id| !found.contains(id) && !rows.contains(id))
            .collect();
        found.extend(next.iter().copied());
        frontier = next;
    }
    found
}

/// `Mailbox/queryChanges` (RFC 8621 §2.4, RFC 8620 §5.6) from the
/// `Mailbox` change log ([`crate::changes::QueryLog`]): every mailbox
/// created, updated or destroyed since goes to `removed` (except new ones)
/// and, if it is in the results now, to `added`. With `sortAsTree` or
/// `filterAsTree` a mailbox's place or match also depends on its
/// ancestors, so the mailboxes below a changed one count as changed too
/// ("the server MUST include all Foos in the current results for which
/// this property may have changed", RFC 8620 §5.6).
pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: MailboxQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    let tree = args.sort_as_tree || args.filter_as_tree;
    let args_tree = (args.sort_as_tree, args.filter_as_tree);
    let args = args.query_changes;
    ctx.check_account(&args.account_id)?;
    let sort = checked_sort(&args.sort)?;
    let mut log = QueryLog::since(
        ctx,
        DataType::Mailbox,
        &args.since_query_state,
        args.max_changes,
    )
    .await?;
    let mailboxes = ctx.store().mailboxes(ctx.id()).await?;
    if tree {
        let below = descendants(&mailboxes, log.touched());
        log.touch(below);
    }
    let current = results(
        &mailboxes,
        args.filter.as_ref(),
        &sort,
        args_tree.0,
        args_tree.1,
    )
    .into_iter()
    .map(|id| Ok((id.get(), ids::mailbox_id(id)?)))
    .collect::<Result<Vec<_>, Bug>>()?;
    Ok(MethodResponse::MailboxQueryChanges(log.response(
        ctx,
        args.since_query_state,
        &current,
        args.calculate_total,
        |row| {
            MailboxId::from_raw(row)
                .ok_or(Bug("mailbox id"))
                .and_then(ids::mailbox_id)
        },
    )?))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn view(parent: Option<i64>, name: &str, role: Option<Role>, subscribed: bool) -> View {
        View {
            parent: parent.and_then(MailboxId::from_raw),
            name: name.to_lowercase(),
            role,
            subscribed,
        }
    }

    fn filter(value: serde_json::Value) -> Compiled {
        compile(&Filter::parse(value).unwrap())
    }

    #[test]
    fn compiled_filters_match_as_before() {
        let inbox = view(None, "INBOX", Some(Role::Inbox), true);
        let work = view(Some(1), "Proyectos Ä", None, false);
        let name = filter(json!({"name": "proyectos ä"}));
        assert!(matches(&work, &name));
        assert!(!matches(&inbox, &name));
        let not_role = filter(json!({"operator": "NOT", "conditions": [{"hasAnyRole": true}]}));
        assert!(matches(&work, &not_role));
        assert!(!matches(&inbox, &not_role));
        let top = filter(json!({"parentId": null}));
        assert!(matches(&inbox, &top));
        assert!(!matches(&work, &top));
        let parent = filter(json!({"parentId": "m1"}));
        assert!(matches(&work, &parent));
        let unknown_parent = filter(json!({"parentId": "zzz"}));
        assert!(!matches(&work, &unknown_parent));
        let either = filter(json!({"operator": "OR", "conditions": [
            {"role": "inbox"}, {"isSubscribed": false}
        ]}));
        assert!(matches(&inbox, &either) && matches(&work, &either));
        let both = filter(json!({"operator": "AND", "conditions": [
            {"role": "inbox"}, {"isSubscribed": false}
        ]}));
        assert!(!matches(&inbox, &both) && !matches(&work, &both));
    }

    fn stored(id: i64, parent: Option<i64>) -> Mailbox {
        Mailbox {
            id: MailboxId::from_raw(id).unwrap(),
            imap_name: nebula_imap::proto::types::MailboxName::new(&format!("m{id}")).unwrap(),
            name: format!("m{id}"),
            delimiter: Some('/'),
            parent: parent.and_then(MailboxId::from_raw),
            role: None,
            subscribed: true,
            selectable: true,
            no_inferiors: false,
            sort_order: 0,
            sync: nebula_store::MailboxSyncState::default(),
        }
    }

    /// `queryChanges` with a tree: the mailboxes below a changed one may
    /// have moved too (RFC 8620 §5.6).
    #[test]
    fn descendants_of_changed_mailboxes() {
        let list = [
            stored(1, None),
            stored(2, Some(1)),
            stored(3, Some(2)),
            stored(4, None),
            // A loop in the stored tree doesn't spin.
            stored(5, Some(6)),
            stored(6, Some(5)),
        ];
        assert_eq!(
            descendants(&list, &BTreeSet::from([1])),
            BTreeSet::from([2, 3])
        );
        assert_eq!(descendants(&list, &BTreeSet::from([4])), BTreeSet::new());
        assert_eq!(
            descendants(&list, &BTreeSet::from([5])),
            BTreeSet::from([6])
        );
    }

    #[test]
    fn results_follow_the_query() {
        let mut list = vec![stored(1, None), stored(2, Some(1)), stored(3, None)];
        list[0].name = "b".to_owned();
        list[1].name = "a".to_owned();
        list[2].name = "c".to_owned();
        let ids = |sort_as_tree| -> Vec<i64> {
            results(
                &list,
                None,
                &checked_sort(&[]).unwrap(),
                sort_as_tree,
                false,
            )
            .into_iter()
            .map(MailboxId::get)
            .collect()
        };
        assert_eq!(ids(false), [2, 1, 3]);
        assert_eq!(ids(true), [1, 2, 3]);
    }

    /// RFC 8620 §5.5: `name` sorts with the comparator's collation
    /// (case-insensitive by default), also as a tree; one the session
    /// doesn't offer is `unsupportedSort`.
    #[test]
    fn sorts_honour_collations() {
        let mut list = vec![
            stored(1, None),
            stored(2, None),
            stored(3, Some(1)),
            stored(4, Some(1)),
        ];
        list[0].name = "beta".to_owned();
        list[1].name = "Alpha".to_owned();
        list[2].name = "10".to_owned();
        list[3].name = "9".to_owned();
        let ids = |value, sort_as_tree| -> Vec<i64> {
            let comparators: Vec<Comparator<MailboxSort>> = Comparator::parse_list(value).unwrap();
            results(
                &list,
                None,
                &checked_sort(&comparators).unwrap(),
                sort_as_tree,
                false,
            )
            .into_iter()
            .map(MailboxId::get)
            .collect()
        };
        assert_eq!(ids(json!([{"property": "name"}]), false), [3, 4, 2, 1]);
        assert_eq!(ids(json!([{"property": "name"}]), true), [2, 1, 3, 4]);
        let numeric = json!([{"property": "name", "collation": "i;ascii-numeric"}]);
        assert_eq!(ids(numeric.clone(), false), [4, 3, 1, 2]);
        assert_eq!(ids(numeric, true), [1, 4, 3, 2]);
        let octet: Vec<Comparator<MailboxSort>> =
            Comparator::parse_list(json!([{"property": "name", "collation": "i;octet"}])).unwrap();
        assert_eq!(
            checked_sort(&octet).err().map(|e| e.kind()),
            Some(MethodErrorType::UnsupportedSort)
        );
    }

    #[test]
    fn filter_as_tree_checks_ancestors() {
        let views: BTreeMap<MailboxId, View> = [
            (1, view(None, "Archivo", None, false)),
            (2, view(Some(1), "2025", None, true)),
        ]
        .into_iter()
        .filter_map(|(id, v)| Some((MailboxId::from_raw(id)?, v)))
        .collect();
        let subscribed = filter(json!({"isSubscribed": true}));
        let child = views.get(&MailboxId::from_raw(2).unwrap()).unwrap();
        assert!(matches(child, &subscribed));
        assert!(!ancestors_match(child, &views, &subscribed));
    }
}
