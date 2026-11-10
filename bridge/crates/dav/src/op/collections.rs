// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The calendars and address books of a home: one PROPFIND Depth 1 with
//! everything the sync needs.
//!
//! **Split listing.** Who a collection is shared with (`DAV:acl`,
//! `CS:invite`, `O:invite`) can be big: thousands of sharees on several
//! collections pass the multistatus limits, and the whole listing would
//! fail. When it does ([`DavError::is_oversized`]), the home is listed
//! again without those three, and each collection that may have them is
//! asked for them alone (a PROPFIND Depth 0), within a budget per round:
//!
//! - at most [`Limits::max_collections`](crate::Limits::max_collections)
//!   requests;
//! - their answers together at most twice
//!   [`Limits::max_multistatus`](crate::Limits::max_multistatus) bytes
//!   (each at most what is left), so memory stays bounded however big the
//!   XML;
//! - at most [`ListOptions::share_items`] ACEs and invited users kept
//!   (the caller's per-account budget).
//!
//! Calendar Server copies (`CS:shared`) go first and their users do not
//! count in `share_items`: their invitation says whose they are. Then the
//! user's own, then the rest, by href. A collection whose sharing was not
//! read (budget spent, its answer too big, not found) says so in
//! [`Collection::sharing_data`]: the caller must not take it for "shared
//! with nobody" (fail closed).

use std::cmp::Ordering;

use nebula_dav_proto::{
    Acl, AddressDataType, CTag, CalendarColor, ComponentSet, Depth, ExternalUrl, Href, Invite,
    OcInvite, Privilege, PrivilegeSet, Properties, Property, Report, ReportSet, SharingModes,
    SyncToken,
};

use crate::op::resource::ResourceKind;
use crate::op::response_for;
use crate::{DavClient, DavError};

/// Whether a listing read a collection's sharing (`DAV:acl`, `CS:invite`,
/// `O:invite`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingData {
    /// Asked for and read (absent ones were not shown by the server).
    Fetched,
    /// Not asked for, or not read (a split listing out of budget, or the
    /// collection's own answer failed).
    NotFetched,
    /// Its answer was too big to read.
    TooLarge,
}

/// How a home is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOptions {
    /// Go straight to the split listing (the whole one was too big
    /// before).
    pub split: bool,
    /// Most ACEs and invited users kept in a split listing (Calendar
    /// Server copies left out).
    pub share_items: usize,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            split: false,
            share_items: 50_000 + nebula_dav_proto::MAX_ACES,
        }
    }
}

/// A home listed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HomeListing {
    /// Its calendars and address books.
    pub collections: Vec<Collection>,
    /// Listed split (sharing asked for per collection).
    pub split: bool,
}

/// What a collection holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    /// A calendar (`C:calendar`).
    Calendar,
    /// An address book (`CR:addressbook`).
    Addressbook,
}

impl CollectionKind {
    /// The kind of its resources.
    #[must_use]
    pub const fn resource(self) -> ResourceKind {
        match self {
            Self::Calendar => ResourceKind::Calendar,
            Self::Addressbook => ResourceKind::Contact,
        }
    }
}

