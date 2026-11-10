// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendars and contacts synced from CalDAV/CardDAV:
//! what discovery found, the collections and their resources. The text of
//! each resource is sealed by the caller with the account's data key; only
//! the indexes (`UID`, UTC start and end, sort name) are in clear. Every
//! write records its JMAP changes (`Calendar`, `CalendarEvent`,
//! `AddressBook`, `ContactCard`) in the same transaction.

mod account;
mod alerts;
mod collection;
mod local;
mod lookup;
mod resource;
mod types;

pub use alerts::FiredAlert;
pub use lookup::ResourceQuery;
pub use types::{
    CalendarExtras, Collection, CollectionKind, CollectionProps, CollectionSharing, CollectionSync,
    Components, DavAccount, DavHref, ExternalUrl, MAX_SHARES, Parsed, ResourceIndex, ResourceWrite,
    Rights, SealedLink, ShareMode, Sharee, StoredResource, VcardVersions,
};

#[cfg(test)]
mod tests;
