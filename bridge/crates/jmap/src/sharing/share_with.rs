// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `shareWith` and `myRights.mayShare` of calendars and address books
//!over what the groupware layer found
//! ([`nebula_groupware::sharing`]).
//!
//! - **Get:** the sharees the server lists, by principal id (`r{n}`, the
//!   same as `Principal/get`); a sharee named by address is the principal
//!   the server shows with it, and one the bridge can't name is left out.
//!   `null` where the user can't share (no `principals` for the web).
//! - **Set:** `shareWith` whole or `shareWith/<id>`, for principals the
//!   server showed or sharees it lists (else `invalidProperties`; a listed
//!   sharee is changed and removed by the name it is listed with, and a
//!   removal the server ignores is `forbidden`); the JMAP rights become the
//!   store's [`Rights`] here and privileges or a Calendar Server access in
//!   the groupware layer ("free/busy only" needs WebDAV ACL: elsewhere
//!   it is `invalidProperties`). Refusals are `forbidden`, never with the
//!   server's text.
//! - **Create:** `shareWith` of a new calendar or address book is
//!   checked first ([`check_new`]: nothing is made for an unknown
//!   principal) and set right after it exists ([`share_new`]); if the
//!   server refuses, the new collection goes again (all or nothing).
//! - **Addresses without a principal** (sabre/dav, Apple): an id the
//!   bridge gave an address (`Principal/query` by `email`) is shared by
//!   address with `CS:share`; elsewhere it is unknown.
//! - In the shared account of someone else, the rights the user has
//!   (`myRights`) decide; re-sharing only with `write-acl` (Stalwart's
//!   Share right).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nebula_dav::proto::CalendarAddress;
use nebula_groupware::principals::Principal;
use nebula_groupware::sharing::{MAX_SHARE_CHANGE, ShareError, ShareUpdate, is_refusal};
use nebula_groupware::{GroupwareAccount, GroupwareError};
use nebula_jmap_proto::object::{AddressBookRights, CalendarRights, MapPatch};
use nebula_jmap_proto::{PrincipalId, SetError, SetErrorType};
use nebula_store::{Collection, MAX_SHARES, Rights, Sharee};

use crate::core::context::Ctx;
use crate::principals::ids::{principal_id, principal_number};

/// The store's rights of a calendar's JMAP rights. Any change of events
/// is `write` on the server, so the four "write" rights go together.
pub(crate) const fn calendar_bits(rights: CalendarRights) -> Rights {
    let mut bits = Rights::empty();
    if rights.may_read_items {
        bits = bits.union(Rights::READ);
    }
    if rights.may_read_free_busy || rights.may_read_items {
        bits = bits.union(Rights::READ_FREE_BUSY);
    }
    if rights.may_write_all || rights.may_write_own || rights.may_update_private || rights.may_rsvp
    {
        bits = bits
            .union(Rights::WRITE_CONTENT)
            .union(Rights::WRITE_PROPERTIES)
            .union(Rights::BIND)
            .union(Rights::UNBIND);
    }
    if rights.may_delete {
        bits = bits.union(Rights::UNBIND);
    }
    if rights.may_share {
        bits = bits.union(Rights::WRITE_ACL);
    }
    bits
}

/// The JMAP rights of a sharee or of the user on a calendar.
pub(crate) const fn calendar_rights(bits: Rights, may_share: bool) -> CalendarRights {
    let read = bits.contains(Rights::READ);
    let write = bits.contains(Rights::WRITE_CONTENT);
    CalendarRights {
        may_read_free_busy: read || bits.contains(Rights::READ_FREE_BUSY),
        may_read_items: read,
        may_write_all: write,
        may_write_own: write,
        may_update_private: write,
        may_rsvp: write,
        may_share,
        may_delete: bits.contains(Rights::UNBIND),
    }
}