/// A calendar or address book.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Collection {
    /// Where it is (a direct member of its home, with a final slash).
    pub href: Href,
    /// Calendar or address book.
    pub kind: CollectionKind,
    /// `displayname`.
    pub display_name: Option<String>,
    /// `calendar-description` or `addressbook-description`.
    pub description: Option<String>,
    /// `calendar-color`.
    pub color: Option<CalendarColor>,
    /// `calendar-order`.
    pub order: Option<i64>,
    /// `supported-calendar-component-set` (`None`: any component).
    pub components: Option<ComponentSet>,
    /// `calendar-timezone`.
    pub timezone: Option<String>,
    /// `calendar-timezone-id` (RFC 7809).
    pub timezone_id: Option<String>,
    /// `default-alarm-vevent-datetime` (`VALARM`s, or `""` for none).
    pub default_alarms_datetime: Option<String>,
    /// `default-alarm-vevent-date`.
    pub default_alarms_date: Option<String>,
    /// `current-user-privilege-set` (`None`: the server did not say).
    pub privileges: Option<PrivilegeSet>,
    /// `sync-token`.
    pub sync_token: Option<SyncToken>,
    /// `getctag`.
    pub ctag: Option<CTag>,
    /// `supported-address-data`.
    pub address_data: Option<Vec<AddressDataType>>,
    /// `supported-report-set`.
    pub reports: ReportSet,
    /// `DAV:owner`.
    pub owner: Option<Href>,
    /// `DAV:acl` (only if the user may read it).
    pub acl: Option<Acl>,
    /// `CS:invite` (Calendar Server sharing).
    pub invite: Option<Invite>,
    /// `CS:allowed-sharing-modes`.
    pub sharing_modes: Option<SharingModes>,
    /// `O:invite` (Nextcloud sharing; empty for a sharee).
    pub oc_invite: Option<OcInvite>,
    /// Whether `acl`, `invite` and `oc_invite` were read: when not, their
    /// absence says nothing.
    pub sharing_data: SharingData,
    /// `CS:shared` in `resourcetype`: a calendar of someone else, shared
    /// with the user (the copy in the user's home).
    pub shared_with_user: bool,
    /// Listed as a direct member of the home. Stalwart's "assisted
    /// discovery" lists the collections other users share with the user
    /// in the user's home too, where they are not (`/dav/cal/alice/x/` in
    /// bob's `/dav/cal/bob/`): those come with `false`, on the home's
    /// origin only. Who uses them decides (only someone else's).
    pub in_home: bool,
    /// `CS:publish-url`: the public link of a calendar Nextcloud
    /// publishes (only shown, never fetched).
    pub publish_url: Option<ExternalUrl>,
    /// A webcal subscription that Nextcloud serves as a calendar (its
    /// cached copy; see [`DavClient::webcal_cache`]), with its source
    /// when it is a valid [`ExternalUrl`].
    pub subscription: Option<Subscription>,
}

/// A webcal subscription: read-only, its events fetched by the DAV
/// server from `source`, never by the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// `CS:source`.
    pub source: Option<ExternalUrl>,
}

impl Collection {
    /// A collection of `kind` at `href`, from the properties of its
    /// listing (also for tests of the layers above).
    #[must_use]
    pub fn from_properties(href: Href, kind: CollectionKind, p: Properties, in_home: bool) -> Self {
        Self {
            href,
            kind,
            display_name: p.display_name,
            description: match kind {
                CollectionKind::Calendar => p.calendar_description,
                CollectionKind::Addressbook => p.addressbook_description,
            },
            color: p.calendar_color,
            order: p.calendar_order,
            components: p.supported_components,
            timezone: p.calendar_timezone,
            timezone_id: p.calendar_timezone_id,
            default_alarms_datetime: p.default_alarm_datetime,
            default_alarms_date: p.default_alarm_date,
            privileges: p.privileges,
            sync_token: p.sync_token,
            ctag: p.ctag,
            address_data: p.supported_address_data,
            reports: p.supported_reports.unwrap_or_default(),
            owner: p.owner,
            acl: p.acl,
            invite: p.invite,
            sharing_modes: p.sharing_modes,
            oc_invite: p.oc_invite,
            sharing_data: SharingData::Fetched,
            shared_with_user: p.resource_type.is_some_and(|kind| kind.shared),
            in_home,
            publish_url: p.publish_url,
            subscription: None,
        }
    }

    /// Has `sync-collection` (RFC 6578): announced in the report set, or a
    /// sync token is there (some servers do not list their reports).
    #[must_use]
    pub fn supports_sync(&self) -> bool {
        self.reports.contains(Report::SyncCollection) || self.sync_token.is_some()
    }

    /// Whether it may have sharing to read: a Calendar Server copy (its
    /// invitation says whose it is), or one the user may read the ACL of,
    /// change it, or share (`CS:can-be-shared` too).
    fn may_have_sharing(&self) -> bool {
        let privileges = self.privileges.unwrap_or_default();
        self.shared_with_user
            || privileges.allows(Privilege::ReadAcl)
            || privileges.allows(Privilege::WriteAcl)
            || privileges.allows(Privilege::Share)
            || self.sharing_modes.is_some_and(|m| m.can_be_shared)
    }

    /// ACEs and invited users read.
    fn sharing_items(&self) -> usize {
        self.acl.as_ref().map_or(0, |acl| acl.aces.len())
            + self.invite.as_ref().map_or(0, |i| i.users.len())
            + self.oc_invite.as_ref().map_or(0, |i| i.users.len())
    }

    fn clear_sharing(&mut self, data: SharingData) {
        self.acl = None;
        self.invite = None;
        self.oc_invite = None;
        self.sharing_data = data;
    }
}

