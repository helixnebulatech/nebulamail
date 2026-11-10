// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sync of one collection.
//!
//! - With `sync-collection` (RFC 6578): from the stored token; the DAV
//!   client follows 507 continuations and starts over (a full listing) if
//!   the token is no longer valid.
//! - Without it: the ctag of the home listing says whether anything
//!   changed, and a PROPFIND Depth 1 lists every href with its etag.
//! - Only members whose etag differs from the stored one are fetched,
//!   with multiget in batches of 100, and applied batch by batch (each
//!   batch is pushed at once). A member without an etag is fetched and
//!   compared with the stored text, so it is no change if it is the same.
//! - A full listing means "everything that is not here is gone".
//! - **Stalwart's quirk**: a token it does not
//!   know gets every member back as if they had all changed, with no
//!   error, so it looks like an incremental answer that misses the
//!   deletions. Comparing etags already avoids spurious changes; and an
//!   incremental answer that names a member we have with the very same
//!   etag (which a real one never does) is checked with a full listing,
//!   so the deletions are found too.
//! - The new token and ctag are saved only when everything is applied: an
//!   interrupted sync starts again from the old ones. Also not while a
//!   member asked for is unresolved (no answer, a 500…): the next round
//!   asks again. After [`MAX_UNRESOLVED_ROUNDS`] rounds in a row, and at
//!   once for one too large (507) or forbidden (403), the member is kept
//!   as unreadable (hidden) with its listed etag, and asked for again only
//!   when that changes.
//! - Only an explicit 404 or 410 of a member means deleted meanwhile.
//! - A fetched member with the same text as stored is no change, unless
//!   its indexes come out different now (the indexing got better: a
//!   migration forgets the etags of what must be indexed again).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError};

use nebula_crypto::AccountKey;
use nebula_dav::proto::{Href, Status, SyncToken, VcardRequest};
use nebula_dav::{Collection as RemoteCollection, Member, ResourceKind};
use nebula_store::{
    Collection, CollectionId, DavHref, Parsed, Recorded, ResourceWrite, StoredResource, UnixTime,
};
use secrecy::ExposeSecret as _;

use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::index::{resource_parsed, resource_write, stored_href};

/// Whether the home listing shows changes since the last complete sync.
pub(crate) fn needs_sync(stored: &Collection, remote: &RemoteCollection) -> bool {
    let sync = &stored.sync;
    if sync.synced_at.is_none() {
        return true;
    }
    if remote.supports_sync()
        && let Some(token) = &remote.sync_token
    {
        return sync.sync_token.as_deref() != Some(token.as_str());
    }
    match &remote.ctag {
        Some(ctag) => sync.ctag.as_deref() != Some(ctag.as_str()),
        // Nothing to compare: look every time.
        None => true,
    }
}

/// What the server listed, by stored href.
struct Listing {
    members: Vec<Member>,
    removed: Vec<Href>,
    full: bool,
    token: Option<SyncToken>,
}

/// An incremental answer that names a member we already have with the
/// same etag: what Stalwart sends for a token it does not know. Members
/// written after the last complete sync (in a later second) don't count:
/// the bridge's own writes (and batches of an interrupted sync) are stored
/// with their new etag before the server lists them as changed.
fn looks_like_everything(
    members: &[Member],
    stored: &HashMap<DavHref, StoredResource>,
    synced_at: Option<UnixTime>,
) -> bool {
    let Some(synced_at) = synced_at else {
        return false;
    };
    members.iter().any(|member| {
        let Some(etag) = &member.etag else {
            return false;
        };
        stored_href(&member.href)
            .ok()
            .and_then(|href| stored.get(&href))
            .is_some_and(|known| {
                known.orphaned_at.is_none()
                    && known.updated_at <= synced_at
                    && known.etag.as_deref() == Some(etag.as_str())
            })
    })
}

/// A member to fetch: unknown, hidden, or with another (or no) etag.
fn must_fetch(member: &Member, stored: &HashMap<DavHref, StoredResource>) -> bool {
    let Ok(href) = stored_href(&member.href) else {
        return false;
    };
    match (stored.get(&href), &member.etag) {
        (Some(known), Some(etag)) => {
            known.orphaned_at.is_some() || known.etag.as_deref() != Some(etag.as_str())
        }
        _ => true,
    }
}