/// The store's rights of an address book's JMAP rights (the web's levels:
/// read; read and write; read, write and delete).
pub(crate) const fn book_bits(rights: AddressBookRights) -> Rights {
    let mut bits = Rights::empty();
    if rights.may_read {
        bits = bits.union(Rights::READ);
    }
    if rights.may_write {
        bits = bits
            .union(Rights::WRITE_CONTENT)
            .union(Rights::WRITE_PROPERTIES)
            .union(Rights::BIND);
    }
    if rights.may_delete {
        bits = bits.union(Rights::UNBIND);
    }
    if rights.may_share {
        bits = bits.union(Rights::WRITE_ACL);
    }
    bits
}

/// The JMAP rights of a sharee or of the user on an address book.
pub(crate) const fn book_rights(bits: Rights, may_share: bool) -> AddressBookRights {
    AddressBookRights {
        may_read: bits.contains(Rights::READ),
        may_write: bits.contains(Rights::WRITE_CONTENT) || bits.contains(Rights::BIND),
        may_share,
        may_delete: bits.contains(Rights::UNBIND),
    }
}

/// The running DAV account of the call, if any.
fn running(ctx: &Ctx<'_>) -> Option<std::sync::Arc<GroupwareAccount>> {
    ctx.inner
        .groupware
        .as_ref()
        .and_then(|groupware| groupware.account(ctx.id()))
}

/// `shareWith` of a collection: `None` (JSON `null`) unless the user may
/// share it.
pub(crate) async fn share_with<R>(
    ctx: &Ctx<'_>,
    collection: &Collection,
    rights: impl Fn(Rights) -> R,
) -> Option<BTreeMap<PrincipalId, R>> {
    collection.props.sharing.mode?;
    let running = running(ctx)?;
    let shares = &collection.props.sharing.shares;
    // One look at the directory for all of them. Sharees named by address
    // need the server's principals: list them once if they are not known
    // yet (after a restart).
    let mut numbers = running.sharee_numbers(shares);
    if numbers.iter().any(Option::is_none) {
        match running.principals().await {
            Ok(_) => numbers = running.sharee_numbers(shares),
            Err(error) => tracing::debug!(%error, "sharing: principals unavailable"),
        }
    }
    let joined = joined_shares(
        shares
            .iter()
            .zip(numbers)
            .filter_map(|((_, bits), number)| {
                let id = number.and_then(|number| principal_id(number).ok())?;
                Some((id, *bits))
            }),
    );
    Some(
        joined
            .into_iter()
            .map(|(id, bits)| (id, rights(bits)))
            .collect(),
    )
}

/// The sharees per principal: one with several entries (by two of their
/// addresses) gets all their rights together, so the web shows the most
/// they have, never less.
fn joined_shares(
    shares: impl Iterator<Item = (PrincipalId, Rights)>,
) -> BTreeMap<PrincipalId, Rights> {
    let mut out: BTreeMap<PrincipalId, Rights> = BTreeMap::new();
    for (id, bits) in shares {
        out.entry(id)
            .and_modify(|known| *known = known.union(bits))
            .or_insert(bits);
    }
    out
}

/// Who a `shareWith` key names.
#[derive(Debug)]
enum Target {
    Principal(Principal),
    /// An address without a principal (servers that share by address).
    Address(CalendarAddress),
}

/// What a `shareWith` change needs from the directory.
pub(crate) trait Directory {
    /// A principal seen before, by number.
    fn known_principal(&self, number: i64) -> Option<Principal>;
    /// An address given this number before.
    fn known_address(&self, number: i64) -> Option<CalendarAddress>;
    /// The server's principals now (a listing).
    fn principals(&self) -> impl Future<Output = Result<Vec<Principal>, GroupwareError>> + Send;
}

impl Directory for GroupwareAccount {
    fn known_principal(&self, number: i64) -> Option<Principal> {
        Self::known_principal(self, number)
    }

    fn known_address(&self, number: i64) -> Option<CalendarAddress> {
        Self::known_address(self, number)
    }

