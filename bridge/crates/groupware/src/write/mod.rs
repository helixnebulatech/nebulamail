// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The bridge's own writes to the DAV server (JMAP `/set`, steps
//! 4.3–4.5), each one reflected in the store at once:
//!
//! - **Resources**: PUT with `If-None-Match: *` (new) or `If-Match`
//!   (change), DELETE and MOVE with `If-Match`: never unconditional.
//!   `If-Match` takes only a strong etag (RFC 9110
//!   §13.1.1: a weak one never matches); without one stored, the resource
//!   is read again and must still be what the local copy says. The new
//!   text is stored sealed for its place with the etag the server gave
//!   only if that etag is strong (RFC 4791 §5.3.4: then the server stored
//!   our bytes as they are); with a weak etag or none, as a GET returns it.
//!   Then the account is [kicked](GroupwareAccount::kick), so the next
//!   round sees what the server did with it.
//! - **A 412** is [`GroupwareError::is_conflict`]: the caller reads the
//!   resource again with [`GroupwareAccount::refetch`] and retries once.
//! - **Collections**: extended MKCOL, PROPPATCH and DELETE, followed by a
//!   sync round, which lists the homes again and records the changes.
//!
//! What goes into the texts (vCard, iCalendar) is the JMAP layer's job;
//! hrefs only come from the store or from the collection plus a name, so
//! nothing here writes outside the account's collections.

use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{Component, ComponentSet, ETag, Href, Property, ResourceName};
use nebula_dav::{CollectionKind as DavKind, DavError, PutCondition, ResourceKind};
use nebula_store::{
    Collection, CollectionKind, DavHref, Recorded, ResourceId, ResourceWrite, StoredResource,
    UnixTime,
};
use secrecy::ExposeSecret as _;
use url::Url;

use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::index::{resource_write, stored_href, vcard_request};

/// What [`GroupwareAccount::delete_collection`] may take with the
/// collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contents {
    /// Nothing: the server must list no member (JMAP without
    /// `onDestroyRemove…`).
    MustBeEmpty,
    /// Everything in it.
    Remove,
}

/// A resource and its text, as the local copy has it now.
#[derive(Clone)]
pub struct ResourceText {
    /// The resource.
    pub resource: StoredResource,
    /// Its iCalendar or vCard text.
    pub text: String,
}

impl std::fmt::Debug for ResourceText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceText")
            .field("resource", &self.resource.id)
            .finish_non_exhaustive()
    }
}

/// The DAV form of a stored href (always one the DAV client gave).
pub(crate) fn href(stored: &DavHref) -> Result<Href, GroupwareError> {
    let url = Url::parse(stored.as_str()).map_err(|_| GroupwareError::NotFound)?;
    Href::from_url(&url).map_err(|error| GroupwareError::Dav(DavError::Href(error)))
}

const fn resource_kind(kind: CollectionKind) -> ResourceKind {
    match kind {
        CollectionKind::Calendar => ResourceKind::Calendar,
        CollectionKind::AddressBook => ResourceKind::Contact,
    }
}

const fn dav_kind(kind: CollectionKind) -> DavKind {
    match kind {
        CollectionKind::Calendar => DavKind::Calendar,
        CollectionKind::AddressBook => DavKind::Addressbook,
    }
}

fn etag_text(etag: Option<ETag>) -> Option<String> {
    etag.map(|etag| etag.as_str().to_owned())
}

impl GroupwareAccount {
    /// A live collection of the account.
    pub(crate) async fn live_collection(
        &self,
        id: nebula_store::CollectionId,
    ) -> Result<Collection, GroupwareError> {
        self.store()
            .dav_collections(self.id)
            .await?
            .into_iter()
            .find(|c| c.id == id && c.orphaned_at.is_none())
            .ok_or(GroupwareError::NotFound)
    }

