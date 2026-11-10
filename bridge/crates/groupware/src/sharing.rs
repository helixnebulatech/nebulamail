// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharing calendars and address books.
//!
//! **What the listing says** ([`collection_sharing`], every sync round):
//!
//! - **Whose it is.** Someone else's when its `DAV:owner` is another
//!   principal (Stalwart lists the collections others share in their
//!   owners' homes, and in the user's home with "assisted discovery"), or
//!   when it is a Calendar Server copy (`CS:shared` in `resourcetype`:
//!   sabre/dav and Apple put it in the user's home with the *user* as
//!   owner; the real owner is the organizer of `CS:invite`, or sabre's
//!   user without an access). A collection outside the user's homes is
//!   kept only if it is someone else's.
//! - **How the user may share it** ([`ShareMode`]): Nextcloud sharing
//!   for the user's own calendar or address book on a server that
//!   announces `oc-resource-sharing`, when the user has `write-acl` and
//!   the listing shows `O:invite` (Nextcloud also announces
//!   `calendarserver-sharing`, but refuses a `CS:share` with 501); else
//!   Calendar Server sharing for a calendar of a server that announces
//!   `calendarserver-sharing`, the user's own (not a copy) and shareable
//!   (`CS:can-be-shared` or the `DAV:share` privilege); else WebDAV ACL on
//!   a server that announces `access-control` and no sharing API of its
//!   own, when the user has `write-acl` and the ACL can be written back
//!   exactly. Radicale announces neither: nothing is shared there.
//! - **With whom**: the plain grants to one principal of the ACL (not the
//!   user, not the owner), or the sharees of `CS:invite` or `O:invite`
//!   (accepted or not answered yet), with their [`Rights`]. Nextcloud's
//!   sharees are principal paths below its DAV root (the part of the
//!   user's principal location before `principals/`), stored as the
//!   principal's URL like ACL sharees.
//!
//! **How many**: up to [`MAX_SHARES`] (10 000) sharees per
//! collection and `max_account_shares` (50 000) and
//! `max_account_share_bytes` (16 MiB of names) per account, its own
//! collections first. Never truncated: a collection past them is left
//! without a mode (a replacement computed from a cut list would remove
//! whoever was left out), and a change that would go past them is
//! [`ShareError::TooMany`] before anything is sent. Comparisons go by
//! normalized keys (the `keys` module), linear however many.
//!
//! **Changes** ([`GroupwareAccount::share_collection`]): with ACL, the ACL
//! is read again right before, the plain grants of the principals that
//! change are replaced and everything else is written back as it was (one
//! `ACL` request: it replaces the list whole); with Calendar Server
//! sharing, `CS:share` POSTs; with Nextcloud, `O:share` POSTs; of at most
//! 256 entries each, every name once, removals first (the `batches` module: a
//! failure half way never leaves anyone with more than before or than
//! asked; the store is read again and the error returned). Nextcloud
//! answers 200 also when it ignores a
//! sharee, a principal that is not among the sharees after the sync round
//! is [`ShareError::Ignored`]. Then a sync round, so the store (and push)
//! shows what the server did, and a check: nobody the change took away is
//! still listed and, with Calendar Server and Nextcloud sharing, nobody
//! has more than asked (else [`ShareError::Ignored`]). Changes of one
//! account go one at a time (they read the sharees, or the ACL, and write
//! them back); the ACL is written without `If-Match` (the servers tried
//! give collections no etag), so a change by another client in between is
//! only caught by that check.
//!
//! JMAP rights become privileges as Stalwart reads them (its
//! `dav/src/common/acl.rs`, 0.16.27): reading is `read` (and
//! `read-free-busy` for calendars), changing a calendar's events is
//! `write` (Stalwart needs it to remove events), an address book's is
//! `write-content` + `write-properties` + `bind` (and `write` with
//! deleting), sharing is `write-acl`. Calendar Server and Nextcloud
//! sharing only have `read` and `read-write` (with deleting): rights they
//! can't give exactly without giving more are
//! [`ShareError::RightsNotSupported`], and `write-acl` is left out.

mod batches;
pub(crate) mod keys;

use std::collections::{HashMap, HashSet};

use nebula_dav::proto::{
    Ace, Acl, CalendarAddress, Href, Invite, InviteStatus, LocationKey, MAX_ACES,
    MAX_OC_SHARE_REQUEST, MAX_SHARE_REQUEST, OcInvite, OcPrincipal, Privilege, PrivilegeSet,
    ShareAccess, Sharee as DavSharee,
};
use nebula_dav::{Collection as Listed, CollectionKind as ListedKind, DavError, SharingData};
use nebula_store::{
    Collection, CollectionId, CollectionKind, CollectionSharing, DavHref, MAX_SHARES, Rights,
    ShareMode, Sharee,
};
use tokio::time::Instant;

use self::batches::{merge_names, share_batches};
use self::keys::{
    ShareeKey, UpdateIndex, address_key, dav_key, location_key, principal_keys, principal_rights,
    rights_by_key, sharee_key,
};
use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::index::stored_href;
use crate::principals::{Principal, principal_number};
use crate::write::href;

// A collection's sharees are stored as they are read: as many as an
// invitation is read with.
const _: () = assert!(MAX_SHARES == nebula_dav::proto::MAX_SHAREES);
const _: () = assert!(MAX_ACES >= MAX_SHARES);

/// Most entries a change may name (`set`, `remove` and the rest
/// together): a whole `shareWith` of [`MAX_SHARES`] that replaces as many.
pub const MAX_SHARE_CHANGE: usize = 2 * MAX_SHARES;

/// What the server announced that sharing depends on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent tokens of the server's DAV header"
)]
pub(crate) struct SharingSupport {
    /// `access-control` (RFC 3744).
    pub(crate) acl: bool,
    /// `calendarserver-sharing`.
    pub(crate) calendar_server: bool,
    /// `resource-sharing` (a server with its own sharing API: its ACL is
    /// not for sharing).
    pub(crate) resource_sharing: bool,
    /// `oc-resource-sharing` (Nextcloud's `O:share`; its ACL is not for
    /// sharing either).
    pub(crate) nextcloud: bool,
}

/// The rights of a privilege set (aggregates expanded). `write-content`
/// counts as `bind` too: Stalwart lets a sharee with `write-content`
/// create resources but does not list `bind` (seen live, 0.16.27).
pub(crate) fn rights_of(set: PrivilegeSet) -> Rights {
    let mut rights = Rights::empty();
    for (has, flag) in [
        (set.can_read(), Rights::READ),
        (set.allows(Privilege::ReadFreeBusy), Rights::READ_FREE_BUSY),
        (set.can_write_content(), Rights::WRITE_CONTENT),
        (
            set.allows(Privilege::WriteProperties),
            Rights::WRITE_PROPERTIES,
        ),
        (set.can_create() || set.can_write_content(), Rights::BIND),
        (set.can_delete(), Rights::UNBIND),
        (set.allows(Privilege::WriteAcl), Rights::WRITE_ACL),
    ] {
        if has {
            rights = rights.union(flag);
        }
    }
    rights
}

/// Whether these rights give any access (else they take it away).
pub(crate) const fn grants_access(rights: Rights) -> bool {
    rights.contains(Rights::READ)
        || rights.contains(Rights::READ_FREE_BUSY)
        || rights.contains(Rights::WRITE_CONTENT)
        || rights.contains(Rights::WRITE_PROPERTIES)
        || rights.contains(Rights::BIND)
        || rights.contains(Rights::UNBIND)
        || rights.contains(Rights::WRITE_ACL)
}

/// The rights of a Calendar Server access.
pub(crate) const fn access_rights(access: ShareAccess) -> Rights {
    let read = Rights::READ.union(Rights::READ_FREE_BUSY);
    match access {
        ShareAccess::Read => read,
        ShareAccess::ReadWrite => read
            .union(Rights::WRITE_CONTENT)
            .union(Rights::WRITE_PROPERTIES)
            .union(Rights::BIND)
            .union(Rights::UNBIND),
    }
}

/// The Calendar Server (and Nextcloud) access for these rights (`None`:
/// no access). Those only have "read" and "read and write" (which
/// includes deleting): rights they can't give exactly
/// without giving more (free/busy only, deleting or writing properties
/// without writing, anything without reading) are refused, never rounded
/// up. Sharing (`WRITE_ACL`) is left out: less than asked, not more.
///
/// # Errors
/// [`ShareError::RightsNotSupported`].
pub(crate) const fn rights_access(rights: Rights) -> Result<Option<ShareAccess>, ShareError> {
    if !grants_access(rights) {
        return Ok(None);
    }
    if !rights.contains(Rights::READ) {
        return Err(ShareError::RightsNotSupported);
    }
    if rights.contains(Rights::WRITE_CONTENT) || rights.contains(Rights::BIND) {
        return Ok(Some(ShareAccess::ReadWrite));
    }
    if rights.contains(Rights::UNBIND) || rights.contains(Rights::WRITE_PROPERTIES) {
        return Err(ShareError::RightsNotSupported);
    }
    Ok(Some(ShareAccess::Read))
}