    fn principals(&self) -> impl Future<Output = Result<Vec<Principal>, GroupwareError>> + Send {
        Self::principals(self)
    }
}

/// Who `shareWith` keys are, if the server showed them (a fresh listing
/// if they are not known yet, at most once per change) or the bridge
/// gave the address an id.
struct Resolver<'a, D> {
    directory: &'a D,
    listing: Option<HashMap<i64, Principal>>,
}

impl<'a, D: Directory> Resolver<'a, D> {
    const fn new(directory: &'a D) -> Self {
        Self {
            directory,
            listing: None,
        }
    }

    async fn target(&mut self, id: &PrincipalId) -> Option<Target> {
        let number = principal_number(id)?;
        if let Some(known) = self.directory.known_principal(number) {
            return Some(Target::Principal(known));
        }
        if let Some(address) = self.directory.known_address(number) {
            return Some(Target::Address(address));
        }
        if self.listing.is_none() {
            let mut listing = HashMap::new();
            match self.directory.principals().await {
                Ok(all) => {
                    for principal in all {
                        listing.entry(principal.number()).or_insert(principal);
                    }
                }
                Err(error) => tracing::debug!(%error, "sharing: principals unavailable"),
            }
            self.listing = Some(listing);
        }
        self.listing
            .as_ref()?
            .get(&number)
            .cloned()
            .map(Target::Principal)
    }
}

/// The text of the error for a change with too many sharees.
const TOO_MANY: &str = "too many sharees (at most 10000)";
const _: () = assert!(MAX_SHARES == 10_000);

/// A `shareWith` patch the bridge takes: at most [`MAX_SHARE_CHANGE`]
/// keys, and a whole map of at most [`MAX_SHARES`]. Checked before
/// anything else (the directory, the server).
fn check_patch_size(set: usize, remove: usize, replace: bool) -> Result<(), SetError> {
    if set.saturating_add(remove) > MAX_SHARE_CHANGE || replace && set > MAX_SHARES {
        return Err(invalid(TOO_MANY));
    }
    Ok(())
}

fn invalid(description: &str) -> SetError {
    SetError::invalid_properties(["shareWith"]).with_description(description)
}

/// A refused or failed share as a set error (no server text).
fn share_error(error: &GroupwareError) -> SetError {
    match error {
        GroupwareError::Share(ShareError::FreeBusyOnly) => {
            return invalid("this server can't share free/busy only");
        }
        GroupwareError::Share(ShareError::NotRemoved) => {
            tracing::warn!("sharing: the server kept a sharee it was asked to remove");
            return SetError::new(SetErrorType::Forbidden)
                .with_description("sharing could not be changed now");
        }
        GroupwareError::Share(ShareError::RightsNotSupported) => {
            return invalid("this server can't share with exactly these rights");
        }
        GroupwareError::Share(ShareError::TooMany) => return invalid(TOO_MANY),
        GroupwareError::Share(ShareError::Unfinished) => {
            return SetError::new(SetErrorType::Forbidden)
                .with_description("sharing was only partly changed; try again");
        }
        _ => {}
    }
    if is_refusal(error) {
        return SetError::new(SetErrorType::Forbidden)
            .with_description("the server does not let you share this");
    }
    tracing::warn!(%error, "sharing: changing a share failed");
    SetError::new(SetErrorType::Forbidden).with_description("sharing could not be changed now")
}

