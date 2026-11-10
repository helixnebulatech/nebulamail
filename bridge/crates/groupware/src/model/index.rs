// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What the DAV client found, as the store keeps it: collection
//! properties, and each resource sealed with its clear-text indexes.

use nebula_crypto::AccountKey;
use nebula_dav::proto::{Component, Href, VcardRequest};
use nebula_dav::{Collection, CollectionKind, ResourceKind};
use nebula_store::{
    CalendarExtras, CollectionId, CollectionKind as StoredKind, CollectionProps, Components,
    DavHref, ExternalUrl, Parsed, ResourceIndex, ResourceWrite, Rights, UnixTime, VcardVersions,
};

use crate::GroupwareError;
use crate::model::convert::{icalendar_index, vcard_index};
use crate::model::defaults::{DefaultAlerts, calendar_zone};

/// Longest display name or description kept, in characters.
const MAX_TEXT: usize = 1024;

/// The store's form of an href.
///
/// # Errors
/// [`GroupwareError::Store`] for an href the store refuses (never for one
/// the DAV client accepted).
pub(crate) fn stored_href(href: &Href) -> Result<DavHref, GroupwareError> {
    Ok(DavHref::new(href.url().as_str())?)
}

pub(crate) const fn stored_kind(kind: CollectionKind) -> StoredKind {
    match kind {
        CollectionKind::Calendar => StoredKind::Calendar,
        CollectionKind::Addressbook => StoredKind::AddressBook,
    }
}

fn text(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.chars().take(MAX_TEXT).collect())
}

fn rights(collection: &Collection) -> Option<Rights> {
    // A subscription is read-only whatever the server says, but the user
    // may rename it and remove it (unsubscribe).
    if collection.subscription.is_some() {
        return Some(
            Rights::READ
                .union(Rights::READ_FREE_BUSY)
                .union(Rights::WRITE_PROPERTIES)
                .union(Rights::UNBIND),
        );
    }
    collection.privileges.map(crate::sharing::rights_of)
}

/// Nextcloud's extras as listed: the public link and whether it is a
/// subscription, the links sealed ([`links`](crate::model::links)),
/// reusing the bytes of `previous` (what the store has for that href) for
/// a link that did not change. `publishable` is decided with the
/// account's sharing (see `sync_round`).
fn extras(
    collection: &Collection,
    key: &AccountKey,
    href: &DavHref,
    previous: Option<&CalendarExtras>,
) -> Result<CalendarExtras, GroupwareError> {
    use crate::model::links::{Link, seal_or_keep};
    let url = |url: &nebula_dav::proto::ExternalUrl| ExternalUrl::new(url.as_str());
    let publish_url = collection.publish_url.as_ref().map(url).transpose()?;
    let source = collection
        .subscription
        .as_ref()
        .and_then(|s| s.source.as_ref())
        .map(url)
        .transpose()?;
    Ok(CalendarExtras {
        publishable: false,
        publish_url: seal_or_keep(
            key,
            href,
            Link::Publish,
            publish_url.as_ref(),
            previous.and_then(|p| p.publish_url.as_ref()),
        )?,
        is_subscription: collection.subscription.is_some(),
        subscription_source: seal_or_keep(
            key,
            href,
            Link::Source,
            source.as_ref(),
            previous.and_then(|p| p.subscription_source.as_ref()),
        )?,
    })
}

fn components(collection: &Collection) -> Option<Components> {
    let set = collection.components.as_ref()?;
    Some(
        set.components()
            .iter()
            .fold(Components::empty(), |all, component| {
                all.union(match component {
                    Component::Event => Components::EVENT,
                    Component::Todo => Components::TODO,
                    Component::Journal => Components::JOURNAL,
                    Component::FreeBusy => Components::FREEBUSY,
                    Component::Availability => Components::AVAILABILITY,
                })
            }),
    )
}

/// The vCard versions (as text, `text/vcard`) an address book lists in
/// `supported-address-data`; jCard and other types do not count.
fn vcard_versions(collection: &Collection) -> Option<VcardVersions> {
    let types = collection.address_data.as_ref()?;
    Some(
        types
            .iter()
            .filter(|data| data.is_vcard())
            .fold(VcardVersions::empty(), |all, data| {
                match data.version.trim() {
                    "3.0" => all.union(VcardVersions::V3),
                    "4.0" => all.union(VcardVersions::V4),
                    _ => all,
                }
            }),
    )
}

