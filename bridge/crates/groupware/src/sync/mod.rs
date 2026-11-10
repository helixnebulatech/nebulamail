// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sync from CalDAV/CardDAV to the store
//! read-only:
//!
//! - **Discovery** ([`GroupwareAccount::discover`]): principal, homes,
//!   addresses, outbox and `calendar-auto-schedule`, stored in
//!   `dav_accounts`.
//! - **A round** ([`GroupwareAccount::sync_round`]): one PROPFIND Depth 1
//!   per home (every collection with its sync token and ctag), applied to
//!   the store; then only the collections whose token or ctag changed are
//!   synced ([`collection`]).

pub(crate) mod collection;

use std::collections::{HashMap, HashSet};

use nebula_dav::proto::{Href, MAX_ACES};
use nebula_dav::{Collection, CollectionKind, DavClient, DavError, ListOptions, SharingData};
use nebula_store::{
    CalendarExtras, Change, ChangeKind, CollectionKind as StoreKind, CollectionProps, DataType,
    DavAccount, DavHref, Recorded, UnixTime,
};

use crate::core::account::{Discovered, GroupwareAccount, NextcloudSupport};
use crate::model::calendar::{participant_addresses, participant_number};
use crate::model::index::{collection_props, stored_href, stored_kind};
use crate::sharing::{SharingSupport, collection_sharing, shares_bytes};
use crate::{GroupwareError, GroupwareEvent, StateChanged};

/// The homes on `client`'s own origin (the one that gets the login: its
/// configured origin, or the account's iCloud host).
fn on_own_origin(homes: Vec<Href>, client: &DavClient) -> Vec<Href> {
    let total = homes.len();
    let kept: Vec<Href> = homes
        .into_iter()
        .filter(|home| client.gets_login(home))
        .collect();
    if kept.len() < total {
        tracing::warn!(
            dropped = total - kept.len(),
            "dav: homes on another origin than their configured server were ignored"
        );
    }
    kept
}

impl GroupwareAccount {
    fn lock_split_homes(
        &self,
    ) -> std::sync::MutexGuard<'_, HashSet<nebula_dav::proto::LocationKey>> {
        self.split_homes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Discovery (RFC 6764, `current-user-principal`, homes), stored for
    /// the account.
    ///
    /// # Errors
    /// [`GroupwareError::Unauthorized`] for a refused login; others if the
    /// server can't be used.
    pub async fn discover(&self) -> Result<(), GroupwareError> {
        let found = self.client.discover().await?;
        let split = self
            .contacts_client
            .as_ref()
            .filter(|_| self.config().contacts);
        let (calendar_homes, addressbook_homes, contacts_principal, contacts_capabilities) =
            match split {
                None => (
                    found.calendar_homes.clone(),
                    found.addressbook_homes.clone(),
                    found.principal.clone(),
                    found.capabilities,
                ),
                // With `contacts_url`, calendars only come from `url` and
                // address books only from `contacts_url`, each on its own
                // server's origin: a home either server names elsewhere
                // (the other configured server, an allowed host) is not
                // used.
                Some(contacts) => {
                    let books = contacts.discover().await.inspect_err(|error| {
                        tracing::warn!(
                            account = %self.id,
                            %error,
                            "dav: discovery on the contacts server (contacts_url) failed"
                        );
                    })?;
                    (
                        on_own_origin(found.calendar_homes.clone(), &self.client),
                        on_own_origin(books.addressbook_homes, contacts),
                        books.principal,
                        books.capabilities,
                    )
                }
            };
        let now = UnixTime::now()?;
        let stored = DavAccount {
            principal: stored_href(&found.principal)?,
            calendar_homes: calendar_homes
                .iter()
                .map(stored_href)
                .collect::<Result<_, _>>()?,
            addressbook_homes: addressbook_homes
                .iter()
                .map(stored_href)
                .collect::<Result<_, _>>()?,
            schedule_outbox: found
                .schedule_outbox
                .as_ref()
                .map(stored_href)
                .transpose()?,
            user_addresses: found.calendar_user_addresses.clone(),
            auto_schedule: found.capabilities.calendar_auto_schedule,
            discovered_at: now,
        };
        let before = self
            .store()
            .dav_account(self.id)
            .await?
            .map(|previous| previous.user_addresses)
            .unwrap_or_default();
        self.store().put_dav_account(self.id, &stored).await?;
        self.record_identities(&before, &stored.user_addresses)
            .await?;
        // A new discovery tries the whole home listings again.
        self.split_homes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.set_discovered(Discovered {
            calendar_homes,
            addressbook_homes,
            auto_schedule: stored.auto_schedule,
            user_addresses: stored.user_addresses,
            principal: found.principal,
            principal_name: found.principal_name,
            principal_type: found.principal_type,
            principal_collections: found.principal_collections,
            schedule_outbox: found.schedule_outbox,
            sharing: SharingSupport {
                acl: found.capabilities.access_control,
                calendar_server: found.capabilities.calendarserver_sharing,
                resource_sharing: found.capabilities.resource_sharing,
                nextcloud: found.capabilities.oc_resource_sharing,
            },
            nextcloud: NextcloudSupport {
                publishing: found.capabilities.oc_calendar_publishing,
                subscriptions: found.capabilities.calendarserver_subscribed
                    && found.capabilities.nc_calendar_webcal_cache,
                trash_bin: found.capabilities.nc_calendar_trashbin,
            },
            contacts_principal,
            contacts_sharing: SharingSupport {
                acl: contacts_capabilities.access_control,
                calendar_server: contacts_capabilities.calendarserver_sharing,
                resource_sharing: contacts_capabilities.resource_sharing,
                nextcloud: contacts_capabilities.oc_resource_sharing,
            },
            at: now,
        });
        Ok(())
    }