/// Applies a `shareWith` patch to a live collection (`bits` turns the JMAP
/// rights into the store's).
///
/// # Errors
/// `invalidProperties` for a principal the server did not show, or a
/// collection the user can't share; `forbidden` if the server refuses.
pub(crate) async fn update<R: Copy>(
    ctx: &Ctx<'_>,
    collection: &Collection,
    patch: &MapPatch<PrincipalId, R>,
    bits: impl Fn(R) -> Rights,
) -> Result<(), SetError> {
    let Some(running) = running(ctx) else {
        return Err(share_error(&GroupwareError::NotDiscovered));
    };
    if collection.props.sharing.mode.is_none() {
        // Unsharing what can't be shared is nothing to do.
        let nothing = match patch {
            MapPatch::Replace(map) => map.as_ref().is_none_or(BTreeMap::is_empty),
            MapPatch::Change { set, .. } => set.is_empty(),
            _ => false,
        };
        return if nothing {
            Ok(())
        } else {
            Err(invalid("this server does not let you share this"))
        };
    }
    let empty = BTreeMap::new();
    let none = BTreeSet::new();
    let (set, remove, replace) = match patch {
        MapPatch::Replace(map) => (map.as_ref().unwrap_or(&empty), &none, true),
        MapPatch::Change { set, remove } => (set, remove, false),
        _ => return Err(invalid("this change of shareWith is not supported")),
    };
    check_patch_size(set.len(), remove.len(), replace)?;
    // A key of `shareWith` is first the sharees listed with that id (as
    // `share_with` named them, whether the directory shows them or not),
    // so what the user sees is what is changed or removed: all of them,
    // when one person is listed by several names.
    let shares = &collection.props.sharing.shares;
    let mut listed: HashMap<i64, Vec<Sharee>> = HashMap::new();
    for ((sharee, _), number) in shares.iter().zip(running.sharee_numbers(shares)) {
        if let Some(number) = number {
            listed.entry(number).or_default().push(sharee.clone());
        }
    }
    let set: Vec<(&PrincipalId, Rights)> = set.iter().map(|(id, r)| (id, bits(*r))).collect();
    let update = build_update(
        &mut Resolver::new(running.as_ref()),
        &listed,
        &set,
        remove,
        replace,
    )
    .await?;
    running
        .share_collection(collection.id, &update)
        .await
        .map_err(|error| share_error(&error))
}

/// The groupware change of a `shareWith` patch (`set` with the store's
/// rights), naming each key as `listed` lists it, else as the directory
/// shows it.
///
/// # Errors
/// `invalidProperties` for an unknown principal or the user.
async fn build_update<D: Directory>(
    resolver: &mut Resolver<'_, D>,
    listed: &HashMap<i64, Vec<Sharee>>,
    set: &[(&PrincipalId, Rights)],
    remove: &BTreeSet<PrincipalId>,
    replace: bool,
) -> Result<ShareUpdate, SetError> {
    let mut update = ShareUpdate {
        replace,
        ..ShareUpdate::default()
    };
    let listed = |id: &PrincipalId| -> &[Sharee] {
        principal_number(id)
            .and_then(|number| listed.get(&number))
            .map_or(&[], Vec::as_slice)
    };
    for (id, rights) in set {
        let names = listed(id);
        if *rights == Rights::empty() && !names.is_empty() {
            update.remove_sharees.extend(names.iter().cloned());
            continue;
        }
        match resolver.target(id).await {
            Some(Target::Principal(principal)) if principal.own => {
                return Err(invalid(
                    "a calendar or address book is not shared with oneself",
                ));
            }
            Some(Target::Principal(principal)) => update.set.push((principal, *rights)),
            Some(Target::Address(address)) => update.set_addresses.push((address, *rights)),
            // Listed, not in the directory (a Nextcloud group): every name
            // it is listed with gets the same rights.
            None if !names.is_empty() => update
                .set_sharees
                .extend(names.iter().map(|sharee| (sharee.clone(), *rights))),
            None => return Err(invalid("unknown principal")),
        }
    }
    for id in remove {
        let names = listed(id);
        if !names.is_empty() {
            update.remove_sharees.extend(names.iter().cloned());
            continue;
        }
        // Not listed: someone the server shows (Stalwart's ACL may have
        // more than the listing); an id it does not show any more has
        // nothing to remove.
        match resolver.target(id).await {
            Some(Target::Principal(principal)) => update.remove.push(principal),
            Some(Target::Address(address)) => update.remove_addresses.push(address),
            None => {}
        }
    }
    Ok(update)
}

