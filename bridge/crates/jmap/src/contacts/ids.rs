// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Ids of books (`a` + the store's collection id) and cards (`k` + the
//! resource id), parsed as strictly as the mail ids (`core::ids`).

use nebula_jmap_proto::{AddressBookId, ContactCardId};
use nebula_store::{CollectionId, ResourceId};

use crate::core::ids::parse_number;
use crate::error::Bug;

pub(crate) fn book_id(id: CollectionId) -> Result<AddressBookId, Bug> {
    format!("a{}", id.get())
        .parse()
        .map_err(|_| Bug("address book id"))
}

pub(crate) fn store_book(id: &AddressBookId) -> Option<CollectionId> {
    parse_number(id.as_str(), 'a').and_then(CollectionId::from_raw)
}

pub(crate) fn card_id(id: ResourceId) -> Result<ContactCardId, Bug> {
    format!("k{}", id.get())
        .parse()
        .map_err(|_| Bug("contact card id"))
}

pub(crate) fn store_card(id: &ContactCardId) -> Option<ResourceId> {
    parse_number(id.as_str(), 'k').and_then(ResourceId::from_raw)
}

/// A change-log row id as a book id.
pub(crate) fn book_row(raw: i64) -> Result<AddressBookId, Bug> {
    CollectionId::from_raw(raw)
        .ok_or(Bug("collection id"))
        .and_then(book_id)
}

/// A change-log row id as a card id.
pub(crate) fn card_row(raw: i64) -> Result<ContactCardId, Bug> {
    ResourceId::from_raw(raw)
        .ok_or(Bug("resource id"))
        .and_then(card_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_strict() {
        let book = CollectionId::from_raw(12).unwrap();
        assert_eq!(book_id(book).unwrap().as_str(), "a12");
        assert_eq!(store_book(&book_id(book).unwrap()), Some(book));
        let card = ResourceId::from_raw(7).unwrap();
        assert_eq!(card_id(card).unwrap().as_str(), "k7");
        assert_eq!(store_card(&"k7".parse().unwrap()), Some(card));
        for bad in ["a0", "a01", "k12", "b1", "a", "A1"] {
            assert_eq!(store_book(&bad.parse().unwrap()), None, "{bad}");
        }
        assert_eq!(store_card(&"a7".parse().unwrap()), None);
        assert_eq!(book_row(3).unwrap().as_str(), "a3");
        assert!(card_row(0).is_err());
    }
}