    /// `ParticipantIdentity` changes (one identity per `mailto:` address of
    /// `calendar-user-address-set`, ids from [`participant_number`]):
    /// addresses that appeared are created, those that left destroyed.
    async fn record_identities(
        &self,
        before: &[String],
        after: &[String],
    ) -> Result<(), GroupwareError> {
        let before = participant_addresses(before.iter().map(String::as_str));
        let after = participant_addresses(after.iter().map(String::as_str));
        let has = |list: &[String], address: &str| {
            list.iter().any(|known| known.eq_ignore_ascii_case(address))
        };
        let created = after
            .iter()
            .filter(|address| !has(&before, address))
            .map(|address| (address, ChangeKind::Created));
        let destroyed = before
            .iter()
            .filter(|address| !has(&after, address))
            .map(|address| (address, ChangeKind::Destroyed));
        let changes: Vec<Change> = created
            .chain(destroyed)
            .map(|(address, kind)| Change {
                object: participant_number(address),
                kind,
            })
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        self.store()
            .record_changes(self.id, DataType::ParticipantIdentity, &changes)
            .await?;
        let _ = self
            .shared
            .events
            .send(GroupwareEvent::StateChanged(StateChanged {
                account: self.id,
                data_types: vec![DataType::ParticipantIdentity],
            }));
        Ok(())
    }

    /// The homes to list: those of the configured kinds, each once, with
    /// the kind of collection taken from each. Without `contacts_url`
    /// every kind is taken from every home (as the server lists them);
    /// with it, calendars only from the calendar homes (on `url`) and
    /// address books only from the address book homes (on
    /// `contacts_url`).
    fn homes(&self, found: &Discovered) -> Vec<(Href, Option<CollectionKind>)> {
        let config = self.config();
        let split = self.contacts_client.is_some();
        let calendars = found
            .calendar_homes
            .iter()
            .filter(|_| config.calendars)
            .map(|home| (home, split.then_some(CollectionKind::Calendar)));
        let books = found
            .addressbook_homes
            .iter()
            .filter(|_| config.contacts)
            .map(|home| (home, split.then_some(CollectionKind::Addressbook)));
        let mut homes: Vec<(Href, Option<CollectionKind>)> = Vec::new();
        for (home, only) in calendars.chain(books) {
            if !homes
                .iter()
                .any(|(done, kind)| *kind == only && done.same_location(home))
            {
                homes.push((home.clone(), only));
            }
        }
        homes
    }

    /// The collections of every home, each once (split listings
    /// remembered until the next discovery).
    async fn listed_collections(
        &self,
        found: &Discovered,
    ) -> Result<Vec<Collection>, GroupwareError> {
        let config = self.config();
        let mut listed: Vec<Collection> = Vec::new();
        let mut seen: HashSet<Href> = HashSet::new();
        for (home, only) in self.homes(found) {
            let client = match only {
                Some(CollectionKind::Addressbook) => self.client_for(StoreKind::AddressBook),
                _ => &self.client,
            };
            let key = home.location_key();
            let options = ListOptions {
                split: self.lock_split_homes().contains(&key),
                // Room for the account's budget and one whole ACL more.
                share_items: config.max_account_shares.saturating_add(MAX_ACES),
            };
            let home_listing = client.list_home(&home, options).await?;
            if home_listing.split {
                self.lock_split_homes().insert(key);
            }
            for collection in home_listing.collections {
                let wanted = match collection.kind {
                    CollectionKind::Calendar => config.calendars,
                    CollectionKind::Addressbook => config.contacts,
                } && only.is_none_or(|kind| kind == collection.kind);
                if wanted && seen.insert(collection.href.clone()) {
                    listed.push(collection);
                }
            }
        }
        Ok(listed)
    }