/// Free/busy only, without reading the events: only WebDAV ACL has it
/// (`read-free-busy`). Nextcloud and Calendar Server sharing only know
/// read or read-write, so it can't be given there.
const fn free_busy_only(rights: Rights) -> bool {
    rights.contains(Rights::READ_FREE_BUSY)
        && !rights.contains(Rights::READ)
        && !rights.contains(Rights::WRITE_CONTENT)
        && !rights.contains(Rights::BIND)
        && !rights.contains(Rights::UNBIND)
        && !rights.contains(Rights::WRITE_PROPERTIES)
}

/// The privileges an ACE grants for these rights (empty: no access).
pub(crate) fn rights_privileges(kind: CollectionKind, rights: Rights) -> Vec<Privilege> {
    let mut out = Vec::new();
    if rights.contains(Rights::READ) {
        out.push(Privilege::Read);
    }
    match kind {
        CollectionKind::Calendar => {
            if rights.contains(Rights::READ_FREE_BUSY) {
                out.push(Privilege::ReadFreeBusy);
            }
            if rights.contains(Rights::WRITE_CONTENT) {
                out.push(Privilege::Write);
            } else if rights.contains(Rights::UNBIND) {
                out.push(Privilege::Unbind);
            }
        }
        CollectionKind::AddressBook => {
            let write = rights.contains(Rights::WRITE_CONTENT) || rights.contains(Rights::BIND);
            match (write, rights.contains(Rights::UNBIND)) {
                (true, true) => out.push(Privilege::Write),
                (true, false) => out.extend([
                    Privilege::WriteContent,
                    Privilege::WriteProperties,
                    Privilege::Bind,
                ]),
                (false, true) => out.push(Privilege::Unbind),
                (false, false) => {}
            }
        }
    }
    if rights.contains(Rights::WRITE_ACL) {
        out.push(Privilege::WriteAcl);
    }
    out
}

fn sharee_of(sharee: &DavSharee) -> Option<Sharee> {
    match sharee {
        DavSharee::Address(address) => Sharee::new(address.as_str()).ok(),
        DavSharee::Principal(href) => Sharee::new(href.as_collection().url().as_str()).ok(),
    }
}

/// The owner of a Calendar Server copy, as a principal location.
fn invite_owner(invite: &Invite, known: &[Principal]) -> Option<Href> {
    match invite.owner()? {
        DavSharee::Principal(href) => Some(href.as_collection()),
        DavSharee::Address(address) => known
            .iter()
            .find(|p| p.has_address(address))
            .map(|p| p.href.as_collection()),
    }
}

fn is_own(principal: &Href, own: &Href) -> bool {
    principal.same_location(own)
}

/// Plain grants to one principal (not `own` nor `owner`), joined per
/// principal (by location: linear), or `None` with more than
/// [`MAX_SHARES`] principals with access (never cut: a replacement
/// computed from a cut list would remove whoever was left out).
fn acl_shares(acl: &Acl, own: &Href, owner: Option<&Href>) -> Option<Vec<(Sharee, Rights)>> {
    let mut out: Vec<(Href, PrivilegeSet)> = Vec::new();
    let mut at: HashMap<LocationKey, usize> = HashMap::new();
    for ace in &acl.aces {
        let Some(principal) = ace.granted_href() else {
            continue;
        };
        if is_own(principal, own) || owner.is_some_and(|o| is_own(principal, o)) {
            continue;
        }
        let privileges: Vec<Privilege> = ace
            .privileges
            .iter()
            .filter_map(nebula_dav::proto::QualifiedName::privilege)
            .collect();
        if let Some((_, set)) = at
            .get(&principal.location_key())
            .and_then(|i| out.get_mut(*i))
        {
            let mut all = privileges;
            all.extend(known_privileges(*set));
            *set = PrivilegeSet::new(&all);
        } else {
            at.insert(principal.location_key(), out.len());
            out.push((principal.as_collection(), PrivilegeSet::new(&privileges)));
        }
    }
    let shares: Vec<(Sharee, Rights)> = out
        .into_iter()
        .filter_map(|(principal, set)| {
            let rights = rights_of(set);
            let sharee = Sharee::new(principal.url().as_str()).ok()?;
            (rights != Rights::empty()).then_some((sharee, rights))
        })
        .collect();
    (shares.len() <= MAX_SHARES).then_some(shares)
}

/// The privileges of a set, listed (for joining two ACEs).
fn known_privileges(set: PrivilegeSet) -> Vec<Privilege> {
    [
        Privilege::Read,
        Privilege::Write,
        Privilege::WriteProperties,
        Privilege::WriteContent,
        Privilege::Bind,
        Privilege::Unbind,
        Privilege::ReadAcl,
        Privilege::WriteAcl,
        Privilege::ReadCurrentUserPrivilegeSet,
        Privilege::All,
        Privilege::ReadFreeBusy,
        Privilege::ScheduleSend,
        Privilege::ScheduleDeliver,
        Privilege::Share,
    ]
    .into_iter()
    .filter(|p| set.allows(*p))
    .collect()
}

/// The sharees with access of a `CS:invite` (not the user), or `None` if
/// it does not list all of them as the bridge can store them (a user it
/// could not read, a sharee it cannot store, more than [`MAX_SHARES`]): a
/// replacement computed from it would leave someone with access.
fn invite_shares(invite: &Invite, own: &Href) -> Option<Vec<(Sharee, Rights)>> {
    if !invite.is_complete() {
        return None;
    }
    let mut shares = Vec::new();
    for (sharee, access, status) in invite.sharees() {
        if !matches!(status, InviteStatus::Accepted | InviteStatus::NoResponse)
            || matches!(sharee, DavSharee::Principal(p) if is_own(p, own))
        {
            continue;
        }
        shares.push((sharee_of(sharee)?, access_rights(access)));
    }
    (shares.len() <= MAX_SHARES).then_some(shares)
}

/// The sharees with access of a Nextcloud `O:invite`, as principal URLs
/// below the DAV `root` (not the user), or `None` if it does not list all
/// of them as the bridge can store them (as [`invite_shares`]).
fn oc_shares(invite: &OcInvite, own: &Href, root: &Href) -> Option<Vec<(Sharee, Rights)>> {
    if !invite.is_complete() {
        return None;
    }
    let mut shares = Vec::new();
    for share in &invite.users {
        if !matches!(
            share.status,
            InviteStatus::Accepted | InviteStatus::NoResponse
        ) {
            continue;
        }
        let principal = share.principal.href(root)?;
        if is_own(&principal, own) {
            continue;
        }
        shares.push((
            Sharee::new(principal.url().as_str()).ok()?,
            access_rights(share.access),
        ));
    }
    (shares.len() <= MAX_SHARES).then_some(shares)
}

/// Whose a listed collection is and how it is shared, or `None` when it
/// must be left out (outside the user's homes and not someone else's).
pub(crate) fn collection_sharing(
    listed: &Listed,
    own: &Href,
    support: SharingSupport,
    known: &[Principal],
) -> Result<Option<CollectionSharing>, GroupwareError> {
    let owner = if listed.shared_with_user {
        listed
            .invite
            .as_ref()
            .and_then(|invite| invite_owner(invite, known))
    } else {
        listed
            .owner
            .as_ref()
            .filter(|owner| !is_own(owner, own))
            .map(Href::as_collection)
    };
    if !listed.in_home && owner.is_none() {
        return Ok(None);
    }
    let privileges = listed.privileges.unwrap_or_default();
    let nextcloud_root = OcPrincipal::dav_root(own).filter(|_| {
        support.nextcloud
            && owner.is_none()
            && !listed.shared_with_user
            && listed.oc_invite.is_some()
            && privileges.allows(Privilege::WriteAcl)
    });
    let calendar_server = nextcloud_root.is_none()
        && !support.nextcloud
        && support.calendar_server
        && listed.kind == ListedKind::Calendar
        && !listed.shared_with_user
        && (listed.sharing_modes.is_some_and(|m| m.can_be_shared)
            || privileges.allows(Privilege::Share));
    let acl = support.acl
        && !support.calendar_server
        && !support.resource_sharing
        && !support.nextcloud
        && privileges.allows(Privilege::WriteAcl)
        && listed
            .acl
            .as_ref()
            .is_some_and(|acl| acl.is_rewritable(&listed.href));
    // An invitation that does not list everyone with access leaves the
    // collection without a mode, as an ACL that cannot be written back: a
    // replacement would not remove who is missing. One never given (never
    // shared) lists nobody, rightly; one the listing did not read (a split
    // listing out of budget, an answer too big) says nothing, so no mode
    // either, whatever the way of sharing.
    let (mode, shares) = if listed.sharing_data != SharingData::Fetched {
        (None, Vec::new())
    } else if let Some(root) = &nextcloud_root {
        match listed
            .oc_invite
            .as_ref()
            .map(|invite| oc_shares(invite, own, root))
        {
            Some(None) => (None, Vec::new()),
            shares => (
                Some(ShareMode::Nextcloud),
                shares.flatten().unwrap_or_default(),
            ),
        }
    } else if calendar_server {
        match listed
            .invite
            .as_ref()
            .map(|invite| invite_shares(invite, own))
        {
            Some(None) => (None, Vec::new()),
            shares => (
                Some(ShareMode::CalendarServer),
                shares.flatten().unwrap_or_default(),
            ),
        }
    } else if acl {
        match listed
            .acl
            .as_ref()
            .map(|acl| acl_shares(acl, own, owner.as_ref()))
        {
            Some(None) => (None, Vec::new()),
            shares => (Some(ShareMode::Acl), shares.flatten().unwrap_or_default()),
        }
    } else {
        (None, Vec::new())
    };
    Ok(Some(CollectionSharing {
        owner: owner.as_ref().map(stored_href).transpose()?,
        mode,
        shares,
    }))
}

