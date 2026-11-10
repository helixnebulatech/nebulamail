// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The accounts of the people who share calendars or address books with
//! the user, as Stalwart shows them (RFC 9670 sharing)
//! one per owner, `a{n}-r{m}` where `r{m}` is the owner's
//! principal id, named with the owner's address, with `calendars` and/or
//! `contacts` for what they share (never creating collections). They are
//! in the session while something of theirs is (the session state
//! changes with them), and push names them too.

use std::collections::BTreeMap;

use nebula_groupware::GroupwareAccount;
use nebula_groupware::sharing::{location_label, location_number};
use nebula_http::{AccountContext, SharedAccount, shared_account_id, shared_account_key};
use nebula_jmap_proto::AccountId as JmapAccountId;
use nebula_jmap_proto::session::AccountCapabilities;
use nebula_store::{AccountId, CollectionKind, DavHref, ResourceId, Store, StoreError};

use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::error::CallError;
use crate::principals::ids::principal_id;

/// Someone who shares something with the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owner {
    /// Their principal.
    pub(crate) href: DavHref,
    /// Their principal id (`r{m}`), the key of their account.
    pub(crate) key: String,
    /// They share calendars, address books.
    pub(crate) calendars: bool,
    pub(crate) contacts: bool,
}

/// The principal id of an owner's location.
fn key_of(owner: &DavHref) -> Option<String> {
    principal_id(location_number(owner)?)
        .ok()
        .map(|id| id.as_str().to_owned())
}

/// Everyone who shares a live collection with the user.
pub(crate) async fn owners(store: &Store, account: AccountId) -> Result<Vec<Owner>, StoreError> {
    let mut by_owner: BTreeMap<DavHref, Owner> = BTreeMap::new();
    for collection in store.dav_collections(account).await? {
        let Some(href) = collection.props.sharing.owner.clone() else {
            continue;
        };
        if collection.orphaned_at.is_some() {
            continue;
        }
        let Some(key) = key_of(&href) else {
            continue;
        };
        let owner = by_owner.entry(href.clone()).or_insert(Owner {
            href,
            key,
            calendars: false,
            contacts: false,
        });
        match collection.props.kind {
            CollectionKind::Calendar => owner.calendars = true,
            CollectionKind::AddressBook => owner.contacts = true,
        }
    }
    Ok(by_owner.into_values().collect())
}

/// The name of an owner's account: their first address, else their
/// display name, else the last part of their location.
fn name(running: Option<&GroupwareAccount>, owner: &Owner) -> String {
    let principal = running.and_then(|running| running.principal_at(&owner.href));
    if let Some(principal) = principal {
        if let Some(address) = principal.addresses.first() {
            return address.address().to_owned();
        }
        if let Some(name) = principal.name.filter(|n| !n.trim().is_empty()) {
            return name;
        }
    }
    location_label(&owner.href).unwrap_or_else(|| owner.key.clone())
}

/// The shared accounts of the session.
pub(crate) async fn shared_accounts(inner: &Inner, account: &AccountContext) -> Vec<SharedAccount> {
    let Some(running) = inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(account.id))
    else {
        return Vec::new();
    };
    let available = running.availability();
    let owners = match owners(&inner.store, account.id).await {
        Ok(owners) => owners,
        Err(error) => {
            tracing::warn!(%error, "sharing: the owners could not be read");
            return Vec::new();
        }
    };
    let calendars = crate::calendars::capability(inner, account.id).map(|mut capability| {
        capability.may_create_calendar = false;
        capability
    });
    let contacts = crate::contacts::capability(inner, account.id).map(|mut capability| {
        capability.may_create_address_book = false;
        capability
    });
    owners
        .iter()
        .filter_map(|owner| {
            let id = shared_account_id(&account.jmap_id, &owner.key)?;
            let calendars = calendars
                .clone()
                .filter(|_| owner.calendars && available.calendars);
            let contacts = contacts
                .clone()
                .filter(|_| owner.contacts && available.contacts);
            if calendars.is_none() && contacts.is_none() {
                return None;
            }
            Some(SharedAccount {
                id,
                name: name(Some(&running), owner),
                capabilities: AccountCapabilities {
                    calendars_parse: calendars.is_some(),
                    calendars,
                    contacts_parse: contacts.is_some(),
                    contacts,
                    ..AccountCapabilities::default()
                },
            })
        })
        .collect()
}

/// The owner of a shared account of the call's user, if `requested` is
/// one that exists now.
pub(crate) async fn owner_of(
    ctx: &Ctx<'_>,
    requested: &JmapAccountId,
) -> Result<Option<DavHref>, CallError> {
    let Some(key) = shared_account_key(&ctx.account.jmap_id, requested) else {
        return Ok(None);
    };
    Ok(owners(ctx.store(), ctx.id())
        .await?
        .into_iter()
        .find(|owner| owner.key == key)
        .map(|owner| owner.href))
}

/// The shared account ids push should name with the groupware states.
pub(crate) async fn push_ids(
    store: &Store,
    account: AccountId,
    own: &JmapAccountId,
) -> Vec<JmapAccountId> {
    owners(store, account)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|owner| shared_account_id(own, &owner.key))
        .collect()
}

/// The account a resource is in: a shared one if its collection is
/// someone else's, else the user's.
pub(crate) async fn account_of_resource(
    store: &Store,
    account: AccountId,
    own: &JmapAccountId,
    resource: ResourceId,
) -> JmapAccountId {
    let collection = match store.dav_resource(account, resource).await {
        Ok(Some(stored)) => stored.collection,
        _ => return own.clone(),
    };
    let owner = store
        .dav_collections(account)
        .await
        .ok()
        .and_then(|all| all.into_iter().find(|c| c.id == collection))
        .and_then(|c| c.props.sharing.owner);
    owner
        .as_ref()
        .and_then(key_of)
        .and_then(|key| shared_account_id(own, &key))
        .unwrap_or_else(|| own.clone())
}