    /// One sync round: the collections of every home, then the resources
    /// of those that changed. With `reconcile`, every collection is
    /// compared whole (after discovery, once a day: catches deletions a
    /// server's sync report missed).
    ///
    /// # Errors
    /// [`GroupwareError::Unauthorized`] for a refused login (at once);
    /// otherwise the first error, after trying the other collections.
    pub async fn sync_round(&self, reconcile: bool) -> Result<Recorded, GroupwareError> {
        let _one_round = self.sync_lock.lock().await;
        let found = self.discovered().ok_or(GroupwareError::NotDiscovered)?;
        let config = self.config();
        // Boxed: a home's listing may be split, a big future.
        let listed = Box::pin(self.listed_collections(&found)).await?;
        // Whose each one is and who it is shared with
        // outside the homes, only someone else's.
        let known = self.known_principals();
        // The links stored now: an unchanged one keeps its sealed bytes;
        // and the owners, for a copy whose invitation was not read.
        let stored_now = self.store().dav_collections(self.id).await?;
        let mut previous: HashMap<DavHref, CalendarExtras> = HashMap::new();
        let mut owners: HashMap<DavHref, DavHref> = HashMap::new();
        for c in stored_now {
            if let Some(owner) = c.props.sharing.owner {
                owners.insert(c.props.href.clone(), owner);
            }
            previous.insert(c.props.href, c.props.extras);
        }
        let mut props = Vec::with_capacity(listed.len());
        let mut kept = Vec::with_capacity(listed.len());
        for collection in listed {
            let kind = stored_kind(collection.kind);
            let Some(mut sharing) = collection_sharing(
                &collection,
                found.principal_for(kind),
                found.sharing_for(kind),
                &known,
            )?
            else {
                continue;
            };
            let href = stored_href(&collection.href)?;
            if !copy_owner(&collection, &mut sharing, owners.get(&href)) {
                continue;
            }
            let mut stored = collection_props(&collection, &self.key, previous.get(&href))?;
            // Only the owner publishes, and only what it may share
            // (Nextcloud's own sharing: its calendar, `write-acl`).
            stored.extras.publishable = found.nextcloud.publishing
                && kind == StoreKind::Calendar
                && !stored.extras.is_subscription
                && sharing.owner.is_none()
                && sharing.mode == Some(nebula_store::ShareMode::Nextcloud);
            stored.sharing = sharing;
            props.push(stored);
            kept.push(collection);
        }
        let max = self.client.server().limits.max_collections;
        let stored: HashSet<DavHref> = self
            .store()
            .dav_collections(self.id)
            .await?
            .into_iter()
            .filter(|c| c.orphaned_at.is_none())
            .map(|c| c.props.href)
            .collect();
        let (mut props, listed) = if props.len() > max {
            keep_at_most(props, kept, &stored, max)?
        } else {
            (props, kept)
        };
        cap_account_shares(
            &mut props,
            &listed,
            &stored,
            ShareBudget {
                items: config.max_account_shares,
                bytes: config.max_account_share_bytes,
            },
        );
        let (mut recorded, live) = self
            .store()
            .apply_collections(self.id, props, UnixTime::now()?)
            .await?;
        self.emit(&recorded);

        let remotes: HashMap<DavHref, &Collection> = listed
            .iter()
            .filter_map(|c| Some((stored_href(&c.href).ok()?, c)))
            .collect();
        let mut first_error = None;
        for stored in live {
            let Some(remote) = remotes.get(&stored.props.href).copied() else {
                continue;
            };
            if !reconcile && !collection::needs_sync(&stored, remote) {
                continue;
            }
            match self.sync_collection(&stored, remote, reconcile).await {
                Ok(done) => recorded.merge(done),
                Err(GroupwareError::Unauthorized) => return Err(GroupwareError::Unauthorized),
                Err(error) => {
                    tracing::warn!(account = %self.id, %error, "syncing a collection failed");
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(recorded),
        }
    }
}

/// A Calendar Server copy names its owner only in its invitation: when
/// this round did not read it, the owner stored from the last one, else
/// `false`: the copy is left out, never taken for the user's own (its
/// free/busy and details would count as the user's).
fn copy_owner(
    listed: &Collection,
    sharing: &mut nebula_store::CollectionSharing,
    stored: Option<&DavHref>,
) -> bool {
    if !listed.shared_with_user
        || sharing.owner.is_some()
        || listed.sharing_data == SharingData::Fetched
    {
        return true;
    }
    if let Some(owner) = stored {
        sharing.owner = Some(owner.clone());
        true
    } else {
        tracing::warn!("dav: a shared copy whose owner was not read is left out");
        false
    }
}

/// Which collections are kept first: 0 for the account's own (no other
/// owner, and not shared with it: a shared one whose owner could not be
/// resolved, such as an `urn:x-uid:` organizer, is still someone
/// else's), 1 for those shared with it that it already has (`stored`, so
/// that nobody can push them out by sharing more), 2 for new shared
/// ones.
fn rank(props: &CollectionProps, listed: &Collection, stored: &HashSet<DavHref>) -> u8 {
    if props.sharing.owner.is_none() && !listed.shared_with_user {
        0
    } else if stored.contains(&props.href) {
        1
    } else {
        2
    }
}

/// The sharees an account keeps, over all its collections.
#[derive(Debug, Clone, Copy)]
struct ShareBudget {
    items: usize,
    bytes: usize,
}

/// Within the account's [`ShareBudget`], in the order of
/// [`keep_at_most`] (its own first; by href within each group, so the
/// same ones fit from one round to the next): each collection takes its
/// sharees from the budget, and one that does not fit is left without a
/// sharing mode and sharees (its owner stays), while the ones after it are
/// still tried (first fit). Never a failure.
fn cap_account_shares(
    props: &mut [CollectionProps],
    listed: &[Collection],
    stored: &HashSet<DavHref>,
    budget: ShareBudget,
) {
    let mut order: Vec<usize> = (0..props.len().min(listed.len())).collect();
    let key = |at: &usize| {
        props
            .get(*at)
            .zip(listed.get(*at))
            .map(|(p, l)| (rank(p, l, stored), p.href.clone()))
    };
    order.sort_by_cached_key(key);
    let (mut items, mut bytes) = (budget.items, budget.bytes);
    let mut left_out = 0_usize;
    for at in order {
        let Some(collection) = props.get_mut(at) else {
            continue;
        };
        let shares = &collection.sharing.shares;
        let (count, size) = (shares.len(), shares_bytes(shares));
        if count <= items && size <= bytes {
            items -= count;
            bytes -= size;
        } else {
            collection.sharing.mode = None;
            collection.sharing.shares = Vec::new();
            left_out += 1;
        }
    }
    if left_out > 0 {
        tracing::warn!(
            left_out,
            "sharing: collections past the account's sharee budget have no sharing mode"
        );
    }
}

/// At most `max` collections, in this order: the account's own, then
/// those shared with it that it already has (`stored`, so that nobody can
/// push them out by sharing more), then new shared ones; by href within
/// each, so that the same ones are kept from one round to the next. What
/// is left out is someone else's that does not fit: hidden as not listed,
/// with a warning. Sharing never fails the round; only the account's own
/// past the limit does.
fn keep_at_most(
    props: Vec<CollectionProps>,
    listed: Vec<Collection>,
    stored: &HashSet<DavHref>,
    max: usize,
) -> Result<(Vec<CollectionProps>, Vec<Collection>), GroupwareError> {
    let mut pairs: Vec<(CollectionProps, Collection)> = props.into_iter().zip(listed).collect();
    let rank = |(props, listed): &(CollectionProps, Collection)| rank(props, listed, stored);
    if pairs.iter().filter(|pair| rank(pair) == 0).count() > max {
        return Err(DavError::TooMany.into());
    }
    pairs.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.0.href.cmp(&b.0.href)));
    let left_out = pairs.len().saturating_sub(max);
    if left_out > 0 {
        tracing::warn!(
            left_out,
            "more collections are shared with the account than are kept"
        );
    }
    pairs.truncate(max);
    Ok(pairs.into_iter().unzip())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