/// Checks the `shareWith` of a collection being created, before anything
/// is made: every key someone the server showed (or an address with an
/// id), never the user.
///
/// # Errors
/// `invalidProperties`, as [`update`].
pub(crate) async fn check_new<R>(
    ctx: &Ctx<'_>,
    share_with: &BTreeMap<PrincipalId, R>,
) -> Result<(), SetError> {
    if share_with.is_empty() {
        return Ok(());
    }
    check_patch_size(share_with.len(), 0, true)?;
    let Some(running) = running(ctx) else {
        return Err(share_error(&GroupwareError::NotDiscovered));
    };
    let mut resolver = Resolver::new(running.as_ref());
    for id in share_with.keys() {
        match resolver.target(id).await {
            None => return Err(invalid("unknown principal")),
            Some(Target::Principal(principal)) if principal.own => {
                return Err(invalid(
                    "a calendar or address book is not shared with oneself",
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Shares a collection just created (`shareWith` of a create): as an
/// update that sets the whole map, on the collection as the store has it
/// now (its sharing mode comes from the listing after the create).
///
/// # Errors
/// As [`update`]; `invalidProperties` where the server does not let the
/// user share it.
pub(crate) async fn share_new<R: Copy>(
    ctx: &Ctx<'_>,
    id: nebula_store::CollectionId,
    share_with: &BTreeMap<PrincipalId, R>,
    bits: impl Fn(R) -> Rights,
) -> Result<(), SetError> {
    if share_with.is_empty() {
        return Ok(());
    }
    let collection = ctx
        .store()
        .dav_collections(ctx.id())
        .await
        .ok()
        .and_then(|all| {
            all.into_iter()
                .find(|c| c.id == id && c.orphaned_at.is_none())
        })
        .ok_or_else(SetError::not_found)?;
    update(
        ctx,
        &collection,
        &MapPatch::Replace(Some(share_with.clone())),
        bits,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's address book levels (contacts/sharing.ts) survive the
    /// round trip through the store's rights.
    #[test]
    fn book_levels_round_trip() {
        for (read, write, delete) in [
            (true, false, false),
            (true, true, false),
            (true, true, true),
        ] {
            let rights = AddressBookRights {
                may_read: read,
                may_write: write,
                may_share: false,
                may_delete: delete,
            };
            assert_eq!(book_rights(book_bits(rights), false), rights);
        }
    }

    /// Two entries of one person (two addresses): the most they have.
    #[test]
    fn entries_of_one_person_are_joined() {
        let id = principal_id(7).unwrap();
        let other = principal_id(8).unwrap();
        let write = Rights::READ.union(Rights::WRITE_CONTENT);
        let joined = joined_shares(
            [
                (id.clone(), write),
                (other.clone(), Rights::READ),
                (id.clone(), Rights::READ),
            ]
            .into_iter(),
        );
        assert_eq!(joined.get(&id), Some(&write));
        assert_eq!(joined.get(&other), Some(&Rights::READ));
    }

    #[test]
    fn calendar_rights_go_together() {
        let read = CalendarRights {
            may_read_free_busy: true,
            may_read_items: true,
            may_write_all: false,
            may_write_own: false,
            may_update_private: false,
            may_rsvp: false,
            may_share: false,
            may_delete: false,
        };
        assert_eq!(calendar_rights(calendar_bits(read), false), read);
        let write = CalendarRights {
            may_write_all: true,
            ..read
        };
        let back = calendar_rights(calendar_bits(write), false);
        assert!(back.may_write_own && back.may_rsvp && back.may_delete);
        let busy = CalendarRights {
            may_read_items: false,
            ..read
        };
        let back = calendar_rights(calendar_bits(busy), false);
        assert!(back.may_read_free_busy && !back.may_read_items);
        let share = CalendarRights {
            may_share: true,
            ..read
        };
        assert!(calendar_bits(share).contains(Rights::WRITE_ACL));
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    use nebula_dav::proto::Href;
    use url::Url;

    /// A directory that lists `listing` and counts how often.
    struct FakeDirectory {
        listing: Vec<Principal>,
        listed: AtomicUsize,
    }

    impl Directory for FakeDirectory {
        fn known_principal(&self, _number: i64) -> Option<Principal> {
            None
        }

        fn known_address(&self, _number: i64) -> Option<CalendarAddress> {
            None
        }

        fn principals(
            &self,
        ) -> impl Future<Output = Result<Vec<Principal>, GroupwareError>> + Send {
            self.listed.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(self.listing.clone()))
        }
    }

    fn bob() -> Principal {
        let href =
            Href::from_url(&Url::parse("http://localhost:8080/dav/pal/bob/").unwrap()).unwrap();
        let mut principal = Principal::new(href);
        principal.addresses = vec![CalendarAddress::parse("mailto:bob@example.com").unwrap()];
        principal
    }

    #[test]
    fn too_many_sharees_is_invalid_properties() {
        let error = share_error(&GroupwareError::Share(ShareError::TooMany));
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        assert!(
            error
                .description()
                .unwrap_or_default()
                .contains("too many sharees")
        );
        assert!(check_patch_size(MAX_SHARE_CHANGE, 1, false).is_err());
        assert!(check_patch_size(MAX_SHARE_CHANGE - 1, 1, false).is_ok());
        assert!(check_patch_size(MAX_SHARES + 1, 0, true).is_err());
        assert!(check_patch_size(MAX_SHARES, 0, true).is_ok());
        let unfinished = share_error(&GroupwareError::Share(ShareError::Unfinished));
        assert!(
            unfinished
                .description()
                .unwrap_or_default()
                .contains("try again")
        );
    }

    /// 1000 ids the directory does not know: one listing, not 1000.
    #[tokio::test]
    async fn unknown_ids_list_the_directory_once() {
        let directory = FakeDirectory {
            listing: vec![bob()],
            listed: AtomicUsize::new(0),
        };
        let mut resolver = Resolver::new(&directory);
        let ids: Vec<PrincipalId> = (1..=1000).map(|n| principal_id(n).unwrap()).collect();
        let set: Vec<(&PrincipalId, Rights)> = ids.iter().map(|id| (id, Rights::READ)).collect();
        let refused = build_update(
            &mut resolver,
            &HashMap::new(),
            &set,
            &BTreeSet::new(),
            false,
        )
        .await;
        assert!(refused.is_err());
        let found = resolver
            .target(&principal_id(bob().number()).unwrap())
            .await;
        assert!(matches!(found, Some(Target::Principal(_))));
        assert_eq!(directory.listed.load(Ordering::SeqCst), 1);
    }

    /// One person listed by two names (an address and a location) under
    /// one id: removing the id removes both; changing it changes both.
    #[tokio::test]
    async fn every_name_of_an_id_is_changed() {
        let directory = FakeDirectory {
            listing: Vec::new(),
            listed: AtomicUsize::new(0),
        };
        let number = 42;
        let id = principal_id(number).unwrap();
        let names = vec![
            Sharee::new("mailto:bob@example.com").unwrap(),
            Sharee::new("http://localhost:8080/dav/pal/bob/").unwrap(),
        ];
        let listed = HashMap::from([(number, names.clone())]);
        let remove = BTreeSet::from([id.clone()]);
        let update = build_update(&mut Resolver::new(&directory), &listed, &[], &remove, false)
            .await
            .unwrap();
        assert_eq!(update.remove_sharees, names);
        let update = build_update(
            &mut Resolver::new(&directory),
            &listed,
            &[(&id, Rights::empty())],
            &BTreeSet::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(update.remove_sharees, names);
        let update = build_update(
            &mut Resolver::new(&directory),
            &listed,
            &[(&id, Rights::READ)],
            &BTreeSet::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            update.set_sharees,
            names
                .iter()
                .map(|n| (n.clone(), Rights::READ))
                .collect::<Vec<_>>()
        );
    }
}
