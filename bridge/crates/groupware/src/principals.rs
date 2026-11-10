// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Principals and free-busy (libre/ocupado) for `Principal/get|query` and
//! `Principal/getAvailability`.
//!
//! - **Who:** the user's own principal (from discovery) and the others
//!   the server shows under its `principal-collection-set`: found by
//!   address with `principal-property-search` when the server lets
//!   clients search `calendar-user-address-set` (RFC 3744 §9.5), else in
//!   a Depth 1 listing of the collection (kept 5 minutes). A server that
//!   refuses to list a collection (Nextcloud's users: 405) but lets
//!   clients search `displayname` has that collection "listed" with a
//!   search for `@` (Nextcloud matches names and addresses by substring,
//!   and applies its own user-enumeration settings and its limit of
//!   `sharing.maxAutocompleteResults`, 25 by default, to it). Servers that show neither
//!   (403, 405…) only have the user's own principal.
//! - **Free-busy:** only with a scheduling server (an outbox and
//!   `calendar-auto-schedule`), a POST of a `VFREEBUSY` request to the
//!   outbox (RFC 6638 §5), and only for principals the server showed: an
//!   address the server did not reveal is never asked about (some
//!   servers would send an iMIP request to outsiders). The reply is read
//!   with calcard inside its panic guard ([`model::freebusy`]).
//!
//! Ids are stable numbers of the principal's location (FNV-1a), so the
//! same principal keeps its id across logins and restarts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use nebula_dav::DavError;
use nebula_dav::DavPrincipal;
use nebula_dav::proto::{
    CalendarAddress, CalendarUserType, FreeBusyRequest, Href, LocationKey, Property,
};
use nebula_store::{CollectionKind, MAX_SHARES, Rights, Sharee, UnixTime};

use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::calendar::Zone;
use crate::model::freebusy::{self, BusyPeriod};

/// How long a listing of the server's principals is used.
const LISTING_TTL: Duration = Duration::from_mins(5);
/// Most principals kept from address searches.
const MAX_FOUND: usize = 1000;
/// Most addresses without a principal remembered: a collection full of
/// sharees by address, and room for the searches besides.
const MAX_REMEMBERED_ADDRESSES: usize = MAX_SHARES + MAX_FOUND;
/// What a server that does not list its principals is searched for by
/// `displayname` instead: Nextcloud matches names and addresses by
/// substring, so this finds the users with an address, but at most
/// `sharing.maxAutocompleteResults` of them (25 by default; seen on
/// Nextcloud 34, 2026-11-04): the others are found by address or name
/// only. Raising it makes this one search slow (10 000 users took more
/// than two minutes).
const DIRECTORY_SEARCH: &str = "@";
/// Longest text [`GroupwareAccount::search_by_name`] asks the server
/// about, in characters.
pub const MAX_NAME_SEARCH: usize = 64;

/// What kind of principal (RFC 9670 §2 `type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PrincipalKind {
    /// A person (also when the server does not say).
    Individual,
    /// A group.
    Group,
    /// A resource (a projector, a car…).
    Resource,
    /// A room.
    Location,
    /// Anything else.
    Other,
}

impl PrincipalKind {
    fn from_dav(kind: Option<CalendarUserType>) -> Self {
        match kind {
            None | Some(CalendarUserType::Individual) => Self::Individual,
            Some(CalendarUserType::Group) => Self::Group,
            Some(CalendarUserType::Resource) => Self::Resource,
            Some(CalendarUserType::Room) => Self::Location,
            Some(_) => Self::Other,
        }
    }
}

/// A principal of the DAV server.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Principal {
    /// Where it is (for sharing: the ACL names it by this href).
    pub href: Href,
    /// Its `displayname`.
    pub name: Option<String>,
    /// Its `mailto:` addresses (`calendar-user-address-set`), the first
    /// one preferred.
    pub addresses: Vec<CalendarAddress>,
    /// What kind.
    pub kind: PrincipalKind,
    /// The user's own principal.
    pub own: bool,
}

impl Principal {
    /// A principal at `href`, with no name or address (another
    /// individual).
    #[must_use]
    pub const fn new(href: Href) -> Self {
        Self {
            href,
            name: None,
            addresses: Vec::new(),
            kind: PrincipalKind::Individual,
            own: false,
        }
    }