    use nebula_dav::proto::Properties;
    use nebula_store::{CollectionSharing, Rights, ShareMode, Sharee};
    use url::Url;

    use super::*;

    fn base() -> Href {
        Href::from_url(&Url::parse("http://localhost:8080/").unwrap()).unwrap()
    }

    fn sharees(count: usize, long: usize) -> Vec<(Sharee, Rights)> {
        (0..count)
            .map(|n| {
                let text = format!("http://localhost:8080/dav/pal/u{n}{}/", "x".repeat(long));
                (Sharee::new(&text).unwrap(), Rights::READ)
            })
            .collect()
    }

    /// A calendar at `path`, someone else's with `owner`, with `count`
    /// sharees (names `long` bytes longer).
    fn pair(
        path: &str,
        owner: Option<&str>,
        count: usize,
        long: usize,
    ) -> (CollectionProps, Collection) {
        let at = Href::resolve(path, &base()).unwrap();
        let listed = Collection::from_properties(
            at.clone(),
            CollectionKind::Calendar,
            Properties::default(),
            true,
        );
        let props = CollectionProps {
            kind: StoreKind::Calendar,
            href: stored_href(&at).unwrap(),
            name: None,
            color: None,
            description: None,
            sort_order: None,
            time_zone: None,
            default_alarms_datetime: None,
            default_alarms_date: None,
            rights: None,
            components: None,
            vcard_versions: None,
            supports_sync: true,
            sharing: CollectionSharing {
                owner: owner.map(|o| DavHref::new(o).unwrap()),
                mode: Some(ShareMode::Acl),
                shares: sharees(count, long),
            },
            extras: CalendarExtras::default(),
        };
        (props, listed)
    }

