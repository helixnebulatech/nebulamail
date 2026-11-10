// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `ContactCard/set` (RFC 9610 §3.5) over CardDAV:
//!
//! - **create**: the JSContact card (with a `uid` if it has none) as a
//!   vCard (4.0 if the book takes it, else 3.0), PUT as
//!   `{book}/{uuid}.vcf` with `If-None-Match: *`.
//! - **update**: the stored vCard → JSContact → the patch → vCard, PUT
//!   with `If-Match`. A 412 reads the card again from the server and
//!   applies the same patch once more; a second one is `forbidden` ("it
//!   changed meanwhile"). A new `addressBookIds` is a MOVE.
//! - **destroy**: DELETE with `If-Match` (a 412 reads it again and retries
//!   once).
//!
//! Each write is in the store when the response goes out (new etag, new
//! state) and kicks a sync round.

use std::collections::{BTreeMap, BTreeSet};

use nebula_groupware::{GroupwareAccount, GroupwareError};
use nebula_jmap_proto::method::{PatchObject, SetArguments, SetResponse};
use nebula_jmap_proto::object::js_body::{BodyPatch, JsBody, JsContact};
use nebula_jmap_proto::object::{ContactCard, ContactCardCreate, ContactCardPatch};
use nebula_jmap_proto::{
    AddressBookId, ContactCardId, CreationId, IdOrRef, MethodResponse, SetError, SetErrorType,
};
use nebula_store::{Collection, CollectionId, DataType, ResourceId, Rights, StoredResource};
use serde_json::{Map, Value};

use super::book::allows;
use super::card::{Books, books_by_id, card_text, stored_card};
use super::convert::{
    card_text as vcard_text, check_media, new_name, patch_text, uuid, version_for,
};
use super::error::set_error;
use super::ids;
use crate::core::context::Ctx;
use crate::error::CallError;

fn forbidden(description: &str) -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description(description)
}

/// The one book of `addressBookIds`: a card is one CardDAV resource.
fn one_book<'a>(
    ctx: &Ctx<'_>,
    books: &'a Books,
    wanted: &BTreeSet<IdOrRef<AddressBookId>>,
) -> Result<&'a Collection, SetError> {
    let mut iter = wanted.iter();
    let (Some(only), None) = (iter.next(), iter.next()) else {
        return Err(SetError::invalid_properties(["addressBookIds"])
            .with_description("a card is in exactly one address book here"));
    };
    ctx.resolve(only)
        .as_ref()
        .and_then(ids::store_book)
        .and_then(|row| books.get(&row))
        .ok_or_else(|| {
            SetError::invalid_properties(["addressBookIds"])
                .with_description("unknown address book")
        })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, SetError> + Send + 'static,
) -> Result<T, SetError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| forbidden("the card could not be converted"))?
}

/// What a new card's body becomes: its own `uid` (or a new one, which the
/// response returns), checked photos.
fn new_body(body: JsBody<JsContact>) -> Result<(Map<String, Value>, Option<String>), SetError> {
    let mut members = body.into_map();
    check_media(&members)?;
    let generated = if members.contains_key("uid") {
        None
    } else {
        let uid = format!("urn:uuid:{}", uuid()?);
        members.insert("uid".to_owned(), Value::from(uid.clone()));
        Some(uid)
    };
    Ok((members, generated))
}

async fn create_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    books: &Books,
    object: Map<String, Value>,
) -> Result<(ResourceId, ContactCard), SetError> {
    let create = ContactCardCreate::parse(object)?;
    let book = one_book(ctx, books, create.address_book_ids.as_set())?;
    if !allows(book, Rights::BIND) {
        return Err(forbidden("this address book is read-only"));
    }
    let (members, generated) = new_body(create.body)?;
    let version = version_for(book.props.vcard_versions);
    let text = blocking(move || vcard_text(&members, version)).await?;
    let card = dav
        .create_resource(book.id, &new_name()?, text)
        .await
        .map_err(|e| set_error(&e))?;
    let mut created = ContactCard {
        id: Some(ids::card_id(card.id).map_err(|_| forbidden("id"))?),
        ..ContactCard::default()
    };
    if let Some(uid) = generated {
        let mut body = Map::new();
        body.insert("uid".to_owned(), Value::from(uid));
        created.body = JsBody::new(body).map_err(|e| e.to_set_error())?;
    }
    Ok((card.id, created))
}

/// The book the card moves to, if `addressBookIds` changes.
fn target_book<'a>(
    ctx: &Ctx<'_>,
    books: &'a Books,
    current: CollectionId,
    patch: &ContactCardPatch,
) -> Result<Option<&'a Collection>, SetError> {
    let Some(change) = &patch.address_book_ids else {
        return Ok(None);
    };
    let here = ids::book_id(current).map_err(|_| forbidden("id"))?;
    let mut set = BTreeSet::from([IdOrRef::Id(here)]);
    change.apply(&mut set);
    // Patches by member name the current book by id: resolve them all.
    let resolved: BTreeSet<IdOrRef<AddressBookId>> = set
        .iter()
        .filter_map(|id| ctx.resolve(id))
        .map(IdOrRef::Id)
        .collect();
    let target = one_book(ctx, books, &resolved)?;
    Ok((target.id != current).then_some(target))
}

