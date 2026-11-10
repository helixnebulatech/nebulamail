// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `AddressBook/set` (RFC 9610 §2.3) over CardDAV:
//!
//! - **create**: extended MKCOL of `{home}/{uuid}/` with `displayname`,
//!   `addressbook-description` and (if not 0) `calendar-order`.
//! - **update**: `name`, `description` and `sortOrder` by PROPPATCH (all
//!   or nothing: a refused property is `forbidden`, nothing changed);
//!   `isSubscribed` in the store.
//! - **destroy**: DELETE of the collection; with cards in it only with
//!   `onDestroyRemoveContents` (else `addressBookHasContents`).
//! - `shareWith` (whole or `shareWith/<id>`) in an update: shared the way
//!   the server lets the user (`crate::sharing`). Also when creating:
//!   checked first, set right after the MKCOL; if the server refuses, the
//!   address book is deleted again (all or nothing).
//! - Nothing is created in someone else's shared account (`forbidden`).
//! - `onSuccessSetIsDefault`: in the store, once everything worked.

use std::collections::BTreeMap;

use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{Property, ResourceName};
use nebula_groupware::{Contents, GroupwareAccount, GroupwareError};
use nebula_jmap_proto::method::{PatchObject, SetResponse};
use nebula_jmap_proto::object::{
    AddressBook, AddressBookCreate, AddressBookPatch, AddressBookProperty, AddressBookSetArguments,
};
use nebula_jmap_proto::{
    AddressBookId, CreationId, MethodResponse, SetError, SetErrorType, UnsignedInt,
};
use nebula_store::{CollectionId, CollectionKind, DataType, Rights};
use serde_json::{Map, Value};

use super::book::{allows, to_jmap};
use super::card::{Books, books_by_id};
use super::convert::uuid;
use super::error::set_error;
use super::ids;
use crate::core::context::Ctx;
use crate::error::{CallError, store_set_error};
use crate::sharing::scope::{default_target, has_contents};

fn forbidden(description: &str) -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description(description)
}

fn order(sort_order: UnsignedInt) -> Result<i32, SetError> {
    i32::try_from(sort_order.get()).map_err(|_| {
        SetError::invalid_properties(["sortOrder"]).with_description("sortOrder is too large")
    })
}

/// The DAV properties of a new book.
fn create_values(create: &AddressBookCreate) -> Result<Vec<PropValue>, SetError> {
    let mut values = vec![PropValue::DisplayName(create.name.clone())];
    if let Some(description) = &create.description {
        values.push(PropValue::AddressbookDescription(description.clone()));
    }
    if create.sort_order != UnsignedInt::ZERO {
        values.push(PropValue::CalendarOrder(order(create.sort_order)?));
    }
    Ok(values)
}

async fn create_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    object: Map<String, Value>,
) -> Result<(CollectionId, AddressBook), SetError> {
    let create = AddressBookCreate::parse(object)?;
    if ctx.scope.is_shared() {
        return Err(forbidden(
            "address books can't be created in someone else's account",
        ));
    }
    let share_with = create.share_with.clone().unwrap_or_default();
    crate::sharing::share_with::check_new(ctx, &share_with).await?;
    let values = create_values(&create)?;
    let name = ResourceName::try_from(uuid()?.as_str()).map_err(|_| forbidden("name"))?;
    let book = dav
        .create_collection(CollectionKind::AddressBook, &name, &values)
        .await
        .map_err(|e| set_error(&e))?;
    let mut book = book;
    if let Err(error) = crate::sharing::share_with::share_new(
        ctx,
        book.id,
        &share_with,
        crate::sharing::share_with::book_bits,
    )
    .await
    {
        // All or nothing: the address book goes again.
        if let Err(cleanup) = dav.delete_collection(book.id, Contents::Remove).await {
            tracing::warn!(error = %cleanup, "contacts: a half-made address book stays");
        }
        return Err(error);
    }
    if !share_with.is_empty()
        && let Ok(all) = ctx.store().dav_collections(ctx.id()).await
        && let Some(fresh) = all
            .into_iter()
            .find(|c| c.id == book.id && c.orphaned_at.is_none())
    {
        book = fresh;
    }
    if create.is_subscribed == Some(false) {
        ctx.store()
            .set_collection_subscribed(ctx.id(), book.id, false)
            .await
            .map_err(|e| store_set_error(&e))?;
        book.is_subscribed = false;
    }
    let object = to_jmap(
        &book,
        Some(&[
            AddressBookProperty::IsDefault,
            AddressBookProperty::IsSubscribed,
            AddressBookProperty::MyRights,
            AddressBookProperty::ShareWith,
        ]),
    )
    .map_err(|_| forbidden("id"))?;
    let mut object = object;
    if book.props.sharing.mode.is_some() {
        object.share_with = Some(
            crate::sharing::share_with::share_with(ctx, &book, |bits| {
                crate::sharing::share_with::book_rights(bits, bits.contains(Rights::WRITE_ACL))
            })
            .await,
        );
    }
    Ok((book.id, object))
}