    fn capped(
        pairs: Vec<(CollectionProps, Collection)>,
        budget: ShareBudget,
    ) -> Vec<(Option<ShareMode>, usize)> {
        let (mut props, listed): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        cap_account_shares(&mut props, &listed, &HashSet::new(), budget);
        props
            .iter()
            .map(|p| (p.sharing.mode, p.sharing.shares.len()))
            .collect()
    }

    /// The account's own first, then others' by href; one that does not
    /// fit is left without a mode (its owner stays) and the ones after it
    /// are still tried. The same every round.
    #[test]
    fn account_share_budget_keeps_own_first() {
        let bob = "http://localhost:8080/dav/pal/bob/";
        let pairs = || {
            vec![
                (pair("/dav/cal/bob/b/", Some(bob), 300, 0)),
                (pair("/dav/cal/bob/a/", Some(bob), 300, 0)),
                (pair("/dav/cal/alice/work/", None, 300, 0)),
                (pair("/dav/cal/bob/c/", Some(bob), 100, 0)),
            ]
        };
        let budget = ShareBudget {
            items: 700,
            bytes: usize::MAX,
        };
        let first = capped(pairs(), budget);
        assert_eq!(
            first,
            [
                (None, 0),
                (Some(ShareMode::Acl), 300),
                (Some(ShareMode::Acl), 300),
                (Some(ShareMode::Acl), 100),
            ]
        );
        assert_eq!(capped(pairs(), budget), first);
        // Owners stay.
        let (mut props, listed): (Vec<_>, Vec<_>) = pairs().into_iter().unzip();
        cap_account_shares(&mut props, &listed, &HashSet::new(), budget);
        assert!(props[0].sharing.owner.is_some());
    }

    /// Long names count by their bytes: 8 KB sharees fill 16 MiB long
    /// before 50 000 of them.
    #[test]
    fn account_share_budget_counts_bytes() {
        let budget = ShareBudget {
            items: 50_000,
            bytes: 16 * 1024 * 1024,
        };
        let long = 8000;
        let pairs = vec![
            pair("/dav/cal/alice/a/", None, 1500, long),
            pair("/dav/cal/alice/b/", None, 1500, long),
        ];
        assert_eq!(
            capped(pairs, budget),
            [(Some(ShareMode::Acl), 1500), (None, 0)]
        );
    }

    #[test]
    fn copies_without_their_invitation_keep_the_last_owner_or_are_left_out() {
        let at = Href::resolve("/dav.php/calendars/alice/copy/", &base()).unwrap();
        let mut listed =
            Collection::from_properties(at, CollectionKind::Calendar, Properties::default(), true);
        listed.shared_with_user = true;
        listed.sharing_data = SharingData::TooLarge;
        let carol = DavHref::new("http://localhost:8080/dav.php/principals/carol/").unwrap();
        let mut sharing = CollectionSharing::default();
        assert!(copy_owner(&listed, &mut sharing, Some(&carol)));
        assert_eq!(sharing.owner, Some(carol));
        let mut sharing = CollectionSharing::default();
        assert!(!copy_owner(&listed, &mut sharing, None));
        // Read this round: as the invitation says (Apple's `urn:x-uid:`
        // organizer stays unresolved, as before).
        listed.sharing_data = SharingData::Fetched;
        assert!(copy_owner(&listed, &mut CollectionSharing::default(), None));
    }
}