/// A change of who a collection is shared with.
#[derive(Debug, Clone, Default)]
pub struct ShareUpdate {
    /// Every current sharee not in `set` is removed (JMAP `shareWith`
    /// set whole).
    pub replace: bool,
    /// Principals to share with (or change), with their rights; empty
    /// rights remove them.
    pub set: Vec<(Principal, Rights)>,
    /// Principals to stop sharing with.
    pub remove: Vec<Principal>,
    /// Addresses the server shows no principal for, to share with (or
    /// change): only Calendar Server sharing names people by address
    /// (sabre/dav, Apple); empty rights remove them.
    pub set_addresses: Vec<(CalendarAddress, Rights)>,
    /// Addresses to stop sharing with.
    pub remove_addresses: Vec<CalendarAddress>,
    /// Sharees as the collection lists them, to change (empty rights
    /// remove them): someone `shareWith` shows that the directory does
    /// not (a Nextcloud group, a sharee outside the listing).
    pub set_sharees: Vec<(Sharee, Rights)>,
    /// Sharees as the collection lists them, to stop sharing with.
    pub remove_sharees: Vec<Sharee>,
}

impl ShareUpdate {
    /// Whether some sharee would get free/busy only.
    fn grants_free_busy_only(&self) -> bool {
        self.set
            .iter()
            .map(|(_, rights)| *rights)
            .chain(self.set_addresses.iter().map(|(_, rights)| *rights))
            .chain(self.set_sharees.iter().map(|(_, rights)| *rights))
            .any(free_busy_only)
    }

    /// How many entries it names.
    fn entries(&self) -> usize {
        self.set.len()
            + self.remove.len()
            + self.set_addresses.len()
            + self.remove_addresses.len()
            + self.set_sharees.len()
            + self.remove_sharees.len()
    }

    /// Whether every address given access is among the sharees listed.
    fn addresses_shared(&self, after: &HashMap<ShareeKey, Rights>) -> bool {
        self.set_addresses
            .iter()
            .filter(|(_, rights)| grants_access(*rights))
            .all(|(address, _)| after.contains_key(&address_key(address)))
    }

    /// Who it gives access to, for counting: each one's keys (the first
    /// is the name of a new entry) and the length of that name.
    fn grants(&self, principal_name: impl Fn(&Principal) -> usize) -> Vec<(Vec<ShareeKey>, usize)> {
        let mut out = Vec::new();
        for (principal, rights) in &self.set {
            if grants_access(*rights) {
                out.push((
                    principal_keys(principal).collect(),
                    principal_name(principal),
                ));
            }
        }
        for (address, rights) in &self.set_addresses {
            if grants_access(*rights) {
                out.push((vec![address_key(address)], address.as_str().len()));
            }
        }
        for (sharee, rights) in &self.set_sharees {
            if grants_access(*rights) {
                out.push((vec![sharee_key(sharee)], sharee.as_str().len()));
            }
        }
        out
    }
}

/// The sharees after a change, counted before it is sent: the current
/// ones it does not take away, and those it gives access to that are not
/// among them. How many, and the length of their names.
fn projected(
    current: &[(Sharee, Rights)],
    index: &UpdateIndex,
    grants: Vec<(Vec<ShareeKey>, usize)>,
) -> (usize, usize) {
    let mut after: HashMap<ShareeKey, usize> = HashMap::with_capacity(current.len());
    for (sharee, _) in current {
        let key = sharee_key(sharee);
        if !index.revokes(&key) {
            after.insert(key, sharee.as_str().len());
        }
    }
    for (keys, bytes) in grants {
        if !keys.iter().any(|key| after.contains_key(key))
            && let Some(first) = keys.into_iter().next()
        {
            after.insert(first, bytes);
        }
    }
    (after.len(), after.values().sum())
}

/// The location of a stored sharee named by it (for an ACE or Nextcloud).
fn sharee_location(sharee: &Sharee) -> Result<Href, ShareError> {
    match dav_sharee(sharee) {
        Some(DavSharee::Principal(at)) => Ok(at),
        _ => Err(ShareError::Unnamed),
    }
}

/// After a change: no sharee it took away is still listed, and (Calendar
/// Server, Nextcloud) nobody it gave access to has more than asked (an
/// entry the change did not reach, another client's change in between):
/// principals, addresses and listed sharees alike. Linear: `after` is
/// indexed by key.
fn check_applied(
    before: &[(Sharee, Rights)],
    after: &HashMap<ShareeKey, Rights>,
    update: &ShareUpdate,
    index: &UpdateIndex,
    exact: bool,
) -> Result<(), ShareError> {
    let revoked: HashSet<ShareeKey> = before
        .iter()
        .map(|(sharee, _)| sharee_key(sharee))
        .filter(|key| index.revokes(key))
        .collect();
    if after
        .keys()
        .any(|key| revoked.contains(key) || index.removes(key))
    {
        return Err(ShareError::Ignored);
    }
    if exact {
        let more = |asked: Rights, listed: Rights| -> Result<bool, ShareError> {
            Ok(rights_access(asked)?.is_some_and(|access| !access_rights(access).contains(listed)))
        };
        for (principal, rights) in &update.set {
            if more(*rights, principal_rights(after, principal))? {
                return Err(ShareError::Ignored);
            }
        }
        for (address, rights) in &update.set_addresses {
            let listed = after
                .get(&address_key(address))
                .copied()
                .unwrap_or_default();
            if more(*rights, listed)? {
                return Err(ShareError::Ignored);
            }
        }
        for (sharee, rights) in &update.set_sharees {
            let listed = after.get(&sharee_key(sharee)).copied().unwrap_or_default();
            if more(*rights, listed)? {
                return Err(ShareError::Ignored);
            }
        }
    }
    Ok(())
}

/// A stored sharee as the DAV client writes it.
fn dav_sharee(sharee: &Sharee) -> Option<DavSharee> {
    let text = sharee.as_str();
    if text.starts_with("mailto:") {
        return CalendarAddress::parse(text).map(DavSharee::Address);
    }
    let url = url::Url::parse(text).ok()?;
    Href::from_url(&url).ok().map(DavSharee::Principal)
}

/// How a principal is named in a `CS:share`: its first address (sabre/dav
/// only knows sharees by address), else its location.
fn principal_sharee(principal: &Principal) -> DavSharee {
    principal.addresses.first().map_or_else(
        || DavSharee::Principal(principal.href.clone()),
        |address| DavSharee::Address(address.clone()),
    )
}

/// A collection's sharees by key (their places in the list), built once
/// per change.
fn shares_by_key(shares: &[(Sharee, Rights)]) -> HashMap<ShareeKey, Vec<usize>> {
    let mut out: HashMap<ShareeKey, Vec<usize>> = HashMap::with_capacity(shares.len());
    for (at, (sharee, _)) in shares.iter().enumerate() {
        out.entry(sharee_key(sharee)).or_default().push(at);
    }
    out
}

/// How the server names this principal among the collection's
/// sharees (by address or by location), as it listed them.
fn current_sharees(
    shares: &[(Sharee, Rights)],
    by_key: &HashMap<ShareeKey, Vec<usize>>,
    principal: &Principal,
) -> Vec<DavSharee> {
    let mut places: Vec<usize> = principal_keys(principal)
        .filter_map(|key| by_key.get(&key))
        .flatten()
        .copied()
        .collect();
    places.sort_unstable();
    places.dedup();
    let found: Vec<DavSharee> = places
        .into_iter()
        .filter_map(|at| shares.get(at))
        .filter_map(|(sharee, _)| dav_sharee(sharee))
        .collect();
    if found.is_empty() {
        // Not listed (yet): remove it by the name we would share with.
        vec![principal_sharee(principal)]
    } else {
        found
    }
}

/// Why a share can't be changed (no server text in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShareError {
    /// The user may not share this collection here (the server has no
    /// sharing, or not for this one).
    #[error("this collection can't be shared")]
    NotShareable,
    /// The ACL has entries the bridge could not write back as they are.
    #[error("the collection's ACL can't be changed safely")]
    AclNotRewritable,
    /// More sharees than allowed.
    #[error("too many sharees")]
    TooMany,
    /// The server answered yes but did not share with someone (Nextcloud
    /// does that for a principal it does not share with).
    #[error("the server did not share it with someone")]
    Ignored,
    /// A principal the server's sharing cannot name.
    #[error("this principal can't be named for sharing")]
    Unnamed,
    /// Free/busy only, on a server whose sharing only gives read or
    /// read-write (Nextcloud, Calendar Server): nothing is sent, so it
    /// never becomes full read access.
    #[error("this server can't share free/busy only")]
    FreeBusyOnly,
    /// The server answered yes but someone to remove still has access.
    #[error("the server did not remove someone")]
    NotRemoved,
    /// Rights this server's sharing can't give exactly (only "read" and
    /// "read and write"): never rounded up to more.
    #[error("this server can't share with exactly these rights")]
    RightsNotSupported,
    /// A change in several requests ran out of time between two of them:
    /// part of it was sent (the store shows what), the rest can be sent
    /// again.
    #[error("the change was only partly made; try again")]
    Unfinished,
}