    fn from_dav(found: DavPrincipal, own: bool) -> Self {
        Self {
            addresses: mailto_addresses(&found.addresses),
            href: found.href,
            name: found.display_name,
            kind: PrincipalKind::from_dav(found.kind),
            own,
        }
    }

    /// A stable number for its id: FNV-1a of its URL (63 bits, at least
    /// 1).
    #[must_use]
    pub fn number(&self) -> i64 {
        principal_number(&self.href)
    }

    /// Whether `address` is one of its addresses.
    #[must_use]
    pub fn has_address(&self, address: &CalendarAddress) -> bool {
        self.addresses
            .iter()
            .any(|own| own.as_str().eq_ignore_ascii_case(address.as_str()))
    }
}

/// The number of a principal at `href`.
#[must_use]
pub fn principal_number(href: &Href) -> i64 {
    fnv_number(href.url().as_str())
}

/// The number of an address without a principal: FNV-1a of its
/// `mailto:` URI without case (the same space as [`principal_number`]:
/// a location is never a `mailto:` URI).
#[must_use]
pub fn address_number(address: &CalendarAddress) -> i64 {
    fnv_number(&address.as_str().to_ascii_lowercase())
}

fn fnv_number(text: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let positive = hash & 0x7fff_ffff_ffff_ffff;
    i64::try_from(positive).unwrap_or(1).max(1)
}

/// The `mailto:` entries of a `calendar-user-address-set` (Radicale puts
/// the principal's path there; it is not an address).
#[must_use]
pub fn mailto_addresses(set: &[String]) -> Vec<CalendarAddress> {
    let mut out: Vec<CalendarAddress> = Vec::new();
    for uri in set {
        let uri = uri.trim();
        let is_mailto = uri
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"));
        if let Some(address) = is_mailto.then(|| CalendarAddress::parse(uri)).flatten()
            && !out
                .iter()
                .any(|known| known.as_str().eq_ignore_ascii_case(address.as_str()))
        {
            out.push(address);
        }
    }
    out
}

/// Principals in the order added, each location once (a set of
/// [`LocationKey`]s, so adding is O(1), not a scan).
#[derive(Debug, Default, Clone)]
pub(crate) struct UniquePrincipals {
    list: Vec<Principal>,
    seen: HashSet<LocationKey>,
}

impl UniquePrincipals {
    /// Adds `principal` unless one at the same location is there.
    pub(crate) fn add(&mut self, principal: Principal) {
        if self.seen.insert(principal.href.location_key()) {
            self.list.push(principal);
        }
    }

    pub(crate) fn contains(&self, href: &Href) -> bool {
        self.seen.contains(&href.location_key())
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    pub(crate) fn into_vec(self) -> Vec<Principal> {
        self.list
    }
}

/// A listing of the principal collections, with an index by number.
#[derive(Debug)]
struct Listing {
    at: Instant,
    list: Vec<Principal>,
    by_number: HashMap<i64, usize>,
    locations: HashSet<LocationKey>,
}

impl Listing {
    fn new(list: Vec<Principal>) -> Self {
        let mut by_number = HashMap::with_capacity(list.len());
        let mut locations = HashSet::with_capacity(list.len());
        for (at, principal) in list.iter().enumerate() {
            by_number.entry(principal.number()).or_insert(at);
            locations.insert(principal.href.location_key());
        }
        Self {
            at: Instant::now(),
            list,
            by_number,
            locations,
        }
    }
}

/// What the server shows of its principals, for a while.
#[derive(Debug, Default)]
pub(crate) struct Directory(Mutex<Cache>);

#[derive(Debug, Default)]
struct Cache {
    /// The last listing of the principal collections, and when.
    listing: Option<Listing>,
    /// Whether the server lets clients search by address (asked once).
    address_search: Option<bool>,
    /// Whether the server lets clients search by `displayname` (asked
    /// once).
    name_search: Option<bool>,
    /// Principals found by address searches.
    found: UniquePrincipals,
    /// Addresses without a principal the bridge gave a number to (for
    /// servers that share by address).
    addresses: Addresses,
    /// Principal collections the server would not list (searched by
    /// name instead).
    unlisted: Vec<Href>,
}

/// What an address number stands for.
#[derive(Debug, Clone)]
enum Remembered {
    One(CalendarAddress),
    /// Two addresses with one number (FNV can be made to collide): it
    /// stands for neither, never the second for the first's id.
    Ambiguous,
}

/// Addresses by number, the oldest forgotten first past
/// [`MAX_REMEMBERED_ADDRESSES`].
#[derive(Debug, Default)]
struct Addresses {
    by_number: HashMap<i64, Remembered>,
    order: VecDeque<i64>,
}

impl Addresses {
    fn remember(&mut self, number: i64, address: &CalendarAddress) {
        if let Some(known) = self.by_number.get_mut(&number) {
            if let Remembered::One(first) = known
                && !first.as_str().eq_ignore_ascii_case(address.as_str())
            {
                *known = Remembered::Ambiguous;
            }
            return;
        }
        while self.order.len() >= MAX_REMEMBERED_ADDRESSES {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.by_number.remove(&oldest);
                }
                None => break,
            }
        }
        self.by_number
            .insert(number, Remembered::One(address.clone()));
        self.order.push_back(number);
    }

    fn get(&self, number: i64) -> Option<&CalendarAddress> {
        match self.by_number.get(&number)? {
            Remembered::One(address) => Some(address),
            Remembered::Ambiguous => None,
        }
    }
}

