// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Quota/get`, `Quota/changes`, `Quota/query` and `Quota/queryChanges`
//! (RFC 9425) from IMAP `GETQUOTAROOT INBOX` (RFC 9208), asked on every
//! call: quotas change with every message and the answer is small.
//!
//! - One JMAP Quota per quota root and resource: `STORAGE` → `octets`,
//!   `MESSAGE` → `count` of emails, `MAILBOX` → `count` of mailboxes.
//!   IMAP doesn't say whom a root covers, so the scope is `account`.
//! - `types` only names types of capabilities in the request's `using`,
//!   and a quota with none left is not there for that request (RFC 9425
//!   §4.1): every type here is `urn:ietf:params:jmap:mail`'s.
//! - Ids come from the root name and the resource (`q` + kind + the name in
//!   hex), so they are stable without storing anything; so are `name`,
//!   `types` and `resourceType`: only `used` and `hardLimit` change.
//! - State: a hash of the whole list. The last few lists per account are
//!   kept in memory, so `Quota/changes` and `Quota/queryChanges` from one
//!   of them are exact; from an older state (or after a restart) they are
//!   `cannotCalculateChanges`.
//! - Push (RFC 9425 §6): a mail change of `Email` or `Mailbox` asks the
//!   quotas again ([`push_state`]); the push has `Quota` when its state
//!   moved.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use nebula_jmap_proto::method::{
    AddedItem, ChangesArguments, ChangesResponse, Comparator, Filter, GetArguments, GetResponse,
    Operator, QueryChangesResponse, QueryResponse,
};
use nebula_jmap_proto::object::{
    Quota, QuotaChangesResponse, QuotaFilterCondition, QuotaProperty, QuotaQueryArguments,
    QuotaQueryChangesArguments, QuotaScope, QuotaSort, ResourceType,
};
use nebula_jmap_proto::types::DataType;
use nebula_jmap_proto::{
    Capability, MethodError, MethodErrorType, MethodResponse, QuotaId, State, UnsignedInt,
};
use nebula_mail::{QuotaResource, QuotaUsage};
use nebula_store::AccountId;

use crate::collation::Collate;
use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// Lists kept per account for `Quota/changes`.
const HISTORY: usize = 16;

/// Recent quota lists, by state.
#[derive(Debug, Default)]
pub(crate) struct QuotaHistory {
    accounts: Mutex<HashMap<AccountId, VecDeque<Snapshot>>>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    state: State,
    entries: Vec<Entry>,
}

impl QuotaHistory {
    fn remember(&self, account: AccountId, snapshot: Snapshot) {
        let mut accounts = self.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        let list = accounts.entry(account).or_default();
        if list.iter().any(|s| s.state == snapshot.state) {
            return;
        }
        if list.len() >= HISTORY {
            list.pop_front();
        }
        list.push_back(snapshot);
    }

    fn find(&self, account: AccountId, state: &State) -> Option<Snapshot> {
        self.accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)?
            .iter()
            .find(|s| s.state == *state)
            .cloned()
    }

    /// Forgets an account (its session ended).
    pub(crate) fn forget(&self, account: AccountId) {
        self.accounts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&account);
    }
}

/// FNV-1a: stable across runs and platforms (unlike `DefaultHasher`). Not
/// for security: ids and states only need to differ.
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.iter().chain(std::iter::once(&0xff)) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn kind_letter(resource: QuotaResource) -> char {
    match resource {
        QuotaResource::Messages => 'm',
        QuotaResource::Mailboxes => 'b',
        _ => 's',
    }
}

/// `q` + kind + the root in hex (or a hash of it, for long roots: an id
/// has at most 255 characters).
fn quota_id(usage: &QuotaUsage) -> Result<QuotaId, Bug> {
    use std::fmt::Write as _;
    let mut id = format!("q{}", kind_letter(usage.resource));
    if usage.root.len() <= 100 {
        id.push('x');
        for byte in usage.root.bytes() {
            let _ = write!(id, "{byte:02x}");
        }
    } else {
        let _ = write!(id, "h{:016x}", fnv(&[usage.root.as_bytes()]));
    }
    id.parse().map_err(|_| Bug("quota id"))
}

