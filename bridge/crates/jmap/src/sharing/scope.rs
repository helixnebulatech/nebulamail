// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Which collections a call sees: the user's own, or those one other
//! person shares with the user, in that person's account (`a{n}-r{m}`,
//! [`super::accounts`]).
//!
//! The groupware methods (calendars, events, address books, cards,
//! participant identities) that name a shared account run with a [`Ctx`]
//! whose `accountId` is that account and whose [`Scope`] is its owner.
//! Every list of collections is filtered by the scope, and so is every
//! resource reached through them; a resource of another scope is "not
//! found". Every other method keeps the user's own context, and a shared
//! account id there is `accountNotSupportedByMethod` (RFC 8620 §3.6.2: a
//! valid account without that data type; mail is never shared here).

use std::collections::HashSet;

use nebula_http::AccountContext;
use nebula_jmap_proto::method::ChangesResponse;
use nebula_jmap_proto::{
    AccountId as JmapAccountId, MethodCall, MethodError, MethodErrorType, MethodResponse,
};
use nebula_store::{Collection, CollectionId, CollectionKind, DavHref, StoredResource};

use crate::core::context::Ctx;
use crate::core::dispatch;
use crate::error::CallError;

/// Whose collections a call sees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The user's own.
    #[default]
    Own,
    /// Those of the owner at this principal location, shared with the
    /// user.
    Shared(DavHref),
}

impl Scope {
    /// The collection is in this scope.
    pub(crate) fn contains(&self, collection: &Collection) -> bool {
        match self {
            Self::Own => collection.props.sharing.owner.is_none(),
            Self::Shared(owner) => collection.props.sharing.owner.as_ref() == Some(owner),
        }
    }

    /// Someone else's account.
    pub(crate) const fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }
}

/// The account a groupware method names.
fn groupware_account(call: &MethodCall) -> Option<&JmapAccountId> {
    Some(match call {
        MethodCall::CalendarGet(args) => &args.account_id,
        MethodCall::CalendarChanges(args)
        | MethodCall::CalendarEventChanges(args)
        | MethodCall::AddressBookChanges(args)
        | MethodCall::ContactCardChanges(args) => &args.account_id,
        MethodCall::CalendarSet(args) => &args.set.account_id,
        MethodCall::CalendarEventGet(args) => &args.get.account_id,
        MethodCall::CalendarEventQuery(args) => &args.query.account_id,
        MethodCall::CalendarEventQueryChanges(args) => &args.query_changes.account_id,
        MethodCall::CalendarEventSet(args) => &args.set.account_id,
        MethodCall::CalendarEventParse(args) => &args.account_id,
        // A COUNTER of an event in someone else's calendar is answered in
        // their shared account (what the web sends), and only there.
        MethodCall::CalendarEventCounter(args) => &args.account_id,
        MethodCall::CalendarEventDeclineCounter(args) => &args.account_id,
        MethodCall::ParticipantIdentityGet(args) => &args.account_id,
        MethodCall::AddressBookGet(args) => &args.account_id,
        MethodCall::AddressBookSet(args) => &args.set.account_id,
        MethodCall::ContactCardGet(args) => &args.account_id,
        MethodCall::ContactCardQuery(args) => &args.account_id,
        MethodCall::ContactCardQueryChanges(args) => &args.account_id,
        MethodCall::ContactCardSet(args) => &args.account_id,
        MethodCall::ContactCardParse(args) => &args.account_id,
        _ => return None,
    })
}

/// Runs a call in the scope its `accountId` names: a groupware method on
/// a shared account in its owner's scope, everything else as it is. (The
/// method's future is boxed: it is big, and this one holds it.)
pub(crate) async fn handle(
    ctx: &mut Ctx<'_>,
    call: MethodCall,
) -> Result<Vec<MethodResponse>, CallError> {
    let owner = match groupware_account(&call) {
        Some(requested) if *requested != ctx.account.jmap_id => {
            super::accounts::owner_of(ctx, requested).await?
        }
        _ => None,
    };
    let Some(owner) = owner else {
        return Box::pin(dispatch::handle(ctx, call)).await;
    };
    let Some(requested) = groupware_account(&call).cloned() else {
        return Box::pin(dispatch::handle(ctx, call)).await;
    };
    let account = AccountContext {
        jmap_id: requested,
        ..ctx.account.clone()
    };
    let mut scoped = Ctx {
        inner: ctx.inner,
        account: &account,
        created: std::mem::take(&mut ctx.created),
        limits: ctx.limits,
        uses_bridge: ctx.uses_bridge,
        using: ctx.using.clone(),
        languages: ctx.languages.clone(),
        scope: Scope::Shared(owner),
        session: ctx.session,
    };
    let result = Box::pin(dispatch::handle(&mut scoped, call)).await;
    ctx.created = scoped.created;
    result
}