/// The order sharing is asked for in a split listing: Calendar Server
/// copies, then the user's own, then the rest; by href within each.
fn sharing_order(a: &Collection, b: &Collection) -> Ordering {
    let rank = |c: &Collection| {
        if c.shared_with_user {
            0_u8
        } else if c.in_home {
            1
        } else {
            2
        }
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| a.href.url().as_str().cmp(b.href.url().as_str()))
}

/// Who a collection is shared with: left out of a split listing, asked
/// for per collection.
const SHARING_PROPERTIES: [Property; 3] = [Property::Acl, Property::Invite, Property::OcInvite];

/// What a home listing asks for.
const COLLECTION_PROPERTIES: [Property; 23] = [
    Property::ResourceType,
    Property::DisplayName,
    Property::CalendarDescription,
    Property::AddressbookDescription,
    Property::CalendarColor,
    Property::CalendarOrder,
    Property::SupportedCalendarComponentSet,
    Property::CalendarTimezone,
    Property::CalendarTimezoneId,
    Property::DefaultAlarmVeventDatetime,
    Property::DefaultAlarmVeventDate,
    Property::CurrentUserPrivilegeSet,
    Property::SyncToken,
    Property::GetCTag,
    Property::SupportedAddressData,
    Property::SupportedReportSet,
    Property::Owner,
    Property::Acl,
    Property::Invite,
    Property::AllowedSharingModes,
    Property::OcInvite,
    Property::PublishUrl,
    // Nextcloud only says a calendar is in its trash bin
    // (`NC:deleted-calendar` in `resourcetype`) when this is asked for
    // too; otherwise it lists it as a live calendar.
    Property::DeletedAt,
];

impl DavClient {
    /// The calendars and address books directly inside `home`, and those
    /// the server lists elsewhere on the home's origin (marked
    /// [`Collection::in_home`] `false`). Anything else (the home itself,
    /// inboxes, outboxes, subscriptions, other collections, other origins)
    /// is left out. Split if the whole listing is too big (see
    /// [`Self::list_home`], with the default options).
    ///
    /// # Errors
    ///
    /// As [`Self::list_home`].
    pub async fn list_collections(&self, home: &Href) -> Result<Vec<Collection>, DavError> {
        Ok(self
            .list_home(home, ListOptions::default())
            .await?
            .collections)
    }

    /// The collections of `home`, as [`Self::list_collections`]: one
    /// PROPFIND Depth 1 with everything, or split (the module's docs)
    /// when that answer is too big or `options.split` says it was before.
    ///
    /// There is no limit on how many: what others share is listed here
    /// too, so a limit that failed would let anyone who can share stop
    /// the account's sync. The body and node limits bound the memory; the
    /// caller keeps at most
    /// [`Limits::max_collections`](crate::Limits::max_collections), its
    /// own first.
    ///
    /// # Errors
    ///
    /// [`DavError`] for the listing (also the split one too big); for a
    /// collection's own sharing request, anything but too big or not
    /// found.
    pub async fn list_home(
        &self,
        home: &Href,
        options: ListOptions,
    ) -> Result<HomeListing, DavError> {
        let home = home.as_collection();
        if !options.split {
            match self
                .propfind(&home, Depth::One, &COLLECTION_PROPERTIES)
                .await
            {
                Ok((multistatus, _)) => {
                    let collections = self.collections_of(&home, multistatus).await?;
                    return Ok(HomeListing {
                        collections,
                        split: false,
                    });
                }
                Err(error) if error.is_oversized() => {
                    tracing::warn!("dav: home listing too big, asking for sharing apart");
                }
                Err(error) => return Err(error),
            }
        }
        let light: Vec<Property> = COLLECTION_PROPERTIES
            .iter()
            .copied()
            .filter(|p| !SHARING_PROPERTIES.contains(p))
            .collect();
        let (multistatus, _) = self.propfind(&home, Depth::One, &light).await?;
        let mut collections = self.collections_of(&home, multistatus).await?;
        for collection in &mut collections {
            collection.clear_sharing(SharingData::NotFetched);
        }
        self.read_sharing(&mut collections, options.share_items)
            .await?;
        Ok(HomeListing {
            collections,
            split: true,
        })
    }

