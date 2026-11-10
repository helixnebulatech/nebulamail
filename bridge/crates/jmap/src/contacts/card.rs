// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `ContactCard/get` and `ContactCard/changes` (RFC 9610 §3): each card's
//! sealed vCard, opened and converted to JSContact with calcard.

use std::collections::{BTreeMap, BTreeSet};

use nebula_groupware::{GroupwareAccount, GroupwareError, ResourceText};
use nebula_jmap_proto::method::{ChangesArguments, GetArguments, GetResponse};
use nebula_jmap_proto::object::js_body::{JsBody, JsContact};
use nebula_jmap_proto::object::{ContactCard, ContactCardProperty};
use nebula_jmap_proto::types::TrueSet;
use nebula_jmap_proto::{ContactCardId, MethodError, MethodResponse};
use nebula_store::{Collection, CollectionId, DataType, ResourceId, StoredResource};

use super::convert::card_body;
use super::error::call_error;
use super::ids;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// The account's books by id.
pub(crate) type Books = BTreeMap<CollectionId, Collection>;

pub(crate) async fn books_by_id(ctx: &Ctx<'_>) -> Result<Books, CallError> {
    Ok(super::books(ctx)
        .await?
        .into_iter()
        .map(|book| (book.id, book))
        .collect())
}

/// A visible card in one of `books` (a calendar event's id is not a
/// card).
pub(crate) async fn stored_card(
    ctx: &Ctx<'_>,
    books: &Books,
    id: ResourceId,
) -> Result<Option<StoredResource>, CallError> {
    Ok(ctx
        .store()
        .dav_resource(ctx.id(), id)
        .await?
        .filter(|card| card.is_visible() && books.contains_key(&card.collection)))
}

/// The text of a card: the local copy, or the server's if the copy does
/// not open. `None` if it is gone.
pub(crate) async fn card_text(
    dav: &GroupwareAccount,
    card: &StoredResource,
) -> Result<Option<ResourceText>, GroupwareError> {
    crate::core::text::readable_text(dav, card).await
}

/// Every visible card of the account's books.
pub(crate) async fn all_cards(
    ctx: &Ctx<'_>,
    books: &Books,
) -> Result<Vec<StoredResource>, CallError> {
    let mut cards = Vec::new();
    for book in books.keys() {
        cards.extend(
            ctx.store()
                .dav_resources(ctx.id(), *book)
                .await?
                .into_iter()
                .filter(StoredResource::is_visible),
        );
    }
    Ok(cards)
}

/// The JMAP object of a card with the properties asked for.
pub(crate) fn to_jmap(
    id: ContactCardId,
    book: CollectionId,
    body: &JsBody<JsContact>,
    properties: Option<&[ContactCardProperty]>,
) -> Result<ContactCard, Bug> {
    let body = match properties {
        None => body.clone(),
        Some(list) => body.select(list.iter().filter_map(|p| match p {
            ContactCardProperty::Body(name) => Some(name.as_str()),
            _ => None,
        })),
    };
    let wants_books =
        properties.is_none_or(|list| list.contains(&ContactCardProperty::AddressBookIds));
    let address_book_ids = if wants_books {
        Some(TrueSet::from(BTreeSet::from([ids::book_id(book)?])))
    } else {
        None
    };
    Ok(ContactCard {
        id: Some(id),
        address_book_ids,
        body,
    })
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<ContactCardId, ContactCardProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = super::dav_account(ctx)?;
    let state = ctx.state(DataType::ContactCard).await?;
    let books = books_by_id(ctx).await?;
    let mut not_found = Vec::new();
    let mut found: Vec<(ContactCardId, StoredResource)> = Vec::new();
    match &args.ids {
        None => {
            let cards = all_cards(ctx, &books).await?;
            let max = ctx.limits.max_objects_in_get;
            if cards.len() > max {
                return Err(MethodError::request_too_large(format!(
                    "more than {max} cards: ask for them by id"
                ))
                .into());
            }
            for card in cards {
                found.push((ids::card_id(card.id)?, card));
            }
        }
        Some(wanted) => {
            for wanted in wanted {
                let Some(id) = ctx.resolve(wanted) else {
                    continue;
                };
                let card = match ids::store_card(&id) {
                    Some(row) => stored_card(ctx, &books, row).await?,
                    None => None,
                };
                match card {
                    Some(card) => found.push((id, card)),
                    None => not_found.push(id),
                }
            }
        }
    }
    let mut texts = Vec::with_capacity(found.len());
    for (id, card) in found {
        match card_text(&dav, &card).await.map_err(call_error)? {
            Some(text) => texts.push((id, text.resource.collection, text.text)),
            None => not_found.push(id),
        }
    }
    let converted = tokio::task::spawn_blocking(move || {
        texts
            .into_iter()
            .map(|(id, book, text)| (id, book, card_body(text.as_bytes())))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| Bug("card conversion"))?;
    let properties = args.properties.as_deref();
    let mut list = Vec::with_capacity(converted.len());
    for (id, book, body) in converted {
        if let Ok(body) = body {
            list.push(to_jmap(id, book, &body, properties)?);
        } else {
            // Indexed when synced, so this is rare (calcard changed?).
            tracing::warn!("contacts: a stored card does not convert");
            not_found.push(id);
        }
    }
    Ok(MethodResponse::ContactCardGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    super::dav_account(ctx)?;
    let response = crate::changes::changes(ctx, DataType::ContactCard, args, ids::card_row).await?;
    let response = crate::sharing::scope::resource_changes(
        ctx,
        nebula_store::CollectionKind::AddressBook,
        response,
        ids::store_card,
    )
    .await?;
    Ok(MethodResponse::ContactCardChanges(response))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn selected_properties() {
        let body: JsBody<JsContact> = JsBody::new(
            json!({"@type": "Card", "version": "1.0", "uid": "u", "name": {"full": "Ana"},
                   "emails": {"e": {"address": "a@example.com"}}})
            .as_object()
            .unwrap()
            .clone(),
        )
        .unwrap();
        let id: ContactCardId = "k3".parse().unwrap();
        let book = CollectionId::from_raw(2).unwrap();
        let all = to_jmap(id.clone(), book, &body, None).unwrap();
        let value = serde_json::to_value(all).unwrap();
        assert_eq!(value["addressBookIds"], json!({"a2": true}));
        assert_eq!(value["emails"]["e"]["address"], "a@example.com");
        let some: Vec<ContactCardProperty> = ["name", "uid", "members"]
            .iter()
            .map(|p| p.parse().unwrap())
            .collect();
        let value = serde_json::to_value(to_jmap(id, book, &body, Some(&some)).unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"id": "k3", "uid": "u", "name": {"full": "Ana"}})
        );
    }
}