/// The PROPPATCH of an update: what to set and what to remove.
fn patch_values(
    patch: &AddressBookPatch,
    current_order: Option<i64>,
) -> Result<(Vec<PropValue>, Vec<Property>), SetError> {
    let mut set = Vec::new();
    let mut remove = Vec::new();
    if let Some(name) = &patch.name {
        set.push(PropValue::DisplayName(name.clone()));
    }
    match &patch.description {
        Some(Some(description)) => set.push(PropValue::AddressbookDescription(description.clone())),
        Some(None) => remove.push(Property::AddressbookDescription),
        None => {}
    }
    if let Some(sort_order) = patch.sort_order {
        let wanted = order(sort_order)?;
        if current_order.unwrap_or(0) != i64::from(wanted) {
            set.push(PropValue::CalendarOrder(wanted));
        }
    }
    Ok((set, remove))
}

async fn update_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    books: &Books,
    id: CollectionId,
    patch: &PatchObject,
) -> Result<(), SetError> {
    let patch = AddressBookPatch::parse(patch)?;
    let book = books.get(&id).ok_or_else(SetError::not_found)?;
    let (set, remove) = patch_values(&patch, book.props.sort_order)?;
    if !(set.is_empty() && remove.is_empty()) {
        if !allows(book, Rights::WRITE_PROPERTIES) {
            return Err(forbidden("this address book can't be changed"));
        }
        dav.update_collection(id, &set, &remove)
            .await
            .map_err(|e| set_error(&e))?;
    }
    if let Some(subscribed) = patch.is_subscribed {
        ctx.store()
            .set_collection_subscribed(ctx.id(), id, subscribed)
            .await
            .map_err(|e| store_set_error(&e))?;
    }
    if let Some(share) = &patch.share_with {
        crate::sharing::share_with::update(ctx, book, share, crate::sharing::share_with::book_bits)
            .await?;
    }
    Ok(())
}

async fn destroy_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    books: &Books,
    id: CollectionId,
    remove_contents: bool,
) -> Result<(), SetError> {
    if !books.contains_key(&id) {
        return Err(SetError::not_found());
    }
    // Quick check on the local copy (a card that doesn't parse counts);
    // then the server is asked, as the copy may miss cards.
    let contents = if remove_contents {
        Contents::Remove
    } else {
        let stored = ctx
            .store()
            .dav_resources(ctx.id(), id)
            .await
            .map_err(|e| store_set_error(&e))?;
        if has_contents(&stored) {
            return Err(SetError::new(SetErrorType::AddressBookHasContents));
        }
        Contents::MustBeEmpty
    };
    dav.delete_collection(id, contents)
        .await
        .map_err(|e| match e {
            GroupwareError::HasContents => SetError::new(SetErrorType::AddressBookHasContents),
            other => set_error(&other),
        })
}