impl Directory {
    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fresh_listing(&self) -> Option<Vec<Principal>> {
        let cache = self.lock();
        cache
            .listing
            .as_ref()
            .filter(|listing| listing.at.elapsed() < LISTING_TTL)
            .map(|listing| listing.list.clone())
    }

    fn known(&self) -> Vec<Principal> {
        let cache = self.lock();
        let listed = cache.listing.iter().flat_map(|listing| listing.list.iter());
        listed.chain(cache.found.list.iter()).cloned().collect()
    }

    /// The principal numbered `number` in the listing or the searches
    /// (`own`, at this location, is not one of them), without copying the
    /// directory: what [`GroupwareAccount::known_principals`] would give.
    fn known_by_number(&self, number: i64, own: Option<&Href>) -> Option<Principal> {
        let cache = self.lock();
        let is_own = |p: &Principal| own.is_some_and(|own| own.same_location(&p.href));
        if let Some(listing) = &cache.listing
            && let Some(found) = listing
                .by_number
                .get(&number)
                .and_then(|at| listing.list.get(*at))
        {
            return (!is_own(found)).then(|| found.clone());
        }
        let listed = |href: &Href| {
            cache
                .listing
                .as_ref()
                .is_some_and(|l| l.locations.contains(&href.location_key()))
        };
        cache
            .found
            .list
            .iter()
            .find(|p| p.number() == number)
            .filter(|p| !is_own(p) && !listed(&p.href))
            .cloned()
    }

    fn remember(&self, principal: &Principal) {
        let mut cache = self.lock();
        if cache.found.contains(&principal.href) {
            return;
        }
        if cache.found.len() >= MAX_FOUND {
            cache.found = UniquePrincipals::default();
        }
        cache.found.add(principal.clone());
    }

    fn remember_address(&self, address: &CalendarAddress) {
        self.lock()
            .addresses
            .remember(address_number(address), address);
    }

    /// The number of the first principal in the listing or the searches
    /// with this address (lowercased), without copying the directory.
    fn number_with_address(&self, address: &str) -> Option<i64> {
        let cache = self.lock();
        let listed = cache.listing.iter().flat_map(|listing| listing.list.iter());
        listed
            .chain(cache.found.list.iter())
            .find(|p| {
                p.addresses
                    .iter()
                    .any(|a| a.as_str().eq_ignore_ascii_case(address))
            })
            .map(Principal::number)
    }
}

/// Errors that mean "this server does not offer that" rather than "try
/// again later": the answer is then just the user's own principal.
const fn not_offered(error: &DavError) -> bool {
    match error {
        DavError::NotFound
        | DavError::Forbidden { .. }
        | DavError::MethodNotAllowed
        | DavError::Conflict { .. }
        | DavError::Xml(_)
        | DavError::ContentType
        | DavError::Protocol(_)
        | DavError::OriginNotAllowed
        | DavError::Redirect(_)
        | DavError::TooMany
        | DavError::TooLarge
        | DavError::Encode(_) => true,
        DavError::Status { status, .. } => *status < 500,
        _ => false,
    }
}

/// A unique `UID` for a free-busy request.
fn request_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("nebula-fb-{nanos}-{count}")
}

impl GroupwareAccount {
    /// The user's own principal, once discovered.
    #[must_use]
    pub fn own_principal(&self) -> Option<Principal> {
        let found = self.discovered()?;
        Some(Principal {
            href: found.principal,
            name: found.principal_name,
            addresses: mailto_addresses(&found.user_addresses),
            kind: PrincipalKind::from_dav(found.principal_type),
            own: true,
        })
    }