/// The capability a client must use to know a data type (RFC 9425 §4.1).
fn capability_of(data_type: &DataType) -> Option<Capability> {
    match data_type {
        DataType::Mailbox | DataType::Email => Some(Capability::Mail),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Entry {
    id: QuotaId,
    usage: QuotaUsage,
}

impl Entry {
    fn name(&self, account_name: &str) -> String {
        if self.usage.root.is_empty() {
            account_name.to_owned()
        } else {
            self.usage.root.clone()
        }
    }

    fn resource_type(&self) -> ResourceType {
        match self.usage.resource {
            QuotaResource::Storage => ResourceType::Octets,
            _ => ResourceType::Count,
        }
    }

    /// The types it counts that the client knows (`uses`: whether a
    /// capability is in `using`).
    fn types(&self, uses: &impl Fn(Capability) -> bool) -> Vec<DataType> {
        let all = match self.usage.resource {
            QuotaResource::Mailboxes => vec![DataType::Mailbox],
            _ => vec![DataType::Email],
        };
        all.into_iter()
            .filter(|t| capability_of(t).is_some_and(uses))
            .collect()
    }

    fn to_jmap(
        &self,
        account_name: &str,
        types: Vec<DataType>,
        properties: Option<&[QuotaProperty]>,
    ) -> Quota {
        let wants = |p| properties.is_none_or(|list| list.contains(&p));
        let mut out = Quota {
            id: Some(self.id.clone()),
            ..Quota::default()
        };
        if wants(QuotaProperty::ResourceType) {
            out.resource_type = Some(self.resource_type());
        }
        if wants(QuotaProperty::Used) {
            out.used = Some(UnsignedInt::saturating(self.usage.used));
        }
        if wants(QuotaProperty::HardLimit) {
            out.hard_limit = Some(UnsignedInt::saturating(self.usage.limit));
        }
        if wants(QuotaProperty::Scope) {
            out.scope = Some(QuotaScope::Account);
        }
        if wants(QuotaProperty::Name) {
            out.name = Some(self.name(account_name));
        }
        if wants(QuotaProperty::Types) {
            out.types = Some(types);
        }
        // IMAP has no warn or soft limits, nor a description.
        if wants(QuotaProperty::WarnLimit) {
            out.warn_limit = Some(None);
        }
        if wants(QuotaProperty::SoftLimit) {
            out.soft_limit = Some(None);
        }
        if wants(QuotaProperty::Description) {
            out.description = Some(None);
        }
        out
    }
}

/// The quotas of `usages`, sorted by id, and their state.
fn snapshot(usages: Vec<QuotaUsage>) -> Result<Snapshot, Bug> {
    let mut entries = usages
        .into_iter()
        .map(|usage| {
            Ok(Entry {
                id: quota_id(&usage)?,
                usage,
            })
        })
        .collect::<Result<Vec<_>, Bug>>()?;
    entries.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for entry in &entries {
        parts.push(entry.id.as_str().as_bytes().to_vec());
        parts.push(entry.usage.used.to_be_bytes().to_vec());
        parts.push(entry.usage.limit.to_be_bytes().to_vec());
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    let state = State::new(format!("q{:016x}", fnv(&refs)));
    Ok(Snapshot { state, entries })
}

/// The current quotas and their state; remembered for `Quota/changes`.
async fn current(ctx: &Ctx<'_>) -> Result<Snapshot, CallError> {
    let usages = ctx.mail()?.quotas().await?.unwrap_or_default();
    let now = snapshot(usages)?;
    ctx.inner.quotas.remember(ctx.id(), now.clone());
    Ok(now)
}

/// Whether a change of these types may move the quotas: every quota
/// here counts emails (their size or number) or mailboxes.
pub(crate) fn moves_quota(types: &[nebula_store::DataType]) -> bool {
    types.iter().any(|t| {
        matches!(
            t,
            nebula_store::DataType::Email | nebula_store::DataType::Mailbox
        )
    })
}

/// The Quota state of the account now, for push (RFC 9425 §6); `None` if
/// its IMAP server has no `QUOTA` (no `urn:ietf:params:jmap:quota`) or
/// can't be asked. Remembered, so `Quota/changes` works from it.
pub(crate) async fn push_state(inner: &Inner, account: AccountId) -> Option<State> {
    let mail = inner.mail.account(account)?;
    if !mail.supports_quota() {
        return None;
    }
    let usages = match mail.quotas().await {
        Ok(usages) => usages?,
        Err(error) => {
            tracing::debug!(error = %error, "push: quotas unavailable");
            return None;
        }
    };
    let now = snapshot(usages).ok()?;
    let state = now.state.clone();
    inner.quotas.remember(account, now);
    Some(state)
}

/// The quotas the client may see: those with a type it knows.
fn visible<'a>(ctx: &Ctx<'_>, entries: &'a [Entry]) -> Vec<(&'a Entry, Vec<DataType>)> {
    let uses = |capability: Capability| ctx.uses(&capability);
    entries
        .iter()
        .filter_map(|entry| {
            let types = entry.types(&uses);
            (!types.is_empty()).then_some((entry, types))
        })
        .collect()
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<QuotaId, QuotaProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let now = current(ctx).await?;
    let shown = visible(ctx, &now.entries);
    let properties = args.properties.as_deref();
    let name = &ctx.account.email;
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => list.extend(
            shown
                .iter()
                .map(|(e, types)| e.to_jmap(name, types.clone(), properties)),
        ),
        Some(wanted) => {
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                match shown.iter().find(|(e, _)| e.id == id) {
                    Some((entry, types)) => {
                        list.push(entry.to_jmap(name, types.clone(), properties));
                    }
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::QuotaGet(GetResponse {
        account_id: ctx.jmap_id(),
        state: now.state,
        list,
        not_found,
    }))
}

/// What changed between two lists of visible quotas, and whether only
/// `used` changed in the updated ones.
struct Diff {
    created: Vec<QuotaId>,
    updated: Vec<QuotaId>,
    destroyed: Vec<QuotaId>,
    only_used: bool,
}

fn diff(old: &[(&Entry, Vec<DataType>)], new: &[(&Entry, Vec<DataType>)]) -> Diff {
    let mut out = Diff {
        created: Vec::new(),
        updated: Vec::new(),
        destroyed: Vec::new(),
        only_used: true,
    };
    for (entry, _) in new {
        match old.iter().find(|(o, _)| o.id == entry.id) {
            None => out.created.push(entry.id.clone()),
            Some((before, _)) if before.usage != entry.usage => {
                // The id fixes the root and the resource (so `name`,
                // `types`, `resourceType`): only `used` and `hardLimit`
                // can differ.
                if before.usage.limit != entry.usage.limit {
                    out.only_used = false;
                }
                out.updated.push(entry.id.clone());
            }
            Some(_) => {}
        }
    }
    out.destroyed = old
        .iter()
        .filter(|(o, _)| !new.iter().any(|(e, _)| e.id == o.id))
        .map(|(o, _)| o.id.clone())
        .collect();
    out
}

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let now = current(ctx).await?;
    let old = ctx
        .inner
        .quotas
        .find(ctx.id(), &args.since_state)
        .ok_or(CallError::from(MethodErrorType::CannotCalculateChanges))?;
    let found = diff(&visible(ctx, &old.entries), &visible(ctx, &now.entries));
    let count = found
        .created
        .len()
        .saturating_add(found.updated.len())
        .saturating_add(found.destroyed.len());
    // RFC 8620 §5.2: never more ids than `maxChanges`; there is no
    // intermediate state to stop at.
    if args
        .max_changes
        .is_some_and(|max| count > max.get().to_usize_saturating())
    {
        return Err(MethodErrorType::CannotCalculateChanges.into());
    }
    // RFC 9425 §4.3: `["used"]` if only that changed, else `null`.
    let updated_properties =
        (found.only_used && !found.updated.is_empty()).then(|| vec![QuotaProperty::Used]);
    Ok(MethodResponse::QuotaChanges(QuotaChangesResponse {
        changes: ChangesResponse {
            account_id: ctx.jmap_id(),
            old_state: args.since_state,
            new_state: now.state,
            has_more_changes: false,
            created: found.created,
            updated: found.updated,
            destroyed: found.destroyed,
        },
        updated_properties,
    }))
}

fn condition_matches(
    entry: &Entry,
    types: &[DataType],
    account_name: &str,
    condition: &QuotaFilterCondition,
) -> bool {
    if let Some(name) = &condition.name
        && !entry
            .name(account_name)
            .to_lowercase()
            .contains(&name.to_lowercase())
    {
        return false;
    }
    if condition
        .scope
        .is_some_and(|scope| scope != QuotaScope::Account)
    {
        return false;
    }
    if condition
        .resource_type
        .is_some_and(|kind| kind != entry.resource_type())
    {
        return false;
    }
    if let Some(data_type) = &condition.data_type
        && !types.contains(data_type)
    {
        return false;
    }
    true
}

fn matches(
    entry: &Entry,
    types: &[DataType],
    account_name: &str,
    filter: &Filter<QuotaFilterCondition>,
) -> bool {
    match filter {
        Filter::Condition(condition) => condition_matches(entry, types, account_name, condition),
        Filter::Operator(operator) => {
            let mut results = operator
                .conditions
                .iter()
                .map(|f| matches(entry, types, account_name, f));
            match operator.operator {
                Operator::And => results.all(|r| r),
                Operator::Or => results.any(|r| r),
                Operator::Not => !results.any(|r| r),
                _ => false,
            }
        }
        _ => false,
    }
}

/// A sort with its collations checked (RFC 8620 §5.5).
struct Sort<'a> {
    comparators: Vec<(&'a Comparator<QuotaSort>, Collate)>,
}

impl<'a> Sort<'a> {
    /// # Errors
    /// `unsupportedSort` for a collation the session doesn't offer.
    fn new(sort: &'a [Comparator<QuotaSort>]) -> Result<Self, MethodError> {
        Ok(Self {
            comparators: sort
                .iter()
                .map(|c| Ok((c, Collate::of(c.collation)?)))
                .collect::<Result<_, MethodError>>()?,
        })
    }

    fn compare(&self, a: &Entry, b: &Entry, account_name: &str) -> Ordering {
        for (comparator, collate) in &self.comparators {
            let order = match comparator.property {
                QuotaSort::Name => collate.compare(&a.name(account_name), &b.name(account_name)),
                QuotaSort::Used => a.usage.used.cmp(&b.usage.used),
                _ => Ordering::Equal,
            };
            let order = if comparator.is_ascending {
                order
            } else {
                order.reverse()
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        a.id.as_str().cmp(b.id.as_str())
    }
}

/// The ids a query gives over `entries`, in order.
fn results(
    ctx: &Ctx<'_>,
    entries: &[Entry],
    filter: Option<&Filter<QuotaFilterCondition>>,
    sort: &Sort<'_>,
) -> Vec<QuotaId> {
    let name = ctx.account.email.as_str();
    let mut found: Vec<&Entry> = visible(ctx, entries)
        .into_iter()
        .filter(|(e, types)| filter.is_none_or(|f| matches(e, types, name, f)))
        .map(|(e, _)| e)
        .collect();
    found.sort_by(|a, b| sort.compare(a, b, name));
    found.into_iter().map(|e| e.id.clone()).collect()
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    query: QuotaQueryArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&query.account_id)?;
    let sort = Sort::new(&query.sort)?;
    let now = current(ctx).await?;
    let found = results(ctx, &now.entries, query.filter.as_ref(), &sort);
    let indices: Vec<usize> = (0..found.len()).collect();
    let anchor = match &query.anchor {
        Some(anchor) => Some(
            found
                .iter()
                .position(|id| id == anchor)
                .ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
        ),
        None => None,
    };
    let page = crate::paging::page(
        &indices,
        query.position.get(),
        anchor.map(|a| (a, query.anchor_offset.get())),
        query.limit.map(UnsignedInt::get),
    )?;
    let ids = page
        .ids
        .into_iter()
        .filter_map(|index| found.get(index).cloned())
        .collect();
    Ok(MethodResponse::QuotaQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: now.state,
        // `Quota/queryChanges` from a remembered state (RFC 9425 §4.5).
        can_calculate_changes: true,
        position: UnsignedInt::saturating(page.position),
        ids,
        total: query
            .calculate_total
            .then(|| UnsignedInt::try_from(found.len()).unwrap_or(UnsignedInt::MAX)),
        limit: None,
    }))
}

/// `removed` and `added` (RFC 8620 §5.6) from the old and new results:
/// every id no longer there, or whose quota changed (it may have moved),
/// is removed; every new one, or changed, is added with its index.
fn query_diff(
    old: &[QuotaId],
    new: &[QuotaId],
    changed: &[QuotaId],
) -> (Vec<QuotaId>, Vec<AddedItem<QuotaId>>) {
    let removed = old
        .iter()
        .filter(|id| !new.contains(id) || changed.contains(id))
        .cloned()
        .collect();
    let added = new
        .iter()
        .enumerate()
        .filter(|(_, id)| !old.contains(id) || changed.contains(id))
        .map(|(index, id)| AddedItem {
            id: id.clone(),
            index: UnsignedInt::saturating(u64::try_from(index).unwrap_or(u64::MAX)),
        })
        .collect();
    (removed, added)
}

/// `Quota/queryChanges` (RFC 9425 §4.5, RFC 8620 §5.6): the old list is
/// one remembered with its state, so the old results are computed again
/// and compared. `upToId` is ignored (the RFC allows it).
pub(crate) async fn query_changes(
    ctx: &Ctx<'_>,
    args: QuotaQueryChangesArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let sort = Sort::new(&args.sort)?;
    let now = current(ctx).await?;
    let old = ctx
        .inner
        .quotas
        .find(ctx.id(), &args.since_query_state)
        .ok_or(CallError::from(MethodErrorType::CannotCalculateChanges))?;
    let changed = diff(&visible(ctx, &old.entries), &visible(ctx, &now.entries)).updated;
    let before = results(ctx, &old.entries, args.filter.as_ref(), &sort);
    let after = results(ctx, &now.entries, args.filter.as_ref(), &sort);
    let (removed, added) = query_diff(&before, &after, &changed);
    if args.max_changes.is_some_and(|max| {
        removed.len().saturating_add(added.len()) > max.get().to_usize_saturating()
    }) {
        return Err(MethodErrorType::TooManyChanges.into());
    }
    Ok(MethodResponse::QuotaQueryChanges(QueryChangesResponse {
        account_id: ctx.jmap_id(),
        old_query_state: args.since_query_state,
        new_query_state: now.state,
        total: args
            .calculate_total
            .then(|| UnsignedInt::try_from(after.len()).unwrap_or(UnsignedInt::MAX)),
        removed,
        added,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(root: &str, resource: QuotaResource) -> QuotaUsage {
        QuotaUsage {
            root: root.to_owned(),
            resource,
            used: 1,
            limit: 2,
        }
    }

    #[test]
    fn ids_are_stable_and_valid() {
        let a = quota_id(&usage("User quota", QuotaResource::Storage)).unwrap();
        assert_eq!(a.as_str(), "qsx557365722071756f7461");
        let b = quota_id(&usage("User quota", QuotaResource::Messages)).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            quota_id(&usage("", QuotaResource::Storage))
                .unwrap()
                .as_str(),
            "qsx"
        );
        let long = "r".repeat(300);
        let id = quota_id(&usage(&long, QuotaResource::Storage)).unwrap();
        assert!(id.as_str().len() < 30);
        // Odd bytes (spaces, quotes, UTF-8) never reach the id.
        assert!(quota_id(&usage("a \"b\" ü", QuotaResource::Storage)).is_ok());
    }

    #[test]
    fn names_and_filters() {
        let entry = Entry {
            id: quota_id(&usage("", QuotaResource::Mailboxes)).unwrap(),
            usage: usage("", QuotaResource::Mailboxes),
        };
        assert_eq!(entry.name("alice@example.com"), "alice@example.com");
        assert_eq!(entry.resource_type(), ResourceType::Count);
        let condition = QuotaFilterCondition {
            data_type: Some(DataType::Mailbox),
            name: Some("ALICE".to_owned()),
            ..QuotaFilterCondition::default()
        };
        let types = entry.types(&|_| true);
        assert!(condition_matches(
            &entry,
            &types,
            "alice@example.com",
            &condition
        ));
        let condition = QuotaFilterCondition {
            resource_type: Some(ResourceType::Octets),
            ..QuotaFilterCondition::default()
        };
        assert!(!condition_matches(
            &entry,
            &types,
            "alice@example.com",
            &condition
        ));
    }

    /// RFC 9425 §4.1: only types of capabilities in `using`.
    #[test]
    fn types_follow_using() {
        let entry = entry("", 1, 2);
        assert_eq!(
            entry.types(&|c| c == Capability::Mail),
            vec![DataType::Email]
        );
        assert!(entry.types(&|c| c == Capability::Quota).is_empty());
    }

    fn entry(root: &str, used: u64, limit: u64) -> Entry {
        let usage = QuotaUsage {
            root: root.to_owned(),
            resource: QuotaResource::Storage,
            used,
            limit,
        };
        Entry {
            id: quota_id(&usage).unwrap(),
            usage,
        }
    }

    fn shown(list: &[Entry]) -> Vec<(&Entry, Vec<DataType>)> {
        list.iter().map(|e| (e, vec![DataType::Email])).collect()
    }

    /// RFC 9425 §4.3: `updatedProperties` is `["used"]` only when nothing
    /// else changed.
    #[test]
    fn only_used_is_told_apart() {
        let old = [entry("a", 1, 10), entry("b", 1, 10)];
        let used = [entry("a", 5, 10), entry("b", 1, 10), entry("c", 0, 1)];
        let found = diff(&shown(&old), &shown(&used));
        assert_eq!(found.updated, vec![entry("a", 0, 0).id]);
        assert_eq!(found.created, vec![entry("c", 0, 0).id]);
        assert!(found.only_used && found.destroyed.is_empty());
        let limit = [entry("a", 1, 20)];
        let found = diff(&shown(&old), &shown(&limit));
        assert!(!found.only_used);
        assert_eq!(found.destroyed, vec![entry("b", 0, 0).id]);
    }

    #[test]
    fn query_changes_diff() {
        let [a, b, c] = ["a", "b", "c"].map(|root| entry(root, 0, 0).id);
        // Old results a, b; now c, a (b gone, c new, a changed).
        let (removed, added) = query_diff(
            &[a.clone(), b.clone()],
            &[c.clone(), a.clone()],
            std::slice::from_ref(&a),
        );
        assert_eq!(removed, vec![a.clone(), b]);
        let added: Vec<(QuotaId, u64)> = added.into_iter().map(|i| (i.id, i.index.get())).collect();
        assert_eq!(added, vec![(c, 0), (a, 1)]);
    }

    #[test]
    fn sorts_honour_collations() {
        use nebula_jmap_proto::method::Collation;
        let by_name = |collation| {
            vec![Comparator {
                property: QuotaSort::Name,
                is_ascending: true,
                collation,
            }]
        };
        let numeric = by_name(Some(Collation::AsciiNumeric));
        let sort = Sort::new(&numeric).unwrap();
        // "10" > "9" as numbers, not as text.
        assert_eq!(
            sort.compare(&entry("10", 0, 0), &entry("9", 0, 0), "x"),
            Ordering::Greater
        );
        let default = by_name(None);
        let sort = Sort::new(&default).unwrap();
        assert_eq!(
            sort.compare(&entry("10", 0, 0), &entry("9", 0, 0), "x"),
            Ordering::Less
        );
        assert_eq!(
            sort.compare(&entry("alpha", 0, 0), &entry("Beta", 0, 0), "x"),
            Ordering::Less
        );
        let octet = by_name(Some(Collation::Octet));
        assert_eq!(
            Sort::new(&octet).err().map(|e| e.kind()),
            Some(MethodErrorType::UnsupportedSort)
        );
    }

    #[test]
    fn history_is_bounded() {
        let history = QuotaHistory::default();
        let account = AccountId::from_raw(1).unwrap();
        for n in 0..(HISTORY + 4) {
            history.remember(
                account,
                Snapshot {
                    state: State::new(format!("s{n}")),
                    entries: Vec::new(),
                },
            );
        }
        assert!(history.find(account, &State::new("s0")).is_none());
        assert!(
            history
                .find(account, &State::new(format!("s{}", HISTORY + 3)))
                .is_some()
        );
        history.forget(account);
        assert!(
            history
                .find(account, &State::new(format!("s{}", HISTORY + 3)))
                .is_none()
        );
    }
}