/// The vCard to ask for when reading cards of a book that takes
/// `versions`: 4.0 if it lists it (Nextcloud converts a stored 3.0 card
/// back to 4.0 then), else as stored.
pub(crate) fn vcard_request(versions: Option<VcardVersions>) -> VcardRequest {
    if versions.is_some_and(|versions| versions.contains(VcardVersions::V4)) {
        VcardRequest::Version4
    } else {
        VcardRequest::AsStored
    }
}

/// The bridge's own form of a `default-alarm-vevent-*` value (`None`:
/// no alarms it understands).
fn default_alarms(text: Option<&str>) -> Option<String> {
    let alerts = DefaultAlerts::from_valarms(text?);
    (!alerts.is_empty()).then(|| alerts.to_valarms())
}

/// The store's form of a listed collection, its links sealed with `key`
/// (see [`extras`]; `previous` is what the store has for its href).
///
/// # Errors
/// As [`stored_href`]; [`GroupwareError::Crypto`] if sealing fails.
pub(crate) fn collection_props(
    collection: &Collection,
    key: &AccountKey,
    previous: Option<&CalendarExtras>,
) -> Result<CollectionProps, GroupwareError> {
    let href = stored_href(&collection.href)?;
    Ok(CollectionProps {
        kind: stored_kind(collection.kind),
        extras: extras(collection, key, &href, previous)?,
        href,
        name: text(collection.display_name.as_ref()),
        color: collection.color.map(|color| color.to_string()),
        description: text(collection.description.as_ref()),
        sort_order: collection.order,
        time_zone: calendar_zone(
            collection.timezone_id.as_deref(),
            collection.timezone.as_deref(),
        ),
        default_alarms_datetime: default_alarms(collection.default_alarms_datetime.as_deref()),
        default_alarms_date: default_alarms(collection.default_alarms_date.as_deref()),
        rights: rights(collection),
        components: components(collection),
        vcard_versions: vcard_versions(collection),
        supports_sync: collection.supports_sync(),
        sharing: nebula_store::CollectionSharing::default(),
    })
}

/// A fetched resource of `collection`, ready to store: its indexes (or
/// that it does not parse; it is kept anyway) and its text
/// sealed with the account's data key for that collection and href
/// ([`sealed`](crate::model::sealed)). CPU work: run it off the async
/// threads.
///
/// # Errors
/// [`GroupwareError::Crypto`] if sealing fails; [`GroupwareError::Store`]
/// for an href the store refuses.
pub(crate) fn resource_write(
    key: &AccountKey,
    collection: CollectionId,
    kind: ResourceKind,
    href: &Href,
    etag: Option<String>,
    data: &str,
    max_occurrences: usize,
) -> Result<ResourceWrite, GroupwareError> {
    let bytes = data.as_bytes();
    let parsed = resource_parsed(kind, data, max_occurrences);
    let href = stored_href(href)?;
    let sealed = crate::model::sealed::seal(key, collection, &href, bytes)?;
    Ok(ResourceWrite {
        href,
        etag,
        sealed: sealed.as_bytes().to_vec(),
        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        parsed,
    })
}

/// The indexes of a resource's text (or that it does not parse). CPU
/// work: run it off the async threads.
pub(crate) fn resource_parsed(kind: ResourceKind, data: &str, max_occurrences: usize) -> Parsed {
    let bytes = data.as_bytes();
    match kind {
        ResourceKind::Calendar => icalendar_index(bytes, max_occurrences).map(|index| {
            Parsed::Valid(ResourceIndex {
                uid: index.uid,
                utc_start: index.utc_start.map(UnixTime::from_secs),
                utc_end: index.utc_end.map(UnixTime::from_secs),
                recurring: index.recurring,
                sort_name: None,
            })
        }),
        ResourceKind::Contact => vcard_index(bytes).map(|index| {
            Parsed::Valid(ResourceIndex {
                uid: index.uid,
                sort_name: index.sort_name,
                ..ResourceIndex::default()
            })
        }),
    }
    .unwrap_or_else(|error| {
        // The error never carries content; the href stays out of logs.
        tracing::info!(%error, "dav: a resource does not parse; kept but not shown");
        Parsed::Invalid
    })
}