impl GroupwareAccount {
    /// The user's principal on the server of `kind`'s collections (with
    /// `contacts_url`, address books are on another server, with a
    /// principal of their own there).
    fn own_principal_for(&self, kind: CollectionKind) -> Result<Href, GroupwareError> {
        self.discovered()
            .map(|found| found.principal_for(kind).clone())
            .ok_or(GroupwareError::NotDiscovered)
    }

    /// Changes who a live collection is shared with, the way its
    /// [`ShareMode`] says, then syncs (the store shows what the server
    /// did).
    ///
    /// Changes of one account go one at a time. Nothing is sent for a
    /// change that would leave the collection with more than
    /// [`MAX_SHARES`] sharees, or the account with more than its budget
    /// (`max_account_shares`, `max_account_share_bytes`): the next sync
    /// would leave it without a mode.
    ///
    /// # Errors
    /// [`GroupwareError::Share`] if it can't be shared or changed safely
    /// ([`ShareError::TooMany`], [`ShareError::Unfinished`]…);
    /// [`GroupwareError::NotFound`]; the server's refusal.
    pub async fn share_collection(
        &self,
        id: CollectionId,
        update: &ShareUpdate,
    ) -> Result<(), GroupwareError> {
        if update.entries() > MAX_SHARE_CHANGE {
            return Err(ShareError::TooMany.into());
        }
        // One change at a time per account: each one reads the sharees
        // (or the ACL) and writes them back. Before reading anything: a
        // replacement is computed from the listing stored.
        let _turn = self.share_turn().await;
        let started = Instant::now();
        let mut collection = self.live_collection(id).await?;
        // Only Calendar Server sharing names people by address.
        if !update.set_addresses.is_empty()
            && collection.props.sharing.mode != Some(ShareMode::CalendarServer)
        {
            return Err(ShareError::Unnamed.into());
        }
        let mode = collection
            .props
            .sharing
            .mode
            .ok_or(ShareError::NotShareable)?;
        let mut fresh = false;
        if matches!(mode, ShareMode::Nextcloud | ShareMode::CalendarServer) {
            // Checked before anything is sent: their "read" would show
            // the events.
            if update.grants_free_busy_only() {
                return Err(ShareError::FreeBusyOnly.into());
            }
            // A whole `shareWith` removes whoever is there now, also
            // someone another client added since the last round (ACL
            // reads the ACL again by itself); a change in several
            // requests leaves out who has what it asks already.
            let per_request = if mode == ShareMode::Nextcloud {
                MAX_OC_SHARE_REQUEST
            } else {
                MAX_SHARE_REQUEST
            };
            if update.replace || update.entries() > per_request {
                self.relist().await?;
                collection = self.live_collection(id).await?;
                fresh = true;
            }
        }
        let index = UpdateIndex::new(update);
        // `exact`: Calendar Server and Nextcloud list each sharee's
        // access, so nobody may end up with more than asked.
        let (replaced, exact) = match mode {
            ShareMode::Acl => {
                self.share_by_acl(&collection, update).await?;
                (Vec::new(), false)
            }
            ShareMode::CalendarServer => {
                let grants = update.grants(|p| sharee_text(&principal_sharee(p)).len());
                let (count, bytes) = projected(&collection.props.sharing.shares, &index, grants);
                self.check_totals(&collection, count, bytes).await?;
                let replaced = self
                    .share_by_invite(&collection, update, &index, fresh, started)
                    .await?;
                (replaced, true)
            }
            ShareMode::Nextcloud => {
                let grants = update.grants(|p| p.href.as_collection().url().as_str().len());
                let (count, bytes) = projected(&collection.props.sharing.shares, &index, grants);
                self.check_totals(&collection, count, bytes).await?;
                let replaced = self
                    .share_by_nextcloud(&collection, update, &index, fresh, started)
                    .await?;
                (replaced, true)
            }
        };
        self.relist().await?;
        let after = self.live_collection(id).await?;
        let shares = &after.props.sharing.shares;
        let replaced: HashSet<ShareeKey> = replaced.iter().map(sharee_key).collect();
        // A server that answers yes and keeps someone to remove (or a
        // sharee it lists by another name) is not a success.
        if shares.iter().any(|(sharee, _)| {
            let key = sharee_key(sharee);
            index.removes(&key) || replaced.contains(&key)
        }) {
            return Err(ShareError::NotRemoved.into());
        }
        let listed = rights_by_key(shares);
        match mode {
            // An address the server does not take (sabre marks it
            // `invite-invalid`) is not among the sharees afterwards.
            ShareMode::CalendarServer if !update.addresses_shared(&listed) => {
                return Err(ShareError::Ignored.into());
            }
            ShareMode::Nextcloud => check_nextcloud_shares(update, &listed)?,
            _ => {}
        }
        check_applied(
            &collection.props.sharing.shares,
            &listed,
            update,
            &index,
            exact,
        )?;
        Ok(())
    }

    /// The collection after a change would have `count` sharees named in
    /// `bytes`: within [`MAX_SHARES`], and the account within its budget
    /// (its own collections are kept first, so the others of its own
    /// count, and all of them for someone else's).
    async fn check_totals(
        &self,
        collection: &Collection,
        count: usize,
        bytes: usize,
    ) -> Result<(), GroupwareError> {
        if count > MAX_SHARES {
            return Err(ShareError::TooMany.into());
        }
        let (mut total, mut total_bytes) = (count, bytes);
        for other in self.store().dav_collections(self.id).await? {
            if other.id == collection.id
                || other.orphaned_at.is_some()
                || other.props.sharing.owner.is_some()
            {
                continue;
            }
            total += other.props.sharing.shares.len();
            total_bytes += shares_bytes(&other.props.sharing.shares);
        }
        let config = self.config();
        if total > config.max_account_shares || total_bytes > config.max_account_share_bytes {
            return Err(ShareError::TooMany.into());
        }
        Ok(())
    }

    /// Before the `POST` numbered `sent` of `total` (a change sent one
    /// by one): out of time (`share_deadline`) after the first, the store
    /// is read again (so it shows what the server did) and the change is
    /// [`ShareError::Unfinished`].
    async fn before_batch(
        &self,
        sent: usize,
        total: usize,
        started: Instant,
    ) -> Result<(), GroupwareError> {
        if sent > 0 && started.elapsed() >= self.config().share_deadline {
            tracing::warn!(sent, total, "sharing: out of time between requests");
            self.relist_after_partial().await;
            return Err(ShareError::Unfinished.into());
        }
        Ok(())
    }

    /// The `POST` numbered `sent` failed: after the first, the store is
    /// read again first. The failure is what the change returns.
    async fn batch_failed(&self, sent: usize, total: usize, error: DavError) -> GroupwareError {
        if sent > 0 {
            tracing::warn!(sent, total, "sharing: a request after the first failed");
            self.relist_after_partial().await;
        }
        error.into()
    }

    /// A sync round after part of a change was sent; its failure is only
    /// logged (the change's own error is what the caller gets).
    async fn relist_after_partial(&self) {
        if let Err(error) = self.relist().await {
            tracing::warn!(account = %self.id, %error, "sharing: sync after a partial change failed");
        }
    }

    /// Nextcloud: `O:share`s naming every principal by its path below the
    /// DAV root (the user's principal location before `principals/`).
    /// Returns the listed sharees a whole `shareWith` removed.
    async fn share_by_nextcloud(
        &self,
        collection: &Collection,
        update: &ShareUpdate,
        index: &UpdateIndex,
        fresh: bool,
        started: Instant,
    ) -> Result<Vec<Sharee>, GroupwareError> {
        let at = href(&collection.props.href)?.as_collection();
        let own = self.own_principal_for(collection.props.kind)?;
        let root = OcPrincipal::dav_root(&own).ok_or(ShareError::Unnamed)?;
        let name = |principal: &Href| OcPrincipal::of(principal, &root).ok_or(ShareError::Unnamed);
        let mut set: Vec<(OcPrincipal, ShareAccess)> = Vec::new();
        let mut remove: Vec<OcPrincipal> = Vec::new();
        for (principal, rights) in &update.set {
            let named = name(&principal.href)?;
            match rights_access(*rights)? {
                Some(access) => set.push((named, access)),
                None => remove.push(named),
            }
        }
        for principal in &update.remove {
            remove.push(name(&principal.href)?);
        }
        // Listed sharees, by the location they are listed at.
        for (sharee, rights) in &update.set_sharees {
            let named = name(&sharee_location(sharee)?)?;
            match rights_access(*rights)? {
                Some(access) => set.push((named, access)),
                None => remove.push(named),
            }
        }
        for sharee in &update.remove_sharees {
            remove.push(name(&sharee_location(sharee)?)?);
        }
        let shares = &collection.props.sharing.shares;
        let mut replaced = Vec::new();
        if update.replace {
            for (sharee, _) in shares {
                let current = dav_sharee(sharee).and_then(|s| match s {
                    DavSharee::Principal(at) => OcPrincipal::of(&at, &root),
                    DavSharee::Address(_) => None,
                });
                if !index.keeps(&sharee_key(sharee))
                    && let Some(current) = current
                {
                    remove.push(current);
                    replaced.push(sharee.clone());
                }
            }
        }
        let (mut set, remove) = merge_names(set, remove, Clone::clone);
        if fresh && set.len() + remove.len() > MAX_OC_SHARE_REQUEST {
            let listed = rights_by_key(shares);
            set.retain(|(named, access)| {
                named
                    .href(&root)
                    .and_then(|at| listed.get(&location_key(&at)))
                    != Some(&access_rights(*access))
            });
        }
        let client = self.client_for(collection.props.kind);
        let batches = share_batches(set, remove, MAX_OC_SHARE_REQUEST);
        let total = batches.len();
        for (sent, (set, remove)) in batches.iter().enumerate() {
            self.before_batch(sent, total, started).await?;
            if let Err(error) = client.oc_share(&at, set, remove).await {
                return Err(self.batch_failed(sent, total, error).await);
            }
        }
        Ok(replaced)
    }