/// One fetched resource, before the blocking work.
struct Fetched {
    href: Href,
    etag: Option<String>,
    data: String,
    /// The stored sealed text of a resource we have, its etag and its
    /// indexes: compared, so the same text (indexed the same) under a new
    /// (or no) etag is no change.
    previous: Option<(Vec<u8>, Option<String>, Parsed)>,
}

/// A member asked for that did not come with its text.
struct Missing {
    href: Href,
    stored: DavHref,
    /// The etag the server gave or listed for it.
    etag: Option<String>,
}

/// Rounds in a row a member may stay unresolved before it is kept as
/// unreadable (`Parsed::Invalid`) with its listed etag, so that a server
/// that always leaves one out does not hold the token back for ever.
pub(crate) const MAX_UNRESOLVED_ROUNDS: u8 = 3;

/// What the blocking work found.
#[derive(Default)]
struct Converted {
    writes: Vec<ResourceWrite>,
    /// Same text, another etag: only the etag is stored.
    etags: Vec<(DavHref, Option<String>)>,
}

fn same_text(
    key: &AccountKey,
    collection: CollectionId,
    href: &DavHref,
    previous: &[u8],
    data: &str,
) -> bool {
    crate::model::sealed::open(key, collection, href, previous)
        .is_ok_and(|text| text.expose_secret() == data.as_bytes())
}

impl GroupwareAccount {
    async fn listing(
        &self,
        stored: &Collection,
        remote: &RemoteCollection,
        known: &HashMap<DavHref, StoredResource>,
        reconcile: bool,
    ) -> Result<Listing, GroupwareError> {
        let href = &remote.href;
        if !remote.supports_sync() {
            return Ok(Listing {
                members: self
                    .client_for(stored.props.kind)
                    .list_members(href)
                    .await?,
                removed: Vec::new(),
                full: true,
                token: remote.sync_token.clone(),
            });
        }
        let from = if reconcile {
            None
        } else {
            stored.sync.sync_token.as_deref().and_then(SyncToken::parse)
        };
        let result = self
            .client_for(stored.props.kind)
            .sync(href, from.as_ref())
            .await?;
        if !result.full && looks_like_everything(&result.changed, known, stored.sync.synced_at) {
            tracing::debug!(account = %self.id, "dav: sync answer lists unchanged members; listing all");
            return Ok(Listing {
                members: self
                    .client_for(stored.props.kind)
                    .list_members(href)
                    .await?,
                removed: Vec::new(),
                full: true,
                token: Some(result.token),
            });
        }
        Ok(Listing {
            members: result.changed,
            removed: result.removed,
            full: result.full,
            token: Some(result.token),
        })
    }

    /// Syncs one collection (see the module docs).
    pub(crate) async fn sync_collection(
        &self,
        stored: &Collection,
        remote: &RemoteCollection,
        reconcile: bool,
    ) -> Result<Recorded, GroupwareError> {
        let known: HashMap<DavHref, StoredResource> = self
            .store()
            .dav_resources(self.id, stored.id)
            .await?
            .into_iter()
            .map(|resource| (resource.href.clone(), resource))
            .collect();
        let listing = self.listing(stored, remote, &known, reconcile).await?;
        let mut recorded = Recorded::default();

        // Removals first: a resource renamed by another client then keeps
        // its id (same UID, hidden a moment before).
        let mut removed: Vec<DavHref> = listing
            .removed
            .iter()
            .filter_map(|href| stored_href(href).ok())
            .collect();
        if listing.full {
            let present: HashSet<DavHref> = listing
                .members
                .iter()
                .filter_map(|member| stored_href(&member.href).ok())
                .collect();
            removed.extend(
                known
                    .values()
                    .filter(|r| r.orphaned_at.is_none() && !present.contains(&r.href))
                    .map(|r| r.href.clone()),
            );
        }
        if !removed.is_empty() {
            let done = self
                .store()
                .apply_resources(self.id, stored.id, Vec::new(), removed, UnixTime::now()?)
                .await?;
            self.emit(&done);
            recorded.merge(done);
        }

        let wanted: Vec<&Member> = listing
            .members
            .iter()
            .filter(|member| must_fetch(member, &known))
            .collect();
        let kind = remote.kind.resource();
        let batch = self.config().server.limits.multiget_batch.max(1);
        let mut unresolved = Vec::new();
        for members in wanted.chunks(batch) {
            let (done, mut missing) = self
                .fetch_batch(stored, remote, kind, members, &known)
                .await?;
            self.emit(&done);
            recorded.merge(done);
            unresolved.append(&mut missing);
        }
        let given_up = self.count_unresolved(stored.id, &mut unresolved);
        if !given_up.is_empty() {
            tracing::info!(
                account = %self.id,
                count = given_up.len(),
                "dav: members never answered, kept as unreadable"
            );
            let fetched = self.markers(given_up, &known).await?;
            let done = self
                .apply_fetched(stored, kind, fetched, Vec::new())
                .await?;
            self.emit(&done);
            recorded.merge(done);
        }
        if !unresolved.is_empty() {
            // The next round starts from the old token and fetches what is
            // missing (the rest has its etag stored now).
            tracing::debug!(
                account = %self.id,
                count = unresolved.len(),
                "dav: members not answered, sync token not saved"
            );
            return Ok(recorded);
        }

        self.store()
            .set_collection_synced(
                self.id,
                stored.id,
                listing.token.map(|token| token.as_str().to_owned()),
                remote.ctag.as_ref().map(|ctag| ctag.as_str().to_owned()),
                UnixTime::now()?,
            )
            .await?;
        Ok(recorded)
    }