    /// Whether `Principal/getAvailability` can work: the server schedules
    /// (`calendar-auto-schedule`) and discovery found an outbox.
    #[must_use]
    pub fn supports_free_busy(&self) -> bool {
        self.availability().calendars
            && self
                .discovered()
                .is_some_and(|found| found.auto_schedule && found.schedule_outbox.is_some())
    }

    /// Every principal the server shows (its listing), with the user's
    /// own first. Only the own one if the server does not list them.
    ///
    /// # Errors
    /// [`GroupwareError::NotDiscovered`] before discovery;
    /// [`GroupwareError::Unauthorized`]; [`GroupwareError::Dav`] when the
    /// server could not be reached (try again later).
    pub async fn principals(&self) -> Result<Vec<Principal>, GroupwareError> {
        let own = self.own_principal().ok_or(GroupwareError::NotDiscovered)?;
        let listed = self.principal_listing().await?;
        let mut all = UniquePrincipals::default();
        all.add(own);
        for principal in listed {
            all.add(principal);
        }
        Ok(all.into_vec())
    }

    /// A principal seen before (own, listed or found), by its number.
    /// An index lookup, not a scan of the directory.
    #[must_use]
    pub fn known_principal(&self, number: i64) -> Option<Principal> {
        let own = self.own_principal();
        if let Some(own) = &own
            && own.number() == number
        {
            return Some(own.clone());
        }
        self.directory
            .known_by_number(number, own.as_ref().map(|p| &p.href))
    }

    /// Every principal seen before (own first, then listed and found),
    /// without asking the server.
    #[must_use]
    pub fn known_principals(&self) -> Vec<Principal> {
        let mut all = UniquePrincipals::default();
        for principal in self
            .own_principal()
            .into_iter()
            .chain(self.directory.known())
        {
            all.add(principal);
        }
        all.into_vec()
    }

    /// Whether the server names sharees by address (Calendar Server
    /// sharing of sabre/dav and Apple, not Nextcloud's): then an address
    /// without a principal can still be shared with, and gets a number of
    /// its own ([`address_number`]). Baïkal shows no other principals at
    /// all, so this is the only way to choose someone there.
    #[must_use]
    pub fn shares_by_address(&self) -> bool {
        self.discovered().is_some_and(|found| {
            let support = found.sharing_for(CollectionKind::Calendar);
            support.calendar_server && !support.nextcloud
        })
    }

    /// The number of an address without a principal, remembered for
    /// [`Self::known_address`]; `None` where the server does not share by
    /// address.
    #[must_use]
    pub fn address_principal(&self, address: &CalendarAddress) -> Option<i64> {
        if !self.shares_by_address() {
            return None;
        }
        self.directory.remember_address(address);
        Some(address_number(address))
    }

    /// The address given this number before ([`Self::address_principal`],
    /// or a sharee read from the server), if any.
    #[must_use]
    pub fn known_address(&self, number: i64) -> Option<CalendarAddress> {
        self.directory.lock().addresses.get(number).cloned()
    }

    /// The principal numbers of stored sharees ([`Self::sharee_number`]
    /// of each), from one look at the directory: linear, however many.
    #[must_use]
    pub fn sharee_numbers(&self, shares: &[(Sharee, Rights)]) -> Vec<Option<i64>> {
        let mut by_address: HashMap<String, i64> = HashMap::new();
        for principal in self.known_principals() {
            for address in &principal.addresses {
                by_address
                    .entry(address.as_str().to_ascii_lowercase())
                    .or_insert_with(|| principal.number());
            }
        }
        shares
            .iter()
            .map(|(sharee, _)| {
                self.sharee_number_with(sharee, |address| {
                    by_address.get(&address.to_ascii_lowercase()).copied()
                })
            })
            .collect()
    }

    /// The principal number of a stored sharee: its location's, or for an
    /// address the number of the principal the server showed with it, or
    /// (on a server that shares by address) the address's own number.
    #[must_use]
    pub fn sharee_number(&self, sharee: &Sharee) -> Option<i64> {
        let own = self.own_principal();
        self.sharee_number_with(sharee, |address| {
            own.as_ref()
                .filter(|own| {
                    own.addresses
                        .iter()
                        .any(|a| a.as_str().eq_ignore_ascii_case(address))
                })
                .map(Principal::number)
                .or_else(|| self.directory.number_with_address(address))
        })
    }

