// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `AddressBook/get` and `AddressBook/changes` (RFC 9610 §2) from the
//! synced collections.

use std::collections::BTreeMap;

use nebula_jmap_proto::method::{ChangesArguments, GetArguments, GetResponse};
use nebula_jmap_proto::object::{AddressBook, AddressBookProperty, AddressBookRights};
use nebula_jmap_proto::{AddressBookId, MethodResponse, UnsignedInt};
use nebula_store::{Collection, DataType, Rights};

use super::ids;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// What the user may do, from `current-user-privilege-set` (all of it but
/// sharing if the server did not say); sharing where the server lets the
/// user ([`crate::sharing`]).
pub(crate) fn rights(book: &Collection) -> AddressBookRights {
    let may_share = book.props.sharing.mode.is_some();
    let Some(rights) = book.props.rights else {
        return AddressBookRights {
            may_share,
            ..AddressBookRights::ALL
        };
    };
    crate::sharing::share_with::book_rights(rights, may_share)
}

/// May create cards in it (`bind`), change them (`write-content`) or
/// delete them (`unbind`). Unknown rights: let the server decide.
pub(crate) fn allows(book: &Collection, right: Rights) -> bool {
    book.props
        .rights
        .is_none_or(|rights| rights.contains(right))
}

/// The name shown: the server's, else the last part of its href.
fn name(book: &Collection) -> String {
    book.props.name.clone().unwrap_or_else(|| {
        book.props
            .href
            .as_str()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|segment| !segment.is_empty())
            .unwrap_or("Contacts")
            .to_owned()
    })
}

/// The JMAP form of a book, with the properties asked for.
pub(crate) fn to_jmap(
    book: &Collection,
    properties: Option<&[AddressBookProperty]>,
) -> Result<AddressBook, Bug> {
    let wants = |p: AddressBookProperty| properties.is_none_or(|list| list.contains(&p));
    let mut out = AddressBook {
        id: Some(ids::book_id(book.id)?),
        ..AddressBook::default()
    };
    if wants(AddressBookProperty::Name) {
        out.name = Some(name(book));
    }
    if wants(AddressBookProperty::Description) {
        out.description = Some(book.props.description.clone());
    }
    if wants(AddressBookProperty::SortOrder) {
        let order = book
            .props
            .sort_order
            .and_then(|order| u64::try_from(order).ok())
            .unwrap_or(0);
        out.sort_order = Some(UnsignedInt::saturating(order));
    }
    if wants(AddressBookProperty::IsDefault) {
        out.is_default = Some(book.is_default);
    }
    if wants(AddressBookProperty::IsSubscribed) {
        out.is_subscribed = Some(book.is_subscribed);
    }
    if wants(AddressBookProperty::ShareWith) {
        // `null` where the user can't share it; `get` fills it in.
        out.share_with = Some(book.props.sharing.mode.map(|_| BTreeMap::new()));
    }
    if wants(AddressBookProperty::MyRights) {
        out.my_rights = Some(rights(book));
    }
    Ok(out)
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<AddressBookId, AddressBookProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    super::dav_account(ctx)?;
    let state = ctx.state(DataType::AddressBook).await?;
    let books = super::books(ctx).await?;
    let properties = args.properties.as_deref();
    let wants_shares = properties.is_none_or(|list| list.contains(&AddressBookProperty::ShareWith));
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    let one = async |book: &Collection| -> Result<AddressBook, CallError> {
        let mut out = to_jmap(book, properties)?;
        if wants_shares && book.props.sharing.mode.is_some() {
            out.share_with = Some(
                crate::sharing::share_with::share_with(ctx, book, |bits| {
                    crate::sharing::share_with::book_rights(bits, bits.contains(Rights::WRITE_ACL))
                })
                .await,
            );
        }
        Ok(out)
    };
    match &args.ids {
        None => {
            for book in &books {
                list.push(one(book).await?);
            }
        }
        Some(wanted) => {
            for wanted in wanted {
                let Some(id) = ctx.resolve(wanted) else {
                    continue;
                };
                let found = ids::store_book(&id).and_then(|row| books.iter().find(|b| b.id == row));
                match found {
                    Some(book) => list.push(one(book).await?),
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::AddressBookGet(GetResponse {
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
    let response = crate::changes::changes(ctx, DataType::AddressBook, args, ids::book_row).await?;
    let response = crate::sharing::scope::collection_changes(
        ctx,
        nebula_store::CollectionKind::AddressBook,
        response,
        ids::store_book,
    )
    .await?;
    Ok(MethodResponse::AddressBookChanges(response))
}

#[cfg(test)]
mod tests {
    use nebula_store::{CollectionId, CollectionKind, CollectionProps, CollectionSync, DavHref};

    use super::*;

    fn book(rights: Option<Rights>, name: Option<&str>) -> Collection {
        Collection {
            id: CollectionId::from_raw(4).unwrap(),
            props: CollectionProps {
                kind: CollectionKind::AddressBook,
                href: DavHref::new("https://dav.example/card/alice/work/").unwrap(),
                name: name.map(str::to_owned),
                color: None,
                description: Some("Clientes".to_owned()),
                sort_order: Some(-3),
                time_zone: None,
                default_alarms_datetime: None,
                default_alarms_date: None,
                rights,
                components: None,
                vcard_versions: None,
                supports_sync: true,
                sharing: nebula_store::CollectionSharing::default(),
                extras: nebula_store::CalendarExtras::default(),
            },
            sync: CollectionSync::default(),
            orphaned_at: None,
            is_visible: true,
            is_subscribed: true,
            is_default: false,
        }
    }

    #[test]
    fn jmap_form() {
        let read_only = book(Some(Rights::READ), None);
        let object = serde_json::to_value(to_jmap(&read_only, None).unwrap()).unwrap();
        assert_eq!(
            object,
            serde_json::json!({
                "id": "a4", "name": "work", "description": "Clientes", "sortOrder": 0,
                "isDefault": false, "isSubscribed": true, "shareWith": null,
                "myRights": {"mayRead": true, "mayWrite": false, "mayShare": false, "mayDelete": false}
            })
        );
        assert!(!allows(&read_only, Rights::BIND));
        let unknown = book(None, Some("Trabajo"));
        assert!(allows(&unknown, Rights::UNBIND));
        assert!(rights(&unknown).may_delete && !rights(&unknown).may_share);
        let only_name = to_jmap(&unknown, Some(&[AddressBookProperty::Name])).unwrap();
        assert_eq!(
            serde_json::to_value(only_name).unwrap(),
            serde_json::json!({"id": "a4", "name": "Trabajo"})
        );
    }
}