    /// Counts a round for each member left unresolved in `collection`
    /// (forgetting the others) and takes out of `unresolved` those that
    /// reached [`MAX_UNRESOLVED_ROUNDS`]: they are given up on.
    fn count_unresolved(
        &self,
        collection: CollectionId,
        unresolved: &mut Vec<Missing>,
    ) -> Vec<Missing> {
        let mut rounds = self
            .unresolved
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before: HashMap<DavHref, u8> = rounds
            .extract_if(|(coll, _), _| *coll == collection)
            .map(|((_, href), count)| (href, count))
            .collect();
        let mut given_up = Vec::new();
        let mut kept = Vec::with_capacity(unresolved.len());
        for missing in unresolved.drain(..) {
            let count = before
                .get(&missing.stored)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if count >= MAX_UNRESOLVED_ROUNDS {
                given_up.push(missing);
            } else {
                rounds.insert((collection, missing.stored.clone()), count);
                kept.push(missing);
            }
        }
        *unresolved = kept;
        given_up
    }

    /// The stored text and indexes of `href`, if we have it visible.
    async fn previous(
        &self,
        href: &DavHref,
        known: &HashMap<DavHref, StoredResource>,
    ) -> Result<Option<(Vec<u8>, Option<String>, Parsed)>, GroupwareError> {
        Ok(match known.get(href) {
            Some(old) if old.orphaned_at.is_none() => self
                .store()
                .dav_resource_data(self.id, old.id)
                .await?
                .map(|sealed| (sealed, old.etag.clone(), old.parsed.clone())),
            _ => None,
        })
    }

    /// Markers for members that cannot be read (too large, forbidden, or
    /// never answered): an empty text, so `Parsed::Invalid` (kept but not
    /// shown), with the etag listed, so they are not asked for again until
    /// they change.
    async fn markers(
        &self,
        members: Vec<Missing>,
        known: &HashMap<DavHref, StoredResource>,
    ) -> Result<Vec<Fetched>, GroupwareError> {
        let mut out = Vec::with_capacity(members.len());
        for member in members {
            out.push(Fetched {
                previous: self.previous(&member.stored, known).await?,
                href: member.href,
                etag: member.etag,
                data: String::new(),
            });
        }
        Ok(out)
    }