    /// A split listing's sharing, per collection, within the budget (the
    /// module's docs).
    async fn read_sharing(
        &self,
        collections: &mut [Collection],
        share_items: usize,
    ) -> Result<(), DavError> {
        let limits = self.server().limits;
        let mut order: Vec<usize> = (0..collections.len())
            .filter(|at| {
                collections
                    .get(*at)
                    .is_some_and(Collection::may_have_sharing)
            })
            .collect();
        order.sort_by(|a, b| match (collections.get(*a), collections.get(*b)) {
            (Some(a), Some(b)) => sharing_order(a, b),
            _ => Ordering::Equal,
        });
        let mut requests = 0_usize;
        let mut bytes_left = limits.max_multistatus.saturating_mul(2);
        let mut items_left = share_items;
        let mut skipped = 0_usize;
        for at in order {
            let Some(collection) = collections.get_mut(at) else {
                continue;
            };
            let copy = collection.shared_with_user;
            if requests >= limits.max_collections || bytes_left == 0 || !copy && items_left == 0 {
                skipped += 1;
                continue;
            }
            requests += 1;
            let max_body = bytes_left.min(limits.max_multistatus);
            let (answer, cost) = self
                .propfind_sized(&collection.href, Depth::Zero, &SHARING_PROPERTIES, max_body)
                .await;
            bytes_left = bytes_left.saturating_sub(cost);
            let (multistatus, final_url) = match answer {
                Ok(found) => found,
                Err(error) if error.is_oversized() => {
                    collection.clear_sharing(SharingData::TooLarge);
                    skipped += 1;
                    continue;
                }
                Err(DavError::NotFound) => {
                    skipped += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(response) = response_for(&multistatus, &collection.href, &final_url) else {
                skipped += 1;
                continue;
            };
            let p = &response.properties;
            collection.acl.clone_from(&p.acl);
            collection.invite.clone_from(&p.invite);
            collection.oc_invite.clone_from(&p.oc_invite);
            let items = collection.sharing_items();
            if !copy {
                if items > items_left {
                    // First fit: a smaller one later may still fit.
                    collection.clear_sharing(SharingData::NotFetched);
                    skipped += 1;
                    continue;
                }
                items_left -= items;
            }
            collection.sharing_data = SharingData::Fetched;
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                requests,
                "dav: sharing of some collections was not read (split listing budget)"
            );
        }
        Ok(())
    }

    /// The collections of a home's Depth 1 answer (module docs of
    /// [`Self::list_collections`]).
    async fn collections_of(
        &self,
        home: &Href,
        multistatus: nebula_dav_proto::Multistatus,
    ) -> Result<Vec<Collection>, DavError> {
        if multistatus.refused > 0 {
            tracing::warn!(
                refused = multistatus.refused,
                "dav: home listing had hrefs that do not resolve safely"
            );
        }
        let mut collections = Vec::new();
        let mut outside = 0_usize;
        for response in multistatus.responses {
            if response.href().same_location(home) {
                continue;
            }
            let in_home = response.href().is_member_of(home);
            if !in_home && !response.href().same_origin(home) {
                outside += 1;
                continue;
            }
            let href = response.href().as_collection();
            let p = response.properties;
            let Some(resource_type) = p.resource_type else {
                continue;
            };
            if resource_type.deleted_calendar || p.deleted_at.is_some() {
                continue;
            }
            let kind = if resource_type.calendar {
                CollectionKind::Calendar
            } else if resource_type.addressbook {
                CollectionKind::Addressbook
            } else {
                continue;
            };
            collections.push(Collection::from_properties(href, kind, p, in_home));
        }
        if outside > 0 {
            tracing::warn!(outside, "dav: hrefs on another origin were ignored");
        }
        if self.webcal_cache() {
            self.mark_subscriptions(home, &mut collections).await?;
        }
        Ok(collections)
    }

    /// With Nextcloud's webcal caching, subscriptions came as calendars:
    /// a second listing without it says which ones they are, and their
    /// source.
    async fn mark_subscriptions(
        &self,
        home: &Href,
        collections: &mut [Collection],
    ) -> Result<(), DavError> {
        let (multistatus, _final_url) = self
            .propfind_plain(
                home,
                Depth::One,
                &[Property::ResourceType, Property::Source],
            )
            .await?;
        for response in multistatus.responses {
            let subscribed = response
                .properties
                .resource_type
                .is_some_and(|kind| kind.subscribed);
            if !subscribed || !response.href().is_member_of(home) {
                continue;
            }
            let href = response.href().as_collection();
            if let Some(collection) = collections
                .iter_mut()
                .find(|c| c.kind == CollectionKind::Calendar && c.href.same_location(&href))
            {
                collection.subscription = Some(Subscription {
                    source: response.properties.source,
                });
            }
        }
        Ok(())
    }
}