/// A call that failed with `accountNotFound` for `requested`, a shared
/// account of the user that exists: `accountNotSupportedByMethod` instead
/// ("The accountId given corresponds to a valid account, but the account
/// does not support this method or data type", RFC 8620 §3.6.2). Any
/// other error stays.
pub(crate) async fn unsupported_account(
    ctx: &Ctx<'_>,
    requested: Option<&JmapAccountId>,
    error: CallError,
) -> CallError {
    let not_found = matches!(
        &error,
        CallError::Method(e) if e.kind() == MethodErrorType::AccountNotFound
    );
    let Some(requested) = requested.filter(|_| not_found) else {
        return error;
    };
    match super::accounts::owner_of(ctx, requested).await {
        Ok(Some(_)) => MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
            .with_description("this shared account does not support this method")
            .into(),
        Ok(None) | Err(_) => error,
    }
}

/// The live collections of `kind` in the call's scope.
pub(crate) async fn collection_ids(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
) -> Result<HashSet<CollectionId>, CallError> {
    Ok(ctx
        .store()
        .dav_collections(ctx.id())
        .await?
        .into_iter()
        .filter(|c| c.props.kind == kind && c.orphaned_at.is_none() && ctx.scope.contains(c))
        .map(|c| c.id)
        .collect())
}

/// Keeps the resources of collections in the call's scope.
pub(crate) async fn retain(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
    resources: &mut Vec<StoredResource>,
) -> Result<(), CallError> {
    let allowed = collection_ids(ctx, kind).await?;
    resources.retain(|resource| allowed.contains(&resource.collection));
    Ok(())
}

/// The resource, if its collection is in the call's scope.
pub(crate) async fn resource(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
    found: Option<StoredResource>,
) -> Result<Option<StoredResource>, CallError> {
    let Some(found) = found else {
        return Ok(None);
    };
    let allowed = collection_ids(ctx, kind).await?;
    Ok(allowed.contains(&found.collection).then_some(found))
}

/// Whether `collection` may become the user's default of `kind` from a
/// call in `scope` (`onSuccessSetIsDefault`): a live one of that kind in
/// the user's own scope. The default is the user's, so a call in a shared
/// account never changes it.
pub(crate) fn may_be_default(scope: &Scope, kind: CollectionKind, collection: &Collection) -> bool {
    !scope.is_shared()
        && collection.props.kind == kind
        && collection.orphaned_at.is_none()
        && scope.contains(collection)
}

/// Whether the collection `row` may become the default of `kind` in this
/// call ([`may_be_default`]).
pub(crate) async fn default_target(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
    row: CollectionId,
) -> Result<bool, CallError> {
    if ctx.scope.is_shared() {
        return Ok(false);
    }
    Ok(ctx
        .store()
        .dav_collections(ctx.id())
        .await?
        .iter()
        .any(|c| c.id == row && may_be_default(&ctx.scope, kind, c)))
}

/// Whether the local copy has anything in a collection that is still on
/// the server, shown over JMAP or not (one that doesn't parse counts):
/// the quick check before deleting a collection that must be empty.
pub(crate) fn has_contents(resources: &[StoredResource]) -> bool {
    resources.iter().any(|r| r.orphaned_at.is_none())
}

/// Whether the account has anything shared by others (when not, every
/// scope filter is a no-op for the own account).
async fn has_shared(ctx: &Ctx<'_>) -> Result<bool, CallError> {
    Ok(ctx
        .store()
        .dav_collections(ctx.id())
        .await?
        .iter()
        .any(|c| c.props.sharing.owner.is_some()))
}

/// `Calendar/changes` and `AddressBook/changes` of the call's scope:
/// created and updated collections of another scope are left out
/// (destroyed ones stay: a client drops ids it does not know).
pub(crate) async fn collection_changes<I>(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
    mut response: ChangesResponse<I>,
    row: impl Fn(&I) -> Option<CollectionId>,
) -> Result<ChangesResponse<I>, CallError> {
    if !ctx.scope.is_shared() && !has_shared(ctx).await? {
        return Ok(response);
    }
    let allowed = collection_ids(ctx, kind).await?;
    let keep = |id: &I| row(id).is_some_and(|row| allowed.contains(&row));
    response.created.retain(keep);
    response.updated.retain(keep);
    Ok(response)
}