    async fn share_by_acl(
        &self,
        collection: &Collection,
        update: &ShareUpdate,
    ) -> Result<(), GroupwareError> {
        let at = href(&collection.props.href)?.as_collection();
        let acl = self.client_for(collection.props.kind).read_acl(&at).await?;
        if !acl.is_rewritable(&at) {
            return Err(ShareError::AclNotRewritable.into());
        }
        let own = self.own_principal_for(collection.props.kind)?;
        let owner = collection
            .props
            .sharing
            .owner
            .as_ref()
            .map(href)
            .transpose()?;
        // Each principal named once: a removal wins, two sets give what
        // both allow (never more than either asked).
        let mut named: Vec<(Href, Option<Rights>)> = Vec::new();
        let mut places: HashMap<LocationKey, usize> = HashMap::new();
        let mut name = |principal: &Href, rights: Option<Rights>| {
            let key = principal.location_key();
            if let Some((_, known)) = places.get(&key).and_then(|i| named.get_mut(*i)) {
                *known = match (*known, rights) {
                    (Some(a), Some(b)) => Some(a.intersection(b)),
                    _ => None,
                };
            } else {
                places.insert(key, named.len());
                named.push((principal.clone(), rights));
            }
        };
        for principal in &update.remove {
            name(&principal.href, None);
        }
        for sharee in &update.remove_sharees {
            name(&sharee_location(sharee)?, None);
        }
        for (principal, rights) in &update.set {
            name(&principal.href, Some(*rights));
        }
        // Listed sharees are named by their location.
        for (sharee, rights) in &update.set_sharees {
            name(&sharee_location(sharee)?, Some(*rights));
        }
        let touched = |principal: &Href| {
            update.replace
                && !is_own(principal, &own)
                && owner.as_ref().is_none_or(|o| !is_own(principal, o))
                || places.contains_key(&principal.location_key())
        };
        let mut aces: Vec<Ace> = acl
            .editable()
            .filter(|ace| {
                ace.granted_href()
                    .is_none_or(|principal| !touched(principal))
            })
            .cloned()
            .collect();
        for (at, rights) in &named {
            let Some(rights) = rights else {
                continue;
            };
            let privileges = rights_privileges(collection.props.kind, *rights);
            if !privileges.is_empty() {
                aces.push(Ace::grant(at.as_collection(), &privileges));
            }
        }
        if aces.len() > MAX_ACES {
            return Err(ShareError::TooMany.into());
        }
        // The sharees the ACL will list: its principals with a grant.
        let mut sharees: HashMap<LocationKey, usize> = HashMap::new();
        for principal in aces.iter().filter_map(Ace::granted_href) {
            if !is_own(principal, &own) && owner.as_ref().is_none_or(|o| !is_own(principal, o)) {
                sharees.insert(
                    principal.location_key(),
                    principal.as_collection().url().as_str().len(),
                );
            }
        }
        self.check_totals(collection, sharees.len(), sharees.values().sum())
            .await?;
        self.client_for(collection.props.kind)
            .set_acl(&at, &aces)
            .await?;
        Ok(())
    }

    /// Calendar Server: `CS:share`s. Returns the listed sharees a whole
    /// `shareWith` removed.
    async fn share_by_invite(
        &self,
        collection: &Collection,
        update: &ShareUpdate,
        index: &UpdateIndex,
        fresh: bool,
        started: Instant,
    ) -> Result<Vec<Sharee>, GroupwareError> {
        let at = href(&collection.props.href)?.as_collection();
        let shares = &collection.props.sharing.shares;
        let by_key = shares_by_key(shares);
        let mut set: Vec<(DavSharee, ShareAccess)> = Vec::new();
        let mut remove: Vec<DavSharee> = Vec::new();
        for (principal, rights) in &update.set {
            match rights_access(*rights)? {
                // Every entry the server lists for them (by another of
                // their addresses too) gets the new access: none is left
                // with more.
                Some(access) => set.extend(
                    current_sharees(shares, &by_key, principal)
                        .into_iter()
                        .map(|name| (name, access)),
                ),
                None => remove.extend(current_sharees(shares, &by_key, principal)),
            }
        }
        for principal in &update.remove {
            remove.extend(current_sharees(shares, &by_key, principal));
        }
        for (address, rights) in &update.set_addresses {
            let named = DavSharee::Address(address.clone());
            match rights_access(*rights)? {
                Some(access) => set.push((named, access)),
                None => remove.push(named),
            }
        }
        remove.extend(
            update
                .remove_addresses
                .iter()
                .map(|address| DavSharee::Address(address.clone())),
        );
        // Listed sharees, by the name they are listed with.
        for (sharee, rights) in &update.set_sharees {
            let named = dav_sharee(sharee).ok_or(ShareError::Unnamed)?;
            match rights_access(*rights)? {
                Some(access) => set.push((named, access)),
                None => remove.push(named),
            }
        }
        for sharee in &update.remove_sharees {
            remove.push(dav_sharee(sharee).ok_or(ShareError::Unnamed)?);
        }
        let mut replaced = Vec::new();
        if update.replace {
            for (sharee, _) in shares {
                if !index.keeps(&sharee_key(sharee))
                    && let Some(named) = dav_sharee(sharee)
                {
                    remove.push(named);
                    replaced.push(sharee.clone());
                }
            }
        }
        let (mut set, remove) = merge_names(set, remove, dav_key);
        if fresh && set.len() + remove.len() > MAX_SHARE_REQUEST {
            let listed = rights_by_key(shares);
            set.retain(|(named, access)| {
                listed.get(&dav_key(named)) != Some(&access_rights(*access))
            });
        }
        if set.is_empty() && remove.is_empty() {
            return Ok(replaced);
        }
        let client = self.client_for(collection.props.kind);
        // Every request is checked before the first is sent.
        client.check_share(&at, &set, &remove)?;
        let batches = share_batches(set, remove, MAX_SHARE_REQUEST);
        let total = batches.len();
        for (sent, (set, remove)) in batches.iter().enumerate() {
            self.before_batch(sent, total, started).await?;
            if let Err(error) = client.share(&at, set, remove).await {
                return Err(self.batch_failed(sent, total, error).await);
            }
        }
        Ok(replaced)
    }

    /// The principal a stored sharee is, if the server showed it (its own
    /// listing or searches): by location, or by address.
    #[must_use]
    pub fn sharee_principal(&self, sharee: &Sharee) -> Option<Principal> {
        let key = sharee_key(sharee);
        self.known_principals()
            .into_iter()
            .find(|p| principal_keys(p).any(|k| k == key))
    }

    /// The ways the user can share their own calendars now (one per
    /// [`ShareMode`] found among them), for `mayShareWith`.
    ///
    /// # Errors
    /// The store's.
    pub async fn calendar_share_modes(&self) -> Result<Vec<ShareMode>, GroupwareError> {
        let mut modes = Vec::new();
        for collection in self.store().dav_collections(self.id).await? {
            if collection.props.kind != CollectionKind::Calendar
                || collection.orphaned_at.is_some()
                || collection.props.sharing.owner.is_some()
            {
                continue;
            }
            if let Some(mode) = collection.props.sharing.mode
                && !modes.contains(&mode)
            {
                modes.push(mode);
            }
        }
        Ok(modes)
    }

    /// Whether `principal` can be named as a sharee with `mode`: any
    /// other principal with ACL and Calendar Server sharing; with
    /// Nextcloud, one below the user's DAV root.
    #[must_use]
    pub fn can_name(&self, principal: &Principal, mode: ShareMode) -> bool {
        if principal.own {
            return false;
        }
        match mode {
            ShareMode::Nextcloud => self
                .own_principal_for(CollectionKind::Calendar)
                .ok()
                .and_then(|own| OcPrincipal::dav_root(&own))
                .and_then(|root| OcPrincipal::of(&principal.href, &root))
                .is_some(),
            ShareMode::Acl | ShareMode::CalendarServer => true,
        }
    }