    fn sharee_number_with(
        &self,
        sharee: &Sharee,
        principal_with: impl Fn(&str) -> Option<i64>,
    ) -> Option<i64> {
        let text = sharee.as_str();
        if text.starts_with("mailto:") {
            let address = CalendarAddress::parse(text)?;
            return principal_with(address.as_str()).or_else(|| self.address_principal(&address));
        }
        let url = url::Url::parse(text).ok()?;
        Href::from_url(&url).ok().map(|at| principal_number(&at))
    }

    /// The principal with this address, if the server shows one: the
    /// user's own, one seen in the last listing, or one found by a search
    /// (or a new listing, on servers that cannot search by address).
    ///
    /// # Errors
    /// As [`Self::principals`].
    pub async fn principal_for_address(
        &self,
        address: &CalendarAddress,
    ) -> Result<Option<Principal>, GroupwareError> {
        let own = self.own_principal().ok_or(GroupwareError::NotDiscovered)?;
        if own.has_address(address) {
            return Ok(Some(own));
        }
        let matching = |list: Vec<Principal>| list.into_iter().find(|p| p.has_address(address));
        if let Some(found) = self.directory.fresh_listing().and_then(matching) {
            return Ok(Some(found));
        }
        if let Some(found) = matching(self.directory.lock().found.list.clone()) {
            return Ok(Some(found));
        }
        if self.address_search().await? {
            let found = self.search_address(address).await?;
            if let Some(found) = &found {
                self.directory.remember(found);
            }
            return Ok(found);
        }
        Ok(matching(self.principal_listing().await?))
    }

    /// The busy times of `attendee` between `from` and `to` (seconds,
    /// UTC), asked with the user's address `organizer`. `None` when the
    /// server has none to give (not one of its users, nothing shared, no
    /// outbox).
    ///
    /// # Errors
    /// [`GroupwareError::Unauthorized`]; [`GroupwareError::Dav`] for the
    /// request (network, a hostile answer, bad times).
    pub async fn free_busy(
        &self,
        organizer: &CalendarAddress,
        attendee: &CalendarAddress,
        from: i64,
        to: i64,
    ) -> Result<Option<Vec<BusyPeriod>>, GroupwareError> {
        let Some(outbox) = self
            .discovered()
            .and_then(|found| found.schedule_outbox)
            .filter(|_| self.supports_free_busy())
        else {
            return Ok(None);
        };
        let request = FreeBusyRequest {
            uid: request_uid(),
            stamp: UnixTime::now()?.secs(),
            start: from,
            end: to,
            organizer: organizer.clone(),
            attendees: vec![attendee.clone()],
        };
        let answers = self.client.free_busy(&outbox, &request).await?;
        let Some(data) = answers
            .into_iter()
            .find(nebula_dav::FreeBusyAnswer::has_data)
            .and_then(|answer| answer.calendar_data)
        else {
            return Ok(None);
        };
        let parsed =
            tokio::task::spawn_blocking(move || freebusy::busy_periods(data.as_bytes(), from, to))
                .await
                .map_err(|_| GroupwareError::Task)?;
        match parsed {
            Ok(periods) => Ok(Some(periods)),
            Err(error) => {
                tracing::debug!(account = %self.id, %error, "free-busy reply does not parse");
                Ok(None)
            }
        }
    }

    /// Whether the server lets clients search principals by address
    /// (asked once per login; any failure counts as no).
    async fn address_search(&self) -> Result<bool, GroupwareError> {
        if let Some(known) = self.directory.lock().address_search {
            return Ok(known);
        }
        self.learn_search_properties().await?;
        Ok(self.directory.lock().address_search.unwrap_or(false))
    }

    /// Whether the server lets clients search principals by name (asked
    /// once per login; any failure counts as no).
    async fn name_search(&self) -> Result<bool, GroupwareError> {
        if let Some(known) = self.directory.lock().name_search {
            return Ok(known);
        }
        self.learn_search_properties().await?;
        Ok(self.directory.lock().name_search.unwrap_or(false))
    }