/// `CalendarEvent/changes` and `ContactCard/changes` of the call's scope:
/// created and updated resources of another scope's collections are left
/// out (destroyed ones stay).
pub(crate) async fn resource_changes<I>(
    ctx: &Ctx<'_>,
    kind: CollectionKind,
    mut response: ChangesResponse<I>,
    row: impl Fn(&I) -> Option<nebula_store::ResourceId>,
) -> Result<ChangesResponse<I>, CallError> {
    if !ctx.scope.is_shared() && !has_shared(ctx).await? {
        return Ok(response);
    }
    let allowed = collection_ids(ctx, kind).await?;
    for list in [&mut response.created, &mut response.updated] {
        let mut kept = Vec::with_capacity(list.len());
        for id in list.drain(..) {
            let Some(resource) = row(&id) else {
                continue;
            };
            if let Some(stored) = ctx.store().dav_resource(ctx.id(), resource).await?
                && allowed.contains(&stored.collection)
            {
                kept.push(id);
            }
        }
        *list = kept;
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use nebula_store::{
        CalendarExtras, CollectionProps, CollectionSharing, CollectionSync, Parsed, ResourceId,
        UnixTime,
    };

    use super::*;

    fn collection(kind: CollectionKind, owner: Option<&str>, orphaned: bool) -> Collection {
        Collection {
            id: CollectionId::from_raw(4).unwrap(),
            props: CollectionProps {
                kind,
                href: DavHref::new("https://dav.example/dav/alice/work/").unwrap(),
                name: None,
                color: None,
                description: None,
                sort_order: None,
                time_zone: None,
                default_alarms_datetime: None,
                default_alarms_date: None,
                rights: None,
                components: None,
                vcard_versions: None,
                supports_sync: true,
                sharing: CollectionSharing {
                    owner: owner.map(|o| DavHref::new(o).unwrap()),
                    ..CollectionSharing::default()
                },
                extras: CalendarExtras::default(),
            },
            sync: CollectionSync::default(),
            orphaned_at: orphaned.then(|| UnixTime::from_secs(1)),
            is_visible: true,
            is_subscribed: true,
            is_default: false,
        }
    }

    /// Only a live collection of the kind, in the own scope, may become
    /// the default.
    #[test]
    fn default_targets() {
        let book = CollectionKind::AddressBook;
        let own = Scope::Own;
        let bob = "https://dav.example/dav/principals/bob/";
        assert!(may_be_default(&own, book, &collection(book, None, false)));
        // A calendar given to AddressBook/set.
        assert!(!may_be_default(
            &own,
            book,
            &collection(CollectionKind::Calendar, None, false)
        ));
        // Someone else's book seen from the own account.
        assert!(!may_be_default(
            &own,
            book,
            &collection(book, Some(bob), false)
        ));
        // Gone from the server.
        assert!(!may_be_default(&own, book, &collection(book, None, true)));
        // From the shared account, not even the owner's own book.
        let shared = Scope::Shared(DavHref::new(bob).unwrap());
        assert!(!may_be_default(
            &shared,
            book,
            &collection(book, Some(bob), false)
        ));
    }

    fn resource(parsed: Parsed, orphaned: bool) -> StoredResource {
        StoredResource {
            id: ResourceId::from_raw(9).unwrap(),
            collection: CollectionId::from_raw(4).unwrap(),
            href: DavHref::new("https://dav.example/dav/alice/work/1.vcf").unwrap(),
            etag: None,
            size: 10,
            parsed,
            updated_at: UnixTime::from_secs(1),
            orphaned_at: orphaned.then(|| UnixTime::from_secs(2)),
        }
    }

    /// Proposals run in the shared account they name, like the other
    /// event methods.
    #[test]
    fn counter_methods_name_their_account() {
        use nebula_jmap_proto::object::{
            CalendarEventCounterArguments, CalendarEventDeclineCounterArguments,
        };
        use serde_json::json;
        let args = |value: serde_json::Value| value.as_object().unwrap().clone();
        let counter = MethodCall::CalendarEventCounter(
            CalendarEventCounterArguments::parse(args(json!({
                "accountId": "a1-r2", "eventId": "v7",
                "utcStart": "2026-11-02T10:00:00Z", "utcEnd": "2026-11-02T11:00:00Z"
            })))
            .unwrap(),
        );
        let decline = MethodCall::CalendarEventDeclineCounter(
            CalendarEventDeclineCounterArguments::parse(args(json!({
                "accountId": "a1-r2", "eventId": "v7", "blobId": "b1"
            })))
            .unwrap(),
        );
        let shared: JmapAccountId = "a1-r2".parse().unwrap();
        assert_eq!(groupware_account(&counter), Some(&shared));
        assert_eq!(groupware_account(&decline), Some(&shared));
    }

    /// A card that doesn't parse (not shown over JMAP) still counts as
    /// contents; one gone from the server doesn't.
    #[test]
    fn hidden_resources_are_contents() {
        assert!(!has_contents(&[]));
        assert!(has_contents(&[resource(Parsed::Invalid, false)]));
        assert!(!has_contents(&[resource(Parsed::Invalid, true)]));
    }
}