    /// One multiget; what it applied, and the members left unresolved.
    async fn fetch_batch(
        &self,
        stored: &Collection,
        remote: &RemoteCollection,
        kind: ResourceKind,
        members: &[&Member],
        known: &HashMap<DavHref, StoredResource>,
    ) -> Result<(Recorded, Vec<Missing>), GroupwareError> {
        let hrefs: Vec<Href> = members.iter().map(|member| member.href.clone()).collect();
        let listed_etags: HashMap<&Href, &str> = members
            .iter()
            .filter_map(|member| Some((&member.href, member.etag.as_ref()?.as_str())))
            .collect();
        let listed = |href: &Href| listed_etags.get(href).map(|e| (*e).to_owned());
        // vCard 4.0 when the book lists it: Nextcloud keeps a 4.0 card as
        // 3.0 (groups as `X-ADDRESSBOOKSERVER-*`) unless 4.0 is asked for.
        let wanted =
            VcardRequest::for_supported(remote.address_data.as_deref().unwrap_or_default());
        let answers = self
            .client_for(stored.props.kind)
            .multiget_as(&remote.href, kind, &hrefs, wanted)
            .await?;
        // Compared as canonical hrefs: an answer may spell one otherwise.
        let mut not_answered: HashSet<&Href> = hrefs.iter().collect();
        let mut gone = Vec::new();
        let mut fetched = Vec::new();
        let mut unreadable = Vec::new();
        let mut unresolved = Vec::new();
        for answer in answers {
            not_answered.remove(&answer.href);
            let Ok(href) = stored_href(&answer.href) else {
                // Never stored: nothing to resolve.
                continue;
            };
            let etag = answer
                .etag
                .map(|etag| etag.as_str().to_owned())
                .or_else(|| listed(&answer.href));
            let Some(data) = answer.data else {
                let missing = Missing {
                    href: answer.href,
                    stored: href.clone(),
                    etag,
                };
                match answer.status {
                    // Deleted meanwhile (only an explicit 404 or 410).
                    Some(Status::NOT_FOUND | Status::GONE) => gone.push(href),
                    // Too large, or not readable: that does not change by
                    // itself; asked for again when the etag changes.
                    Some(Status::INSUFFICIENT_STORAGE | Status::FORBIDDEN) => {
                        unreadable.push(missing);
                    }
                    // Anything else may pass (a 500, no status): the token
                    // waits, and the member is asked for again.
                    _ => unresolved.push(missing),
                }
                continue;
            };
            fetched.push(Fetched {
                previous: self.previous(&href, known).await?,
                href: answer.href,
                etag,
                data,
            });
        }
        for href in not_answered {
            if let Ok(stored) = stored_href(href) {
                unresolved.push(Missing {
                    href: href.clone(),
                    stored,
                    etag: listed(href),
                });
            }
        }
        if !unreadable.is_empty() {
            tracing::info!(
                account = %self.id,
                count = unreadable.len(),
                "dav: members too large or not readable, kept as unreadable"
            );
            fetched.append(&mut self.markers(unreadable, known).await?);
        }
        let done = self.apply_fetched(stored, kind, fetched, gone).await?;
        Ok((done, unresolved))
    }

    /// Indexes and stores fetched members (off the async threads) and
    /// hides the gone ones.
    async fn apply_fetched(
        &self,
        stored: &Collection,
        kind: ResourceKind,
        fetched: Vec<Fetched>,
        gone: Vec<DavHref>,
    ) -> Result<Recorded, GroupwareError> {
        let key = Arc::clone(&self.key);
        let max_occurrences = self.config().max_occurrences;
        let collection = stored.id;
        let converted = tokio::task::spawn_blocking(move || {
            let mut out = Converted::default();
            for item in fetched {
                let href = stored_href(&item.href)?;
                if let Some((sealed, old_etag, old_parsed)) = &item.previous
                    && same_text(&key, collection, &href, sealed, &item.data)
                    && resource_parsed(kind, &item.data, max_occurrences) == *old_parsed
                {
                    if *old_etag != item.etag {
                        out.etags.push((href, item.etag));
                    }
                    continue;
                }
                out.writes.push(resource_write(
                    &key,
                    collection,
                    kind,
                    &item.href,
                    item.etag,
                    &item.data,
                    max_occurrences,
                )?);
            }
            Ok::<_, GroupwareError>(out)
        })
        .await
        .map_err(|_| GroupwareError::Task)??;
        if !converted.etags.is_empty() {
            self.store()
                .set_resource_etags(self.id, stored.id, converted.etags)
                .await?;
        }
        if converted.writes.is_empty() && gone.is_empty() {
            return Ok(Recorded::default());
        }
        Ok(self
            .store()
            .apply_resources(self.id, stored.id, converted.writes, gone, UnixTime::now()?)
            .await?)
    }
}