    /// Seals and indexes `text` for `href` in `collection` (off the async
    /// threads).
    async fn prepare(
        &self,
        collection: &Collection,
        href: Href,
        etag: Option<String>,
        text: String,
    ) -> Result<ResourceWrite, GroupwareError> {
        let key = std::sync::Arc::clone(&self.key);
        let kind = resource_kind(collection.props.kind);
        let id = collection.id;
        let max_occurrences = self.config().max_occurrences;
        tokio::task::spawn_blocking(move || {
            resource_write(&key, id, kind, &href, etag, &text, max_occurrences)
        })
        .await
        .map_err(|_| GroupwareError::Task)?
    }

    /// Records a write and tells the listeners and the live task.
    fn written(&self, recorded: &Recorded) {
        self.emit(recorded);
        self.kick();
    }

    /// The text of a resource of the account from the local copy (`None`
    /// if there is no such resource, or it left the server).
    ///
    /// # Errors
    /// [`GroupwareError::Unreadable`] if the copy does not open (read it
    /// again with [`Self::refetch`]); store errors.
    pub async fn resource_text(
        &self,
        id: ResourceId,
    ) -> Result<Option<ResourceText>, GroupwareError> {
        let Some(resource) = self.store().dav_resource(self.id, id).await? else {
            return Ok(None);
        };
        if resource.orphaned_at.is_some() {
            return Ok(None);
        }
        let Some(sealed) = self.store().dav_resource_data(self.id, id).await? else {
            return Ok(None);
        };
        let key = std::sync::Arc::clone(&self.key);
        let (collection, stored) = (resource.collection, resource.href.clone());
        let text = tokio::task::spawn_blocking(move || {
            crate::model::sealed::open(&key, collection, &stored, &sealed)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes.expose_secret().to_vec()).ok())
        })
        .await
        .map_err(|_| GroupwareError::Task)?
        .ok_or(GroupwareError::Unreadable)?;
        Ok(Some(ResourceText { resource, text }))
    }

    /// Reads a resource again from the server (after a 412, or when the
    /// local copy is unreadable) and stores it. `None` if it is gone
    /// (then it is hidden in the store too).
    ///
    /// # Errors
    /// [`GroupwareError::Dav`] for the request; store errors.
    pub async fn refetch(
        &self,
        resource: &StoredResource,
    ) -> Result<Option<ResourceText>, GroupwareError> {
        let collection = self.live_collection(resource.collection).await?;
        let at = href(&resource.href)?;
        let kind = resource_kind(collection.props.kind);
        let now = UnixTime::now()?;
        let wanted = vcard_request(collection.props.vcard_versions);
        match self
            .client_for(collection.props.kind)
            .get_as(&at, kind, wanted)
            .await
        {
            Ok(fetched) => {
                let text = fetched.data;
                let write = self
                    .prepare(&collection, at, etag_text(fetched.etag), text.clone())
                    .await?;
                let recorded = self
                    .store()
                    .apply_resources(self.id, collection.id, vec![write], Vec::new(), now)
                    .await?;
                self.emit(&recorded);
                let resource = self
                    .store()
                    .dav_resource_at(self.id, collection.id, resource.href.clone())
                    .await?
                    .ok_or(GroupwareError::NotFound)?;
                Ok(Some(ResourceText { resource, text }))
            }
            Err(DavError::NotFound) => {
                let recorded = self
                    .store()
                    .apply_resources(
                        self.id,
                        collection.id,
                        Vec::new(),
                        vec![resource.href.clone()],
                        now,
                    )
                    .await?;
                self.emit(&recorded);
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Stores what the server has at `at` after our write: `text` with
    /// `etag` if it is strong, else what a GET returns. RFC 4791 §5.3.4
    /// (and RFC 6352 §6.3.2.3): a server that changes what it stores
    /// (sabre adds a `PRODID`, Stalwart may fix an object) must not give a
    /// strong etag for it, so only a strong one says our bytes are there.
    async fn store_written(
        &self,
        collection: &Collection,
        at: Href,
        etag: Option<ETag>,
        text: String,
    ) -> Result<StoredResource, GroupwareError> {
        let (etag, text) = if let Some(etag) = etag.filter(|etag| !etag.is_weak()) {
            (Some(etag), text)
        } else {
            let fetched = self
                .client_for(collection.props.kind)
                .get_as(
                    &at,
                    resource_kind(collection.props.kind),
                    vcard_request(collection.props.vcard_versions),
                )
                .await?;
            (fetched.etag, fetched.data)
        };
        let stored = stored_href(&at)?;
        let write = self.prepare(collection, at, etag_text(etag), text).await?;
        let recorded = self
            .store()
            .apply_resources(
                self.id,
                collection.id,
                vec![write],
                Vec::new(),
                UnixTime::now()?,
            )
            .await?;
        self.written(&recorded);
        self.store()
            .dav_resource_at(self.id, collection.id, stored)
            .await?
            .ok_or(GroupwareError::NotFound)
    }

    /// Creates `name` in a live collection with `text` (PUT with
    /// `If-None-Match: *`).
    ///
    /// # Errors
    /// [`GroupwareError::NotFound`] for a collection that is not live;
    /// [`GroupwareError::Dav`] with the server's refusal (412: the name is
    /// taken; 403 with a precondition such as `no-uid-conflict`; too
    /// large…).
    pub async fn create_resource(
        &self,
        collection: nebula_store::CollectionId,
        name: &ResourceName,
        text: String,
    ) -> Result<StoredResource, GroupwareError> {
        let collection = self.live_collection(collection).await?;
        let at = href(&collection.props.href)?
            .as_collection()
            .member(name, false)
            .map_err(DavError::Href)?;
        let kind = resource_kind(collection.props.kind);
        let etag = self
            .client_for(collection.props.kind)
            .put(&at, kind, &text, &PutCondition::Create)
            .await?;
        self.store_written(&collection, at, etag, text).await
    }

    /// The etag to make a change conditional on: the stored one if it is
    /// strong (`If-Match` compares strongly, RFC 9110 §13.1.1), else the
    /// server's after a GET, if the resource there is still what the local
    /// copy (the text the change was made on) says; if not, a conflict, so
    /// the caller patches the copy just read. A server that only gives weak
    /// etags gets [`GroupwareError::NoEtag`]: nothing is written without a
    /// condition (`If-Unmodified-Since` is no substitute: one-second
    /// resolution, and CalDAV servers need not keep `Last-Modified`).
    async fn etag_for(&self, resource: &StoredResource) -> Result<ETag, GroupwareError> {
        let strong = |etag: Option<&str>| etag.and_then(ETag::parse).filter(|e| !e.is_weak());
        if let Some(etag) = strong(resource.etag.as_deref()) {
            return Ok(etag);
        }
        let local = match self.resource_text(resource.id).await {
            Ok(local) => local.map(|local| local.text),
            Err(GroupwareError::Unreadable) => None,
            Err(error) => return Err(error),
        };
        let fresh = self
            .refetch(resource)
            .await?
            .ok_or(GroupwareError::NotFound)?;
        let etag = strong(fresh.resource.etag.as_deref()).ok_or(GroupwareError::NoEtag)?;
        if local.as_deref() != Some(fresh.text.as_str()) {
            return Err(GroupwareError::Dav(DavError::PreconditionFailed));
        }
        Ok(etag)
    }

    /// Replaces the text of a resource (PUT with `If-Match` its etag).
    ///
    /// # Errors
    /// A conflict ([`GroupwareError::is_conflict`]) if it changed on the
    /// server; [`GroupwareError::NoEtag`]; the server's refusal.
    pub async fn replace_resource(
        &self,
        resource: &StoredResource,
        text: String,
    ) -> Result<StoredResource, GroupwareError> {
        let collection = self.live_collection(resource.collection).await?;
        let at = href(&resource.href)?;
        let etag = self.etag_for(resource).await?;
        let kind = resource_kind(collection.props.kind);
        let new_etag = self
            .client_for(collection.props.kind)
            .put(&at, kind, &text, &PutCondition::Update(etag))
            .await?;
        self.store_written(&collection, at, new_etag, text).await
    }

    /// Deletes a resource (DELETE with `If-Match` its etag). One already
    /// gone counts as deleted.
    ///
    /// # Errors
    /// A conflict if it changed on the server; the server's refusal.
    pub async fn delete_resource(&self, resource: &StoredResource) -> Result<(), GroupwareError> {
        let collection = self.live_collection(resource.collection).await?;
        let at = href(&resource.href)?;
        let etag = match self.etag_for(resource).await {
            Ok(etag) => Some(etag),
            Err(GroupwareError::NotFound) => None,
            Err(error) => return Err(error),
        };
        if let Some(etag) = &etag {
            match self
                .client_for(collection.props.kind)
                .delete(&at, Some(etag))
                .await
            {
                Ok(()) | Err(DavError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let recorded = self
            .store()
            .apply_resources(
                self.id,
                collection.id,
                Vec::new(),
                vec![resource.href.clone()],
                UnixTime::now()?,
            )
            .await?;
        self.written(&recorded);
        Ok(())
    }

    /// Moves a resource to another live collection of the same kind, under
    /// the same name (MOVE with `If-Match`, never overwriting). It keeps
    /// its id.
    ///
    /// # Errors
    /// [`GroupwareError::NotFound`] for a collection of another kind; a
    /// conflict if it changed or the name is taken there; the server's
    /// refusal.
    pub async fn move_resource(
        &self,
        resource: &StoredResource,
        to: nebula_store::CollectionId,
    ) -> Result<StoredResource, GroupwareError> {
        let from = self.live_collection(resource.collection).await?;
        let target = self.live_collection(to).await?;
        if from.props.kind != target.props.kind {
            return Err(GroupwareError::NotFound);
        }
        let at = href(&resource.href)?;
        let name = at
            .last_segment()
            .and_then(|name| ResourceName::try_from(name).ok())
            .ok_or(GroupwareError::NotFound)?;
        let destination = href(&target.props.href)?
            .as_collection()
            .member(&name, false)
            .map_err(DavError::Href)?;
        let etag = self.etag_for(resource).await?;
        // No sync round between the MOVE and recording it: one could find
        // the resource at its new place first and record it as a new one,
        // and then the move here would find its destination taken (seen
        // with Stalwart 1.0.1, 2026-10-30).
        let one_round = self.sync_lock.lock().await;
        let new_etag = self
            .client_for(from.props.kind)
            .move_to(&at, &destination, Some(&etag), false)
            .await?;
        // The text is the same; read it from the server anyway when it
        // gives no etag, since the etag at the new place is needed.
        let kind = resource_kind(target.props.kind);
        // As after a PUT: only a strong etag says the bytes are ours.
        let (etag, text) = if let Some(etag) = new_etag.filter(|etag| !etag.is_weak()) {
            let current = self
                .resource_text(resource.id)
                .await?
                .ok_or(GroupwareError::NotFound)?;
            (Some(etag), current.text)
        } else {
            let fetched = self
                .client_for(target.props.kind)
                .get_as(
                    &destination,
                    kind,
                    vcard_request(target.props.vcard_versions),
                )
                .await?;
            (fetched.etag, fetched.data)
        };
        let write = self
            .prepare(&target, destination, etag_text(etag), text)
            .await?;
        let recorded = self
            .store()
            .move_dav_resource(self.id, resource.id, target.id, write, UnixTime::now()?)
            .await?;
        drop(one_round);
        self.written(&recorded);
        self.store()
            .dav_resource(self.id, resource.id)
            .await?
            .ok_or(GroupwareError::NotFound)
    }

    /// Lists the homes again and records what changed (after a
    /// collection write). Only a refused login is an error: the write
    /// itself worked.
    pub(crate) async fn relist(&self) -> Result<(), GroupwareError> {
        match Box::pin(self.sync_round(false)).await {
            Ok(_) => Ok(()),
            Err(GroupwareError::Unauthorized) => Err(GroupwareError::Unauthorized),
            Err(error) => {
                tracing::warn!(account = %self.id, %error, "dav: sync after a collection write failed");
                Ok(())
            }
        }
    }

    /// Creates a calendar (MKCALENDAR) or address book (extended MKCOL,
    /// RFC 5689) called `name` in the first home of its kind with these
    /// properties, and
    /// returns it as stored.
    ///
    /// # Errors
    /// [`GroupwareError::NotDiscovered`] without a home of that kind;
    /// [`GroupwareError::PropertiesRefused`] if the server would not set
    /// them; the server's refusal.
    pub async fn create_collection(
        &self,
        kind: CollectionKind,
        name: &ResourceName,
        values: &[PropValue],
    ) -> Result<Collection, GroupwareError> {
        let found = self.discovered().ok_or(GroupwareError::NotDiscovered)?;
        let homes = match kind {
            CollectionKind::Calendar => &found.calendar_homes,
            CollectionKind::AddressBook => &found.addressbook_homes,
        };
        let home = homes.first().ok_or(GroupwareError::NotDiscovered)?;
        let at = home
            .as_collection()
            .member(name, true)
            .map_err(DavError::Href)?;
        // Calendars with MKCALENDAR (RFC 4791 §5.3.1, every CalDAV server
        // has it); address books with extended MKCOL (RFC 5689).
        let made = match kind {
            CollectionKind::Calendar => self.client.mkcalendar(&at, values).await,
            CollectionKind::AddressBook => {
                self.client_for(kind)
                    .mkcol(&at, dav_kind(kind), values)
                    .await
            }
        };
        match made {
            Ok(()) => {}
            // RFC 5689 §5.2: a 207 lists what could not be set.
            Err(DavError::Status { status: 207, .. }) => {
                return Err(GroupwareError::PropertiesRefused(
                    values.iter().map(PropValue::property).collect(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        self.relist().await?;
        let stored = stored_href(&at)?;
        self.store()
            .dav_collections(self.id)
            .await?
            .into_iter()
            .find(|c| c.props.href == stored && c.orphaned_at.is_none())
            .ok_or(GroupwareError::NotFound)
    }

    /// Gives the account a calendar and an address book if its homes have
    /// none (like Stalwart, which always has a default of each): a new
    /// user of Radicale or Baïkal starts with empty homes, and the web
    /// app has nowhere to create events or contacts. Also a "Tasks" list
    /// (VTODO only) when no calendar takes tasks. Only for the kinds
    /// that are synced and have a home. Called after a full round (the
    /// store knows every collection); a failure is logged, never an
    /// error (the account keeps working without them).
    pub async fn ensure_defaults(&self) {
        let Some(found) = self.discovered() else {
            return;
        };
        let config = self.config();
        let live = match self.store().dav_collections(self.id).await {
            Ok(list) => list,
            Err(error) => {
                tracing::warn!(account = %self.id, %error, "dav: reading collections failed");
                return;
            }
        };
        // Only the user's own count: one shared by someone else is not a
        // place of the user's.
        let alive = |c: &&Collection| c.orphaned_at.is_none() && c.props.sharing.owner.is_none();
        let has_calendar = live
            .iter()
            .filter(alive)
            .any(|c| c.props.kind == CollectionKind::Calendar);
        // A calendar that does not say takes every component (RFC 4791
        // §5.2.3).
        let has_task_list = live.iter().filter(alive).any(|c| {
            c.props.kind == CollectionKind::Calendar
                && c.props
                    .components
                    .is_none_or(|set| set.contains(nebula_store::Components::TODO))
        });
        let has_book = live
            .iter()
            .filter(alive)
            .any(|c| c.props.kind == CollectionKind::AddressBook);
        for (kind, enabled, homes, name, display, has, components) in [
            (
                CollectionKind::Calendar,
                config.calendars,
                &found.calendar_homes,
                "calendar",
                "Calendar",
                has_calendar,
                Some(Component::Event),
            ),
            // Tasks: a list of their own, as Apple
            // Reminders, when no calendar takes VTODO (a new user of
            // Radicale, whose "Calendar" above is for events only).
            (
                CollectionKind::Calendar,
                config.calendars,
                &found.calendar_homes,
                "tasks",
                "Tasks",
                has_task_list,
                Some(Component::Todo),
            ),
            (
                CollectionKind::AddressBook,
                config.contacts,
                &found.addressbook_homes,
                "contacts",
                "Contacts",
                has_book,
                None,
            ),
        ] {
            if !enabled || homes.is_empty() || has {
                continue;
            }
            let Ok(name) = ResourceName::try_from(name) else {
                continue;
            };
            let mut values = vec![PropValue::DisplayName(display.to_owned())];
            if let Some(component) = components {
                values.push(PropValue::SupportedComponents(ComponentSet::new(&[
                    component,
                ])));
            }
            match self.create_collection(kind, &name, &values).await {
                Ok(_) => {
                    tracing::info!(account = %self.id, ?kind, "dav: created a default collection");
                }
                Err(error) => {
                    tracing::warn!(account = %self.id, ?kind, %error, "dav: creating a default collection failed");
                }
            }
        }
    }

    /// Sets and removes properties of a live collection (PROPPATCH).
    ///
    /// # Errors
    /// [`GroupwareError::PropertiesRefused`] with what the server refused
    /// (then nothing changed); the server's refusal.
    pub async fn update_collection(
        &self,
        collection: nebula_store::CollectionId,
        set: &[PropValue],
        remove: &[Property],
    ) -> Result<(), GroupwareError> {
        if set.is_empty() && remove.is_empty() {
            return Ok(());
        }
        let collection = self.live_collection(collection).await?;
        let at = href(&collection.props.href)?.as_collection();
        let refused = self
            .client_for(collection.props.kind)
            .proppatch(&at, set, remove)
            .await?;
        if !refused.is_empty() {
            return Err(GroupwareError::PropertiesRefused(
                refused.into_iter().map(|(property, _)| property).collect(),
            ));
        }
        self.relist().await
    }

    /// Deletes a live collection (DELETE, which takes everything in it).
    /// With [`Contents::MustBeEmpty`], the server is asked first for a
    /// fresh listing (PROPFIND Depth 1): the local copy may miss resources
    /// (new ones, or ones it can't show), and a blind DELETE would take
    /// them. There is no `If-Match` for collections on most servers, so a
    /// resource added between the listing and the DELETE (milliseconds)
    /// still goes with it.
    ///
    /// # Errors
    /// [`GroupwareError::HasContents`] if it must be empty and isn't (also
    /// when there are too many members to list); the server's refusal.
    pub async fn delete_collection(
        &self,
        collection: nebula_store::CollectionId,
        contents: Contents,
    ) -> Result<(), GroupwareError> {
        let collection = self.live_collection(collection).await?;
        let at = href(&collection.props.href)?.as_collection();
        let client = self.client_for(collection.props.kind);
        if contents == Contents::MustBeEmpty {
            match client.list_members(&at).await {
                Ok(members) if members.is_empty() => {}
                Ok(_) | Err(DavError::TooMany) => return Err(GroupwareError::HasContents),
                Err(DavError::NotFound) => return self.relist().await,
                Err(error) => return Err(error.into()),
            }
        }
        match client.delete(&at, None).await {
            Ok(()) | Err(DavError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        self.relist().await
    }
}