/// The new default `id` in the response: `isDefault: true` in its
/// `created` object if this call made it, else among the `updated`.
fn mark_default<V>(
    created: &mut BTreeMap<CreationId, AddressBook>,
    updated: &mut BTreeMap<AddressBookId, Option<V>>,
    id: AddressBookId,
) {
    if let Some(object) = created
        .values_mut()
        .find(|book| book.id.as_ref() == Some(&id))
    {
        object.is_default = Some(true);
    } else {
        updated.entry(id).or_insert(None);
    }
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: AddressBookSetArguments,
) -> Result<MethodResponse, CallError> {
    let set = args.set;
    ctx.check_account(&set.account_id)?;
    let dav = super::dav_account(ctx)?;
    let old_state = ctx
        .check_state(DataType::AddressBook, set.if_in_state.as_ref())
        .await?;
    let mut created: BTreeMap<CreationId, AddressBook> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in set.create {
        match create_one(ctx, &dav, object).await {
            Ok((id, object)) => {
                ctx.created.insert(creation_id.clone(), ids::book_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let books = books_by_id(ctx).await?;
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &set.update {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_book) {
            Some(row) => update_one(ctx, &dav, &books, row, patch).await,
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
    for key in &set.destroy {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_book) {
            Some(row) => destroy_one(ctx, &dav, &books, row, args.on_destroy_remove_contents).await,
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
    let all_ok = not_created.is_empty() && not_updated.is_empty() && not_destroyed.is_empty();
    if all_ok
        && let Some(default) = &args.on_success_set_is_default
        && let Some(id) = ctx.resolve(default)
        && let Some(row) = ids::store_book(&id)
    {
        // Only a live address book of the user's own (not a calendar, not
        // someone else's); anything else is ignored ("the server will
        // try", RFC 9610 §2.3).
        if default_target(ctx, CollectionKind::AddressBook, row).await? {
            match ctx
                .store()
                .set_default_collection(ctx.id(), row, CollectionKind::AddressBook)
                .await
            {
                Ok(_) => mark_default(&mut created, &mut updated, id),
                Err(error) => tracing::debug!(%error, "onSuccessSetIsDefault: not a live book"),
            }
        } else {
            tracing::debug!("onSuccessSetIsDefault: not one of the user's address books");
        }
    }
    let new_state = ctx.state(DataType::AddressBook).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::AddressBookSet(response))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A book made in the same call gets `isDefault: true` in `created`;
    /// another one is listed as updated.
    #[test]
    fn the_new_default_in_the_response() {
        let id = |s: &str| s.parse::<AddressBookId>().unwrap();
        let mut created = BTreeMap::new();
        created.insert(
            "k1".parse::<CreationId>().unwrap(),
            AddressBook {
                id: Some(id("b7")),
                ..AddressBook::default()
            },
        );
        let mut updated: BTreeMap<AddressBookId, Option<()>> = BTreeMap::new();
        mark_default(&mut created, &mut updated, id("b7"));
        assert_eq!(created.values().next().unwrap().is_default, Some(true));
        assert!(updated.is_empty());
        mark_default(&mut created, &mut updated, id("b3"));
        assert!(updated.contains_key(&id("b3")));
    }

    #[test]
    fn dav_properties() {
        let create = AddressBookCreate::parse(
            json!({"name": "Trabajo", "description": "Clientes", "sortOrder": 2})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert_eq!(
            create_values(&create).unwrap(),
            [
                PropValue::DisplayName("Trabajo".to_owned()),
                PropValue::AddressbookDescription("Clientes".to_owned()),
                PropValue::CalendarOrder(2),
            ]
        );
        let plain =
            AddressBookCreate::parse(json!({"name": "X"}).as_object().unwrap().clone()).unwrap();
        assert_eq!(create_values(&plain).unwrap().len(), 1);

        let patch = |value: Value| {
            AddressBookPatch::parse(&serde_json::from_value(value).unwrap()).unwrap()
        };
        let (set, remove) =
            patch_values(&patch(json!({"name": "Y", "description": null})), None).unwrap();
        assert_eq!(set, [PropValue::DisplayName("Y".to_owned())]);
        assert_eq!(remove, [Property::AddressbookDescription]);
        // The same order is no change; another one is.
        let (set, _) = patch_values(&patch(json!({"sortOrder": 3})), Some(3)).unwrap();
        assert!(set.is_empty());
        let (set, _) = patch_values(&patch(json!({"sortOrder": 0})), None).unwrap();
        assert!(set.is_empty());
        let (set, _) = patch_values(&patch(json!({"sortOrder": 4})), Some(3)).unwrap();
        assert_eq!(set, [PropValue::CalendarOrder(4)]);
        assert_eq!(
            patch_values(&patch(json!({"sortOrder": 3_000_000_000_u64})), None)
                .unwrap_err()
                .properties(),
            ["sortOrder"]
        );
    }
}