    /// The principal at `owner` (a collection's owner), if known.
    #[must_use]
    pub fn principal_at(&self, owner: &DavHref) -> Option<Principal> {
        let at = href(owner).ok()?;
        self.known_principals()
            .into_iter()
            .find(|p| p.href.same_location(&at))
    }
}

/// After an `O:share`: every principal given access is among the sharees
/// now (Nextcloud answers 200 when it ignores one).
fn check_nextcloud_shares(
    update: &ShareUpdate,
    shares: &HashMap<ShareeKey, Rights>,
) -> Result<(), GroupwareError> {
    let missing = update
        .set
        .iter()
        .filter(|(_, rights)| grants_access(*rights))
        .any(|(principal, _)| !principal_keys(principal).any(|key| shares.contains_key(&key)));
    if missing {
        return Err(ShareError::Ignored.into());
    }
    Ok(())
}

/// The text of a sharee as the DAV client names it.
fn sharee_text(sharee: &DavSharee) -> String {
    match sharee {
        DavSharee::Address(address) => address.as_str().to_owned(),
        DavSharee::Principal(at) => at.url().as_str().to_owned(),
    }
}

/// The length of a list's sharee names.
pub(crate) fn shares_bytes(shares: &[(Sharee, Rights)]) -> usize {
    shares.iter().map(|(sharee, _)| sharee.as_str().len()).sum()
}

/// The principal number (for ids) of a stored principal location (an
/// owner).
#[must_use]
pub fn location_number(location: &DavHref) -> Option<i64> {
    href(location).ok().map(|at| principal_number(&at))
}

/// The last segment of a stored location, decoded (a name of last
/// resort).
#[must_use]
pub fn location_label(location: &DavHref) -> Option<String> {
    href(location)
        .ok()
        .and_then(|at| at.last_segment().map(str::to_owned))
}

impl From<ShareError> for GroupwareError {
    fn from(error: ShareError) -> Self {
        Self::Share(error)
    }
}

