// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP for Contacts (RFC 9610) over CardDAV.
//!
//! - **Reads** come from the store (what `nebula-groupware` synced):
//!   `AddressBook/get|changes` and `ContactCard/query|changes` from the
//!   indexes, `ContactCard/get` from the sealed vCard of each card,
//!   converted to JSContact with calcard.
//! - **Writes** go to the DAV server first and to the store right after
//!   (`nebula_groupware::GroupwareAccount`, `write/`): a new card is a PUT
//!   of `{book}/{uuid}.vcf` with `If-None-Match: *`; a change starts from
//!   the stored vCard (calcard keeps what it does not convert in
//!   `vCard.properties`), applies the JMAP patch to its JSContact form
//!   (`JsBody::apply`) and PUTs it back with `If-Match`; a 412 reads the
//!   card again and applies the patch once more. Moving to another book is
//!   a MOVE; destroying, a DELETE with `If-Match`.
//! - **Books**: extended MKCOL, PROPPATCH (`name` → `displayname`,
//!   `description` → `addressbook-description`, `sortOrder` → Apple's
//!   `calendar-order`, which servers keep as a dead property) and DELETE.
//!   `isSubscribed` and `isDefault` are only the bridge's. No sharing:
//!   `shareWith` is `null` and setting it is `invalidProperties`.
//! - A card is in exactly one book (`maxAddressBooksPerCard` 1): that is
//!   what a CardDAV resource is.
//! - `ContactCard/parse` reads any blob with calcard; it needs no DAV.
//!
//! Ids: `a{n}` for books and `k{n}` for cards (the store's collection and
//! resource ids). The capabilities are announced per account only while
//! its DAV discovery found an address book home and the login works.

mod book;
mod card;
pub(crate) mod convert;
mod error;
mod ids;
mod parse;
mod query;
mod set_book;
mod set_card;

use std::sync::Arc;

use nebula_groupware::GroupwareAccount;
use nebula_jmap_proto::session::ContactsAccountCapability;
use nebula_jmap_proto::{MethodCall, MethodError, MethodErrorType, MethodResponse, UnsignedInt};
use nebula_store::{AccountId, Collection, CollectionKind};

use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::error::CallError;

/// The contacts capability of an account: only while its DAV account
/// found an address book home and was not refused.
pub(crate) fn capability(inner: &Inner, account: AccountId) -> Option<ContactsAccountCapability> {
    let running = inner.groupware.as_ref()?.account(account)?;
    running
        .availability()
        .contacts
        .then_some(ContactsAccountCapability {
            max_address_books_per_card: Some(UnsignedInt::from(1_u32)),
            may_create_address_book: true,
        })
}

/// The running DAV account of the request's account, if it has contacts.
pub(crate) fn dav_account(ctx: &Ctx<'_>) -> Result<Arc<GroupwareAccount>, CallError> {
    let running = ctx
        .inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(ctx.id()));
    match running {
        Some(account) if account.availability().contacts => Ok(account),
        _ => Err(
            MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
                .with_description("this account has no contacts now")
                .into(),
        ),
    }
}

/// The address books of the call's account (the user's own, or those
/// someone shares in their account) that are on the server.
pub(crate) async fn books(ctx: &Ctx<'_>) -> Result<Vec<Collection>, CallError> {
    Ok(ctx
        .store()
        .dav_collections(ctx.id())
        .await?
        .into_iter()
        .filter(|c| {
            c.props.kind == CollectionKind::AddressBook
                && c.orphaned_at.is_none()
                && ctx.scope.contains(c)
        })
        .collect())
}

/// Runs a contacts method.
pub(crate) async fn handle(
    ctx: &mut Ctx<'_>,
    call: MethodCall,
) -> Result<MethodResponse, CallError> {
    match call {
        MethodCall::AddressBookGet(args) => book::get(ctx, args).await,
        MethodCall::AddressBookChanges(args) => book::changes(ctx, args).await,
        MethodCall::AddressBookSet(args) => set_book::set(ctx, args).await,
        MethodCall::ContactCardGet(args) => card::get(ctx, args).await,
        MethodCall::ContactCardChanges(args) => card::changes(ctx, args).await,
        MethodCall::ContactCardQuery(args) => query::query(ctx, args).await,
        MethodCall::ContactCardQueryChanges(args) => query::query_changes(ctx, args).await,
        MethodCall::ContactCardSet(args) => set_card::set(ctx, args).await,
        MethodCall::ContactCardParse(args) => parse::parse(ctx, args).await,
        _ => Err(MethodError::unknown_method().into()),
    }
}