/// The content change, retried once after a 412, on the account's
/// [write turn](GroupwareAccount::write_turn).
async fn update_body(
    dav: &GroupwareAccount,
    books: &Books,
    card: StoredResource,
    patches: Vec<BodyPatch>,
) -> Result<StoredResource, SetError> {
    let fallback = version_for(
        books
            .get(&card.collection)
            .and_then(|book| book.props.vcard_versions),
    );
    let _turn = dav.write_turn().await;
    let mut current = card_text(dav, &card)
        .await
        .map_err(|e| set_error(&e))?
        .ok_or_else(SetError::not_found)?;
    let patches = std::sync::Arc::new(patches);
    for attempt in 0..2 {
        let text = current.text.clone();
        let list = std::sync::Arc::clone(&patches);
        let now = Ctx::now()
            .map(nebula_store::UnixTime::secs)
            .map_err(|_| forbidden("clock"))?;
        let Some(new_text) = blocking(move || patch_text(&text, &list, fallback, now)).await?
        else {
            return Ok(current.resource);
        };
        match dav.replace_resource(&current.resource, new_text).await {
            Ok(stored) => return Ok(stored),
            Err(error) if error.is_conflict() && attempt == 0 => {
                tracing::debug!("contacts: card changed meanwhile; applying the patch again");
                current = dav
                    .refetch(&current.resource)
                    .await
                    .map_err(|e| set_error(&e))?
                    .ok_or_else(SetError::not_found)?;
            }
            Err(error) => return Err(set_error(&error)),
        }
    }
    Err(set_error(&GroupwareError::Dav(
        nebula_dav::DavError::PreconditionFailed,
    )))
}

/// A move to another book, retried once after a 412.
async fn move_card(
    dav: &GroupwareAccount,
    card: StoredResource,
    to: CollectionId,
) -> Result<StoredResource, SetError> {
    match dav.move_resource(&card, to).await {
        Ok(moved) => Ok(moved),
        Err(error) if error.is_conflict() => {
            let fresh = dav
                .refetch(&card)
                .await
                .map_err(|e| set_error(&e))?
                .ok_or_else(SetError::not_found)?;
            dav.move_resource(&fresh.resource, to)
                .await
                .map_err(|e| set_error(&e))
        }
        Err(error) => Err(set_error(&error)),
    }
}

async fn update_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    books: &Books,
    id: ResourceId,
    patch: &PatchObject,
) -> Result<(), SetError> {
    let patch = ContactCardPatch::parse(patch)?;
    let card = stored_card(ctx, books, id)
        .await
        .map_err(|_| forbidden("database error"))?
        .ok_or_else(SetError::not_found)?;
    let from = books
        .get(&card.collection)
        .ok_or_else(SetError::not_found)?;
    let target = target_book(ctx, books, card.collection, &patch)?;
    if !patch.body.is_empty() && !allows(from, Rights::WRITE_CONTENT) {
        return Err(forbidden("this address book is read-only"));
    }
    if let Some(target) = target
        && (!allows(from, Rights::UNBIND) || !allows(target, Rights::BIND))
    {
        return Err(forbidden(
            "the card can't be moved between these address books",
        ));
    }
    let card = if patch.body.is_empty() {
        card
    } else {
        update_body(dav, books, card, patch.body).await?
    };
    if let Some(target) = target {
        move_card(dav, card, target.id).await?;
    }
    Ok(())
}

async fn destroy_one(
    ctx: &Ctx<'_>,
    dav: &GroupwareAccount,
    books: &Books,
    id: ResourceId,
) -> Result<(), SetError> {
    let card = stored_card(ctx, books, id)
        .await
        .map_err(|_| forbidden("database error"))?
        .ok_or_else(SetError::not_found)?;
    if books
        .get(&card.collection)
        .is_some_and(|book| !allows(book, Rights::UNBIND))
    {
        return Err(forbidden("this address book is read-only"));
    }
    match dav.delete_resource(&card).await {
        Ok(()) => Ok(()),
        Err(error) if error.is_conflict() => {
            // Changed elsewhere meanwhile: still destroy what is there now.
            match dav.refetch(&card).await.map_err(|e| set_error(&e))? {
                Some(fresh) => dav
                    .delete_resource(&fresh.resource)
                    .await
                    .map_err(|e| set_error(&e)),
                None => Ok(()),
            }
        }
        Err(error) => Err(set_error(&error)),
    }
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    set: SetArguments<ContactCardId>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&set.account_id)?;
    let dav = super::dav_account(ctx)?;
    let old_state = ctx
        .check_state(DataType::ContactCard, set.if_in_state.as_ref())
        .await?;
    let books = books_by_id(ctx).await?;
    let mut created: BTreeMap<CreationId, ContactCard> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in set.create {
        match create_one(ctx, &dav, &books, object).await {
            Ok((id, object)) => {
                ctx.created.insert(creation_id.clone(), ids::card_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &set.update {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_card) {
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
        let result = match id.as_ref().and_then(ids::store_card) {
            Some(row) => destroy_one(ctx, &dav, &books, row).await,
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
    let new_state = ctx.state(DataType::ContactCard).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::ContactCardSet(response))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn new_cards_get_a_uid() {
        let body = |value: Value| JsBody::new(value.as_object().unwrap().clone()).unwrap();
        let (members, generated) = new_body(body(
            json!({"@type": "Card", "version": "1.0", "uid": "u1"}),
        ))
        .unwrap();
        assert_eq!(members["uid"], "u1");
        assert!(generated.is_none());
        let (members, generated) =
            new_body(body(json!({"@type": "Card", "version": "1.0"}))).unwrap();
        let uid = generated.unwrap();
        assert!(uid.starts_with("urn:uuid:") && uid.len() == 45, "{uid}");
        assert_eq!(members["uid"], uid.as_str());
        assert_eq!(
            new_body(body(json!({"media": {"m": {"blobId": "bu1"}}})))
                .unwrap_err()
                .kind(),
            SetErrorType::InvalidProperties
        );
    }
}