/// A DAV failure that means "the server does not let you share this"
/// rather than "try again".
#[must_use]
pub const fn is_refusal(error: &GroupwareError) -> bool {
    matches!(
        error,
        GroupwareError::Share(_)
            | GroupwareError::Dav(
                DavError::Forbidden { .. } | DavError::MethodNotAllowed | DavError::Conflict { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use nebula_dav::proto::{
        AcePrincipal, Acl, InviteUser, Property, SharingModes, XmlLimits, parse_multistatus,
    };
    use url::Url;

    use super::*;
    use crate::principals::PrincipalKind;

    fn base() -> Href {
        Href::from_url(&Url::parse("http://localhost:8080/").unwrap()).unwrap()
    }

    fn at(path: &str) -> Href {
        Href::resolve(path, &base()).unwrap()
    }

    fn principal(path: &str, address: &str) -> Principal {
        Principal {
            href: at(path),
            name: None,
            addresses: vec![CalendarAddress::parse(address).unwrap()],
            kind: PrincipalKind::Individual,
            own: false,
        }
    }

    fn listed(xml_props: &str, home: &str) -> Listed {
        let kind = if xml_props.contains("resourcetype") {
            ""
        } else {
            "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>"
        };
        let xml = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/" xmlns:O="http://owncloud.org/ns"><D:response><D:href>{home}x/</D:href><D:propstat><D:prop>{kind}{xml_props}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        );
        let multistatus =
            parse_multistatus(xml.as_bytes(), base().url(), &XmlLimits::default()).unwrap();
        let response = &multistatus.responses[0];
        let p = &response.properties;
        assert!(p.has(Property::ResourceType));
        Listed::from_properties(
            response.href().clone(),
            ListedKind::Calendar,
            p.clone(),
            true,
        )
    }

    const STALWART: SharingSupport = SharingSupport {
        acl: true,
        calendar_server: false,
        resource_sharing: false,
        nextcloud: false,
    };
    const SABRE: SharingSupport = SharingSupport {
        acl: true,
        calendar_server: true,
        resource_sharing: true,
        nextcloud: false,
    };
    /// Nextcloud 34: ACL, Calendar Server sharing announced (but not
    /// working) and its own.
    const NEXTCLOUD: SharingSupport = SharingSupport {
        acl: true,
        calendar_server: true,
        resource_sharing: false,
        nextcloud: true,
    };

    #[test]
    fn nextcloud_both_sides() {
        let alice = at("/remote.php/dav/principals/users/alice/");
        let bob = at("/remote.php/dav/principals/users/bob/");
        let all = "<D:current-user-privilege-set><D:privilege><D:write-acl/></D:privilege><D:privilege><D:write/></D:privilege><D:privilege><D:read/></D:privilege></D:current-user-privilege-set>";
        let invite = "<O:invite><O:user><D:href>principal:principals/users/bob</D:href><O:common-name>bob</O:common-name><O:invite-accepted/><O:access><O:read-write/></O:access></O:user><O:user><D:href>principal:principals/groups/admin</D:href><O:invite-accepted/><O:access><O:read/></O:access></O:user><O:user><D:href>principal:principals/users/alice</D:href><O:invite-accepted/></O:user></O:invite>";
        let props = format!(
            "<D:owner><D:href>/remote.php/dav/principals/users/alice/</D:href></D:owner>{all}<CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>{invite}"
        );
        let owner = listed(&props, "/remote.php/dav/calendars/alice/");
        let sharing = collection_sharing(&owner, &alice, NEXTCLOUD, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.owner, None);
        // Nextcloud's own, not the Calendar Server sharing it announces.
        assert_eq!(sharing.mode, Some(ShareMode::Nextcloud));
        assert_eq!(
            sharing.shares,
            [
                (
                    Sharee::new("http://localhost:8080/remote.php/dav/principals/users/bob/")
                        .unwrap(),
                    access_rights(ShareAccess::ReadWrite)
                ),
                (
                    Sharee::new("http://localhost:8080/remote.php/dav/principals/groups/admin/")
                        .unwrap(),
                    access_rights(ShareAccess::Read)
                ),
            ]
        );
        // A sharee with access whose href is not a `principal:` cannot be
        // removed by a replacement: no mode. One without access does not
        // matter.
        for (user, mode) in [
            (
                "<O:user><D:href>mailto:x@y.example</D:href><O:invite-accepted/></O:user>",
                None,
            ),
            (
                "<O:user><D:href>mailto:x@y.example</D:href><O:invite-declined/></O:user>",
                Some(ShareMode::Nextcloud),
            ),
        ] {
            let props = props.replace("</O:invite>", &format!("{user}</O:invite>"));
            let odd = listed(&props, "/remote.php/dav/calendars/alice/");
            let sharing = collection_sharing(&odd, &alice, NEXTCLOUD, &[])
                .unwrap()
                .unwrap();
            assert_eq!(sharing.mode, mode, "{user}");
            assert_eq!(sharing.shares.is_empty(), mode.is_none(), "{user}");
        }
        // Without write-acl (the birthday calendar) or without O:invite:
        // not shareable.
        let birthdays = listed(
            "<D:owner><D:href>/remote.php/dav/principals/users/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:read/></D:privilege></D:current-user-privilege-set><O:invite/>",
            "/remote.php/dav/calendars/alice/",
        );
        let sharing = collection_sharing(&birthdays, &alice, NEXTCLOUD, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.mode, None);
        let no_invite = listed(all, "/remote.php/dav/calendars/alice/");
        assert_eq!(
            collection_sharing(&no_invite, &alice, NEXTCLOUD, &[])
                .unwrap()
                .unwrap()
                .mode,
            None
        );
        // Bob's view: in his home, owned by alice, an empty invite.
        let copy = listed(
            &format!(
                "<D:owner><D:href>/remote.php/dav/principals/users/alice/</D:href></D:owner>{all}<O:invite/>"
            ),
            "/remote.php/dav/calendars/bob/",
        );
        let sharing = collection_sharing(&copy, &bob, NEXTCLOUD, &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            sharing.owner.unwrap().as_str(),
            "http://localhost:8080/remote.php/dav/principals/users/alice/"
        );
        assert_eq!(sharing.mode, None);
        assert!(sharing.shares.is_empty());
        // Address books too.
        let mut book = owner.clone();
        book.kind = ListedKind::Addressbook;
        assert_eq!(
            collection_sharing(&book, &alice, NEXTCLOUD, &[])
                .unwrap()
                .unwrap()
                .mode,
            Some(ShareMode::Nextcloud)
        );
        // A principal outside any `principals/` path can't be the user's
        // root: nothing to name sharees with.
        let odd = at("/remote.php/dav/users/alice/");
        assert_eq!(
            collection_sharing(&owner, &odd, NEXTCLOUD, &[])
                .unwrap()
                .unwrap()
                .mode,
            None
        );
    }

    #[test]
    fn stalwart_owner_side() {
        let own = at("/dav/pal/alice/");
        let c = listed(
            "<D:owner><D:href>/dav/pal/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set><D:acl><D:ace><D:principal><D:href>/dav/pal/bob/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege><D:privilege><C:read-free-busy/></D:privilege></D:grant></D:ace></D:acl>",
            "/dav/cal/alice/",
        );
        let sharing = collection_sharing(&c, &own, STALWART, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.owner, None);
        assert_eq!(sharing.mode, Some(ShareMode::Acl));
        assert_eq!(
            sharing.shares,
            [(
                Sharee::new("http://localhost:8080/dav/pal/bob/").unwrap(),
                Rights::READ.union(Rights::READ_FREE_BUSY)
            )]
        );
        // Without write-acl, or with an ACL the bridge could not write
        // back: not shareable.
        let c = listed(
            "<D:current-user-privilege-set><D:privilege><D:read/></D:privilege></D:current-user-privilege-set><D:acl/>",
            "/dav/cal/alice/",
        );
        let sharing = collection_sharing(&c, &own, STALWART, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.mode, None);
        assert!(sharing.shares.is_empty());
        // Radicale: no access-control.
        let c = listed(
            "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set><D:acl/>",
            "/dav/cal/alice/",
        );
        let none = SharingSupport::default();
        assert_eq!(
            collection_sharing(&c, &own, none, &[]).unwrap().unwrap(),
            CollectionSharing::default()
        );
    }

    #[test]
    fn stalwart_sharee_side() {
        let own = at("/dav/pal/bob/");
        // Listed in bob's home by "assisted discovery": kept, alice's.
        let mut c = listed(
            "<D:owner><D:href>/dav/pal/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:read/></D:privilege></D:current-user-privilege-set>",
            "/dav/cal/alice/",
        );
        c.in_home = false;
        let sharing = collection_sharing(&c, &own, STALWART, &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            sharing.owner.unwrap().as_str(),
            "http://localhost:8080/dav/pal/alice/"
        );
        assert_eq!(sharing.mode, None);
        // Outside the home and nobody else's: left out.
        let mut mine = listed(
            "<D:owner><D:href>/dav/pal/bob/</D:href></D:owner>",
            "/dav/cal/other/",
        );
        mine.in_home = false;
        assert_eq!(
            collection_sharing(&mine, &own, STALWART, &[]).unwrap(),
            None
        );
    }

    #[test]
    fn sabre_both_sides() {
        let alice = at("/dav.php/principals/alice/");
        let bob = at("/dav.php/principals/bob/");
        let owner = listed(
            "<D:owner><D:href>/dav.php/principals/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:all/></D:privilege><D:privilege><D:share/></D:privilege></D:current-user-privilege-set><CS:invite><CS:user><CS:invite-accepted/><CS:access/><D:href>/dav.php/principals/alice</D:href></CS:user><CS:user><CS:invite-accepted/><CS:access><CS:read-write/></CS:access><D:href>mailto:bob@nebulamail.test</D:href></CS:user><CS:user><CS:invite-invalid/><CS:access><CS:read/></CS:access><D:href>/dav.php/principals/nobody/</D:href></CS:user></CS:invite><CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>",
            "/dav.php/calendars/alice/",
        );
        let sharing = collection_sharing(&owner, &alice, SABRE, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.owner, None);
        assert_eq!(sharing.mode, Some(ShareMode::CalendarServer));
        assert_eq!(
            sharing.shares,
            [(
                Sharee::new("mailto:bob@nebulamail.test").unwrap(),
                access_rights(ShareAccess::ReadWrite)
            )]
        );
        let copy = listed(
            "<D:resourcetype><D:collection/><C:calendar/><CS:shared/></D:resourcetype><D:owner><D:href>/dav.php/principals/bob/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:read/></D:privilege><D:privilege><D:write-acl/></D:privilege></D:current-user-privilege-set><CS:invite><CS:user><CS:invite-accepted/><CS:access/><D:href>/dav.php/principals/alice</D:href></CS:user><CS:user><CS:invite-accepted/><CS:access><CS:read/></CS:access><D:href>mailto:bob@nebulamail.test</D:href></CS:user></CS:invite><CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>",
            "/dav.php/calendars/bob/",
        );
        let sharing = collection_sharing(&copy, &bob, SABRE, &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            sharing.owner.unwrap().as_str(),
            "http://localhost:8080/dav.php/principals/alice/"
        );
        // A sharee can't share it again; its write-acl is not for that.
        assert_eq!(sharing.mode, None);
        // sabre/dav address books: write-acl, but no ACL sharing there.
        let mut book = owner.clone();
        book.kind = ListedKind::Addressbook;
        let sharing = collection_sharing(&book, &alice, SABRE, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.mode, None);
    }

    #[test]
    fn owner_by_address() {
        let invite = Invite {
            organizer: Some(DavSharee::Address(
                CalendarAddress::parse("alice@example.com").unwrap(),
            )),
            users: vec![InviteUser {
                sharee: DavSharee::Address(CalendarAddress::parse("bob@example.com").unwrap()),
                access: Some(ShareAccess::Read),
                status: InviteStatus::Accepted,
            }],
            omitted: 0,
        };
        let alice = principal("/principals/alice", "alice@example.com");
        assert_eq!(
            invite_owner(&invite, std::slice::from_ref(&alice)).unwrap(),
            at("/principals/alice/")
        );
        assert_eq!(invite_owner(&invite, &[]), None);
    }

    #[test]
    fn rights_and_privileges() {
        // What the web asks for address books (contacts/sharing.ts).
        let read = Rights::READ;
        let write = read.union(Rights::WRITE_CONTENT).union(Rights::BIND);
        let manage = write.union(Rights::UNBIND);
        let book = CollectionKind::AddressBook;
        assert_eq!(rights_privileges(book, read), [Privilege::Read]);
        assert_eq!(
            rights_privileges(book, write),
            [
                Privilege::Read,
                Privilege::WriteContent,
                Privilege::WriteProperties,
                Privilege::Bind
            ]
        );
        assert_eq!(
            rights_privileges(book, manage),
            [Privilege::Read, Privilege::Write]
        );
        // And back, as Stalwart lists them (no bind for write-content).
        let back = rights_of(PrivilegeSet::new(&[
            Privilege::Read,
            Privilege::WriteContent,
            Privilege::WriteProperties,
        ]));
        assert!(back.contains(write) && !back.contains(Rights::UNBIND));
        let back = rights_of(PrivilegeSet::new(&[Privilege::Read, Privilege::Write]));
        assert!(back.contains(manage));
        // Calendars: write for any change, free-busy on its own.
        let calendar = CollectionKind::Calendar;
        assert_eq!(
            rights_privileges(
                calendar,
                Rights::READ
                    .union(Rights::READ_FREE_BUSY)
                    .union(Rights::WRITE_CONTENT)
                    .union(Rights::WRITE_ACL)
            ),
            [
                Privilege::Read,
                Privilege::ReadFreeBusy,
                Privilege::Write,
                Privilege::WriteAcl
            ]
        );
        assert_eq!(
            rights_privileges(calendar, Rights::READ_FREE_BUSY),
            [Privilege::ReadFreeBusy]
        );
        assert!(rights_privileges(calendar, Rights::empty()).is_empty());
        // Calendar Server: read or read-write, never more than asked.
        let unsupported = Err(ShareError::RightsNotSupported);
        assert_eq!(rights_access(Rights::READ_FREE_BUSY), unsupported);
        assert_eq!(rights_access(Rights::UNBIND), unsupported);
        assert_eq!(rights_access(Rights::WRITE_CONTENT), unsupported);
        assert_eq!(
            rights_access(Rights::READ.union(Rights::UNBIND)),
            unsupported
        );
        assert_eq!(
            rights_access(Rights::READ.union(Rights::WRITE_PROPERTIES)),
            unsupported
        );
        assert_eq!(
            rights_access(Rights::READ.union(Rights::READ_FREE_BUSY)),
            Ok(Some(ShareAccess::Read))
        );
        assert_eq!(
            rights_access(Rights::READ.union(Rights::WRITE_ACL)),
            Ok(Some(ShareAccess::Read))
        );
        assert_eq!(rights_access(manage), Ok(Some(ShareAccess::ReadWrite)));
        assert_eq!(rights_access(write), Ok(Some(ShareAccess::ReadWrite)));
        assert_eq!(rights_access(Rights::empty()), Ok(None));
        assert!(!grants_access(Rights::empty()));
        assert!(grants_access(Rights::READ_FREE_BUSY));
    }

    #[test]
    fn acl_grants_are_joined_and_filtered() {
        let own = at("/dav/pal/alice/");
        let bob = at("/dav/pal/bob/");
        let acl = Acl {
            aces: vec![
                Ace::grant(bob.clone(), &[Privilege::Read]),
                Ace::grant(at("/dav/pal/bob"), &[Privilege::WriteContent]),
                Ace::grant(own.clone(), &[Privilege::All]),
                Ace {
                    deny: true,
                    ..Ace::grant(at("/dav/pal/carol/"), &[Privilege::Read])
                },
                Ace {
                    principal: AcePrincipal::All,
                    ..Ace::grant(bob, &[Privilege::Read])
                },
            ],
        };
        let shares = acl_shares(&acl, &own, None).unwrap();
        assert_eq!(shares.len(), 1);
        assert!(
            shares[0]
                .1
                .contains(Rights::READ.union(Rights::WRITE_CONTENT))
        );
        let _ = SharingModes::default();
    }

    #[test]
    fn sharees_match_principals() {
        let bob = principal("/dav/pal/bob/", "bob@example.com");
        let is = |text: &str| {
            let key = sharee_key(&Sharee::new(text).unwrap());
            principal_keys(&bob).any(|k| k == key)
        };
        assert!(is("mailto:Bob@Example.com"));
        assert!(is("http://localhost:8080/dav/pal/bob"));
        assert!(!is("http://localhost:8081/dav/pal/bob/"));
        assert_eq!(
            principal_sharee(&bob),
            DavSharee::Address(CalendarAddress::parse("bob@example.com").unwrap())
        );
    }

    fn grants(count: usize) -> Acl {
        Acl {
            aces: (0..count)
                .map(|n| Ace::grant(at(&format!("/dav/pal/u{n}/")), &[Privilege::Read]))
                .collect(),
        }
    }

    fn stalwart_calendar(acl: Acl) -> Listed {
        let mut c = listed(
            "<D:owner><D:href>/dav/pal/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set><D:acl/>",
            "/dav/cal/alice/",
        );
        c.acl = Some(acl);
        c
    }

    #[test]
    fn acl_with_many_grants_is_shareable() {
        let own = at("/dav/pal/alice/");
        let c = stalwart_calendar(grants(300));
        let sharing = collection_sharing(&c, &own, STALWART, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.mode, Some(ShareMode::Acl));
        assert_eq!(sharing.shares.len(), 300);
    }

    #[test]
    fn acl_over_max_shares_is_not_truncated() {
        let own = at("/dav/pal/alice/");
        assert!(acl_shares(&grants(MAX_SHARES + 1), &own, None).is_none());
        assert_eq!(
            acl_shares(&grants(MAX_SHARES), &own, None).map(|s| s.len()),
            Some(MAX_SHARES)
        );
        let c = stalwart_calendar(grants(MAX_SHARES + 1));
        let sharing = collection_sharing(&c, &own, STALWART, &[])
            .unwrap()
            .unwrap();
        assert_eq!(sharing.mode, None);
        assert!(sharing.shares.is_empty());
    }

    /// Sharing a split listing did not read is not "shared with nobody":
    /// no mode, whatever the way of sharing (sabre/dav without its
    /// invitation, Stalwart without its ACL, Nextcloud without O:invite).
    #[test]
    fn sharing_not_read_has_no_mode() {
        let alice = at("/dav.php/principals/alice/");
        let sabre = listed(
            "<D:owner><D:href>/dav.php/principals/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:all/></D:privilege><D:privilege><D:share/></D:privilege></D:current-user-privilege-set><CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>",
            "/dav.php/calendars/alice/",
        );
        // Read, without an invitation: never shared, rightly.
        assert_eq!(
            collection_sharing(&sabre, &alice, SABRE, &[])
                .unwrap()
                .unwrap()
                .mode,
            Some(ShareMode::CalendarServer)
        );
        let own = at("/dav/pal/alice/");
        let nextcloud = listed(
            "<D:owner><D:href>/dav/pal/alice/</D:href></D:owner><D:current-user-privilege-set><D:privilege><D:write-acl/></D:privilege><D:privilege><D:read/></D:privilege></D:current-user-privilege-set><O:invite/>",
            "/dav/cal/alice/",
        );
        for (mut c, me, support) in [
            (sabre, &alice, SABRE),
            (stalwart_calendar(grants(3)), &own, STALWART),
            (nextcloud, &own, NEXTCLOUD),
        ] {
            for data in [SharingData::NotFetched, SharingData::TooLarge] {
                c.sharing_data = data;
                let sharing = collection_sharing(&c, me, support, &[]).unwrap().unwrap();
                assert_eq!(sharing.mode, None, "{data:?}");
                assert!(sharing.shares.is_empty());
            }
        }
    }

    fn people(count: usize) -> Vec<Principal> {
        (0..count)
            .map(|n| {
                principal(
                    &format!("/dav/pal/u{n}/"),
                    &format!("u{n}\u{40}example.com"),
                )
            })
            .collect()
    }

    fn listed_as(principals: &[Principal], access: ShareAccess) -> Vec<(Sharee, Rights)> {
        principals
            .iter()
            .map(|p| {
                (
                    Sharee::new(p.addresses[0].as_str()).unwrap(),
                    access_rights(access),
                )
            })
            .collect()
    }

    /// Generous bounds (debug build, a loaded machine): the quadratic
    /// version with a URL parse per comparison took minutes.
    #[test]
    fn check_applied_is_linear() {
        let before_people = people(10_000);
        let before = listed_as(&before_people, ShareAccess::ReadWrite);
        let asked = people(20_000).split_off(10_000);
        let after = listed_as(&asked, ShareAccess::Read);
        let update = ShareUpdate {
            replace: true,
            set: asked
                .iter()
                .map(|p| (p.clone(), access_rights(ShareAccess::Read)))
                .collect(),
            ..ShareUpdate::default()
        };
        let started = std::time::Instant::now();
        let index = UpdateIndex::new(&update);
        let listed = rights_by_key(&after);
        check_applied(&before, &listed, &update, &index, true).unwrap();
        assert!(check_nextcloud_shares(&update, &listed).is_ok());
        assert!(update.addresses_shared(&listed));
        let grants_of = update.grants(|p| p.href.url().as_str().len());
        let (count, _) = projected(&before, &index, grants_of);
        assert_eq!(count, 10_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let own = at("/dav/pal/alice/");
        let started = std::time::Instant::now();
        assert_eq!(
            acl_shares(&grants(10_000), &own, None).map(|s| s.len()),
            Some(10_000)
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn check_applied_still_catches_more_than_asked_and_left_behind() {
        let all = people(1000);
        let before = listed_as(&all, ShareAccess::ReadWrite);
        let update = ShareUpdate {
            set: all
                .iter()
                .map(|p| (p.clone(), access_rights(ShareAccess::Read)))
                .collect(),
            ..ShareUpdate::default()
        };
        let index = UpdateIndex::new(&update);
        // One kept read-write: more than asked.
        let mut after = listed_as(&all, ShareAccess::Read);
        after[500].1 = access_rights(ShareAccess::ReadWrite);
        assert_eq!(
            check_applied(&before, &rights_by_key(&after), &update, &index, true),
            Err(ShareError::Ignored)
        );
        // Not exact (ACL): not checked.
        assert!(check_applied(&before, &rights_by_key(&after), &update, &index, false).is_ok());
        // By a listed sharee too.
        let by_sharee = ShareUpdate {
            set_sharees: vec![(after[500].0.clone(), access_rights(ShareAccess::Read))],
            ..ShareUpdate::default()
        };
        assert_eq!(
            check_applied(
                &before,
                &rights_by_key(&after),
                &by_sharee,
                &UpdateIndex::new(&by_sharee),
                true
            ),
            Err(ShareError::Ignored)
        );
        // A removed one still listed (by its location this time).
        let removal = ShareUpdate {
            remove: vec![all[999].clone()],
            ..ShareUpdate::default()
        };
        let mut still = listed_as(&all, ShareAccess::Read);
        still.push((
            Sharee::new(all[999].href.url().as_str()).unwrap(),
            Rights::READ,
        ));
        let removal_index = UpdateIndex::new(&removal);
        assert_eq!(
            check_applied(
                &before,
                &rights_by_key(&still),
                &removal,
                &removal_index,
                false
            ),
            Err(ShareError::Ignored)
        );
        // A replacement that keeps nobody: anyone left is caught.
        let replace = ShareUpdate {
            replace: true,
            ..ShareUpdate::default()
        };
        assert_eq!(
            check_applied(
                &before,
                &rights_by_key(&before[..1]),
                &replace,
                &UpdateIndex::new(&replace),
                false
            ),
            Err(ShareError::Ignored)
        );
    }

    #[test]
    fn projected_over_the_limit_is_too_many() {
        let current = listed_as(&people(MAX_SHARES - 1), ShareAccess::Read);
        let new = people(MAX_SHARES + 5).split_off(MAX_SHARES - 1);
        let update = ShareUpdate {
            set: new
                .iter()
                .map(|p| (p.clone(), access_rights(ShareAccess::Read)))
                .collect(),
            ..ShareUpdate::default()
        };
        let index = UpdateIndex::new(&update);
        let (count, bytes) = projected(
            &current,
            &index,
            update.grants(|p| sharee_text(&principal_sharee(p)).len()),
        );
        assert_eq!(count, MAX_SHARES + 5);
        assert!(count > MAX_SHARES);
        assert!(bytes > shares_bytes(&current));
        // Setting again who is there adds nobody; removing makes room.
        let again = ShareUpdate {
            set: people(5)
                .into_iter()
                .map(|p| (p, access_rights(ShareAccess::ReadWrite)))
                .collect(),
            remove: people(10).split_off(5),
            ..ShareUpdate::default()
        };
        let (count, _) = projected(
            &current,
            &UpdateIndex::new(&again),
            again.grants(|p| sharee_text(&principal_sharee(p)).len()),
        );
        assert_eq!(count, MAX_SHARES - 1 - 5);
    }
}