    async fn learn_search_properties(&self) -> Result<(), GroupwareError> {
        let mut by_address = false;
        let mut by_name = false;
        for collection in &self.principal_collections() {
            match self.client.principal_search_properties(collection).await {
                Ok(properties) => {
                    by_address |= properties.contains(&Property::CalendarUserAddressSet);
                    by_name |= properties.contains(&Property::DisplayName);
                }
                Err(error) if not_offered(&error) => {}
                Err(error) => return Err(error.into()),
            }
            if by_address && by_name {
                break;
            }
        }
        let mut cache = self.directory.lock();
        cache.address_search = Some(by_address);
        cache.name_search = Some(by_name);
        Ok(())
    }

    async fn search_address(
        &self,
        address: &CalendarAddress,
    ) -> Result<Option<Principal>, GroupwareError> {
        for collection in &self.principal_collections() {
            match self
                .client
                .search_principals(
                    collection,
                    Property::CalendarUserAddressSet,
                    address.address(),
                )
                .await
            {
                Ok(found) => {
                    // The server matches as it likes (substrings…): only
                    // an exact address counts.
                    let exact = found
                        .into_iter()
                        .map(|p| Principal::from_dav(p, false))
                        .find(|p| p.has_address(address));
                    if exact.is_some() {
                        return Ok(exact);
                    }
                }
                Err(error) if not_offered(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }

    /// The principals of the collections (a fresh listing at most every
    /// [`LISTING_TTL`]); empty where the server does not list them.
    async fn principal_listing(&self) -> Result<Vec<Principal>, GroupwareError> {
        if let Some(listed) = self.directory.fresh_listing() {
            return Ok(listed);
        }
        let own = self.own_principal().map(|p| p.href);
        let mut all = UniquePrincipals::default();
        let mut add = |found: Vec<DavPrincipal>| {
            for principal in found {
                let mine = own
                    .as_ref()
                    .is_some_and(|o| o.same_location(&principal.href));
                all.add(Principal::from_dav(principal, mine));
            }
        };
        let mut unlisted = Vec::new();
        for collection in &self.principal_collections() {
            match self.client.list_principals(collection).await {
                Ok(found) => add(found),
                // Not listed (Nextcloud's users: 405): searched instead,
                // if the server lets clients search names.
                Err(error) if not_offered(&error) => {
                    unlisted.push(collection.clone());
                    if !self.name_search().await? {
                        continue;
                    }
                    match self
                        .client
                        .search_principals(collection, Property::DisplayName, DIRECTORY_SEARCH)
                        .await
                    {
                        Ok(found) => add(found),
                        Err(error) if not_offered(&error) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let all = all.into_vec();
        let mut cache = self.directory.lock();
        cache.listing = Some(Listing::new(all.clone()));
        cache.unlisted = unlisted;
        drop(cache);
        Ok(all)
    }

    /// Principals whose name contains `text`, on a server that does not
    /// list some principal collection but lets clients search names
    /// (Nextcloud: its groups have no address, so the listing's search
    /// for `@` misses them). One `principal-property-search` of
    /// `displayname` per unlisted collection; what is found is remembered
    /// (for `Principal/get` and sharing). Empty elsewhere: a server that
    /// lists its principals already gave them all, and `text` must be
    /// 2 to [`MAX_NAME_SEARCH`] characters without controls.
    ///
    /// # Errors
    /// As [`Self::principals`].
    pub async fn search_by_name(&self, text: &str) -> Result<Vec<Principal>, GroupwareError> {
        let text = text.trim();
        let length = text.chars().count();
        if !(2..=MAX_NAME_SEARCH).contains(&length) || text.chars().any(char::is_control) {
            return Ok(Vec::new());
        }
        self.principal_listing().await?;
        let unlisted = self.directory.lock().unlisted.clone();
        if unlisted.is_empty() || !self.name_search().await? {
            return Ok(Vec::new());
        }
        let own = self.own_principal().map(|p| p.href);
        let mut out = UniquePrincipals::default();
        for collection in &unlisted {
            match self
                .client
                .search_principals(collection, Property::DisplayName, text)
                .await
            {
                Ok(found) => {
                    for principal in found {
                        let mine = own
                            .as_ref()
                            .is_some_and(|o| o.same_location(&principal.href));
                        out.add(Principal::from_dav(principal, mine));
                    }
                }
                Err(error) if not_offered(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let out = out.into_vec();
        for principal in out.iter().filter(|p| !p.own) {
            self.directory.remember(principal);
        }
        Ok(out)
    }

    fn principal_collections(&self) -> Vec<Href> {
        self.discovered()
            .map(|found| found.principal_collections)
            .unwrap_or_default()
    }
}

/// The time zones of principals, from what the DAV server says of their
/// calendars (`calendar-timezone-id`, else the `TZID` of
/// `calendar-timezone`, as each calendar's `timeZone`).
/// Nothing is asked for this: it comes from the local copy of the
/// calendars the server listed (allowed origins only). A zone calcard
/// does not know is never one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrincipalZones {
    /// The user's: the zone of their default calendar, else the one zone
    /// all their calendars with a zone agree on.
    pub own: Option<String>,
    /// Others' (principal number → zone): the one zone all the calendars
    /// they share with the user agree on.
    pub others: Vec<(i64, String)>,
}

impl PrincipalZones {
    /// The zone of the principal `number` (`own`: the user's).
    #[must_use]
    pub fn of(&self, number: i64, own: bool) -> Option<&str> {
        if own {
            return self.own.as_deref();
        }
        self.others
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, zone)| zone.as_str())
    }

    /// From each calendar's owner (`None`: the user), whether it is the
    /// default one and its zone.
    #[must_use]
    pub fn from_calendars<'a>(
        calendars: impl IntoIterator<Item = (Option<i64>, bool, Option<&'a str>)>,
    ) -> Self {
        // Owner → (zone of the default calendar, every zone seen).
        let mut seen: Vec<(Option<i64>, Option<String>, Vec<String>)> = Vec::new();
        for (owner, is_default, zone) in calendars {
            let Some(zone) = zone.and_then(Zone::named).map(Zone::iana_name) else {
                continue;
            };
            let at = if let Some(at) = seen.iter().position(|(o, _, _)| *o == owner) {
                at
            } else {
                seen.push((owner, None, Vec::new()));
                seen.len().saturating_sub(1)
            };
            let Some((_, default, zones)) = seen.get_mut(at) else {
                continue;
            };
            if is_default && owner.is_none() {
                *default = Some(zone.clone());
            }
            if !zones.contains(&zone) {
                zones.push(zone);
            }
        }
        let mut out = Self::default();
        for (owner, default, mut zones) in seen {
            let zone = default.or_else(|| (zones.len() == 1).then(|| zones.remove(0)));
            match (owner, zone) {
                (None, zone) => out.own = zone,
                (Some(number), Some(zone)) => out.others.push((number, zone)),
                (Some(_), None) => {}
            }
        }
        out
    }
}

impl GroupwareAccount {
    /// The principals' time zones (see [`PrincipalZones`]).
    ///
    /// # Errors
    /// [`GroupwareError::Store`] on database errors.
    pub async fn principal_time_zones(&self) -> Result<PrincipalZones, GroupwareError> {
        let own = self.own_principal().map(|p| p.number());
        let collections = self.store().dav_collections(self.id).await?;
        Ok(PrincipalZones::from_calendars(
            collections
                .iter()
                .filter(|c| c.props.kind == CollectionKind::Calendar && c.orphaned_at.is_none())
                .map(|c| {
                    let owner = c
                        .props
                        .sharing
                        .owner
                        .as_ref()
                        .map(crate::sharing::location_number)
                        .map(|number| number.filter(|n| Some(*n) != own));
                    (owner, c.is_default, c.props.time_zone.as_deref())
                })
                // An owner whose location does not read is no one.
                .filter_map(|(owner, is_default, zone)| match owner {
                    None => Some((None, is_default, zone)),
                    Some(None) => None,
                    Some(Some(number)) => Some((Some(number), is_default, zone)),
                }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_mailto_only() {
        let set = [
            "mailto:alice@example.com".to_owned(),
            "/principals/alice@example.com/".to_owned(),
            "MAILTO:Alice@Example.com".to_owned(),
            "mailto:alice.smith@example.com".to_owned(),
            "urn:uuid:1234".to_owned(),
            "mailto:bad address@example.com".to_owned(),
        ];
        let addresses: Vec<String> = mailto_addresses(&set)
            .iter()
            .map(|a| a.as_str().to_owned())
            .collect();
        assert_eq!(
            addresses,
            ["mailto:alice@example.com", "mailto:alice.smith@example.com"]
        );
    }

    #[test]
    fn numbers_are_stable_and_case_sensitive() {
        let base = url::Url::parse("https://dav.example.com/").unwrap();
        let base = Href::from_url(&base).unwrap();
        let bob = Href::resolve("/principals/bob/", &base).unwrap();
        let bob_upper = Href::resolve("/principals/Bob/", &base).unwrap();
        assert_eq!(principal_number(&bob), principal_number(&bob.clone()));
        assert_ne!(principal_number(&bob), principal_number(&bob_upper));
        assert!(principal_number(&bob) > 0);
    }

    #[test]
    fn principal_zones() {
        // The user's default calendar wins; another zone does not matter.
        let zones = PrincipalZones::from_calendars([
            (None, false, Some("America/New_York")),
            (None, true, Some("Europe/Madrid")),
            (Some(7), false, Some("Asia/Tokyo")),
            (Some(7), false, Some("Asia/Tokyo")),
            (Some(8), false, Some("Asia/Tokyo")),
            (Some(8), false, Some("Europe/Paris")),
            (Some(9), false, Some("Not/AZone")),
            (Some(10), true, None),
        ]);
        assert_eq!(zones.of(1, true), Some("Europe/Madrid"));
        assert_eq!(zones.of(7, false), Some("Asia/Tokyo"));
        // Two zones: unknown. An unknown zone or none: unknown.
        assert_eq!(zones.of(8, false), None);
        assert_eq!(zones.of(9, false), None);
        assert_eq!(zones.of(10, false), None);
        // Without a default with a zone: the one zone, else none.
        let zones = PrincipalZones::from_calendars([
            (None, true, None),
            (None, false, Some("Europe/Madrid")),
            (None, false, None),
        ]);
        assert_eq!(zones.own.as_deref(), Some("Europe/Madrid"));
        let zones = PrincipalZones::from_calendars([
            (None, false, Some("Europe/Madrid")),
            (None, false, Some("Europe/Paris")),
        ]);
        assert_eq!(zones.own, None);
        assert_eq!(
            PrincipalZones::from_calendars([]),
            PrincipalZones::default()
        );
        // A shared calendar's default flag says nothing of its owner.
        let zones = PrincipalZones::from_calendars([
            (Some(7), true, Some("Asia/Tokyo")),
            (Some(7), false, Some("Europe/Paris")),
        ]);
        assert_eq!(zones.of(7, false), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(PrincipalKind::from_dav(None), PrincipalKind::Individual);
        assert_eq!(
            PrincipalKind::from_dav(Some(CalendarUserType::Room)),
            PrincipalKind::Location
        );
        assert_eq!(
            PrincipalKind::from_dav(Some(CalendarUserType::Unknown)),
            PrincipalKind::Other
        );
    }

    fn address(n: usize) -> CalendarAddress {
        CalendarAddress::parse(&format!("mailto:u{n}@example.com")).unwrap()
    }

    /// 1500 sharees by address (more than the searches keep) all keep
    /// their ids; past the limit the oldest go first, not all at once.
    #[test]
    fn many_addresses_stay_resolvable() {
        let directory = Directory::default();
        for n in 0..1500 {
            directory.remember_address(&address(n));
        }
        let cache = directory.lock();
        for n in 0..1500 {
            let a = address(n);
            assert_eq!(cache.addresses.get(address_number(&a)), Some(&a));
        }
        drop(cache);
        for n in 1500..=MAX_REMEMBERED_ADDRESSES {
            directory.remember_address(&address(n));
        }
        let cache = directory.lock();
        assert_eq!(cache.addresses.get(address_number(&address(0))), None);
        let last = address(MAX_REMEMBERED_ADDRESSES);
        assert_eq!(cache.addresses.get(address_number(&last)), Some(&last));
        assert_eq!(
            cache.addresses.get(address_number(&address(1))),
            Some(&address(1))
        );
    }

    /// Two addresses with one number (FNV collisions can be made): the
    /// second never takes the first's id; neither resolves.
    #[test]
    fn colliding_address_numbers_resolve_to_nobody() {
        let mut book = Addresses::default();
        let (bob, eve) = (address(1), address(2));
        book.remember(7, &bob);
        assert_eq!(book.get(7), Some(&bob));
        // The same address in another case is still bob.
        let upper = CalendarAddress::parse("mailto:U1@EXAMPLE.COM").unwrap();
        book.remember(7, &upper);
        assert_eq!(book.get(7), Some(&bob));
        book.remember(7, &eve);
        assert_eq!(book.get(7), None);
        book.remember(7, &bob);
        assert_eq!(book.get(7), None);
    }
}
