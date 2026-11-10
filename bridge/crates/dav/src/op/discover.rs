// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Discovery: where the user's calendars and address books live.
//!
//! RFC 6764 §6 and libdav's `find_current_user_principal`: ask for
//! `current-user-principal` at the configured URL (if it has a path), at
//! `/.well-known/caldav` and `/.well-known/carddav` (following their
//! redirects, each checked), then at `/`. Then the principal's homes
//! (RFC 4791 §6.2.1, RFC 6352 §7.1.1), addresses and outbox (RFC 6638),
//! where the other principals are (RFC 3744 §5.8), and the `DAV:` header
//! of each home.
//!
//! With iCloud (`https://caldav.icloud.com`, `https://contacts.icloud.com`)
//! discovery may follow Apple's per-account hosts (`p<digits>-caldav…`,
//! `p<digits>-contacts…`, [`AccountHosts`](crate::AccountHosts)) and
//! remembers the one it found: later requests send the login only there.

use nebula_dav_proto::{CalendarUserType, DavCapabilities, Depth, Href, Property, Status};

use crate::op::response_for;
use crate::{DavClient, DavError, Origin};

/// What discovery found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Discovery {
    /// The user's principal.
    pub principal: Href,
    /// `calendar-home-set` (allowed origins only).
    pub calendar_homes: Vec<Href>,
    /// `addressbook-home-set` (allowed origins only).
    pub addressbook_homes: Vec<Href>,
    /// `calendar-user-address-set` (`mailto:…`).
    pub calendar_user_addresses: Vec<String>,
    /// `schedule-outbox-URL`.
    pub schedule_outbox: Option<Href>,
    /// `principal-collection-set` (allowed origins only).
    pub principal_collections: Vec<Href>,
    /// The principal's `displayname`.
    pub principal_name: Option<String>,
    /// The principal's `calendar-user-type`.
    pub principal_type: Option<CalendarUserType>,
    /// The union of the `DAV:` headers of the homes (or the principal).
    pub capabilities: DavCapabilities,
}

/// Errors that end discovery; the others try the next place.
const fn is_fatal(error: &DavError) -> bool {
    matches!(
        error,
        DavError::Unauthorized
            | DavError::Guard(_)
            | DavError::Tls(_)
            | DavError::PlainHttpNotLoopback
            | DavError::Timeout
    )
}

impl DavClient {
    /// Finds the principal, its homes and the server's capabilities.
    ///
    /// # Errors
    ///
    /// [`DavError::Unauthorized`] for a wrong password, a connection or
    /// policy error, or [`DavError::Protocol`] if no place gave a
    /// principal, the principal's answer is not about it, or it has
    /// neither home set with a 2xx status.
    pub async fn discover(&self) -> Result<Discovery, DavError> {
        let principal = self.find_principal().await?;
        let (multistatus, final_url) = self
            .propfind_as(
                &principal,
                Depth::Zero,
                &[
                    Property::CalendarHomeSet,
                    Property::AddressbookHomeSet,
                    Property::CalendarUserAddressSet,
                    Property::ScheduleOutboxUrl,
                    Property::PrincipalCollectionSet,
                    Property::DisplayName,
                    Property::CalendarUserType,
                ],
                true,
            )
            .await?;
        // An answer without the principal, or without any home set, is
        // not "no homes": taken as that, every collection would be hidden.
        let response = response_for(&multistatus, &principal, &final_url)
            .ok_or(DavError::Protocol("no answer for the principal"))?;
        let has_home = |property| response.status_of(property) == Some(Status::OK);
        if !has_home(Property::CalendarHomeSet) && !has_home(Property::AddressbookHomeSet) {
            return Err(DavError::Protocol("no home set"));
        }
        let properties = response.properties.clone();
        // iCloud: the account's own host is the first host of the pattern
        // among the principal, where its answer came from, and what it
        // names. From here on only that one gets the login; hrefs on any
        // other host of the pattern are dropped like any other origin.
        let account_host = self.server().account_hosts().and_then(|rule| {
            [&principal, &final_url]
                .into_iter()
                .chain(&properties.calendar_home_set)
                .chain(&properties.addressbook_home_set)
                .chain(&properties.schedule_outbox_url)
                .chain(&properties.principal_collection_set)
                .filter_map(|href| Origin::of(href.url()).ok())
                .find(|origin| rule.matches(origin))
        });
        if account_host.is_some() {
            tracing::debug!("dav: the account's iCloud host was found");
        }
        self.set_account_host(account_host);
        self.check_origin(&principal)?;
        let allowed = |hrefs: Vec<Href>| -> Vec<Href> {
            let total = hrefs.len();
            let kept: Vec<Href> = hrefs
                .into_iter()
                .filter(|href| self.check_origin(href).is_ok())
                .map(|href| href.as_collection())
                .collect();
            if kept.len() < total {
                tracing::warn!(
                    dropped = total - kept.len(),
                    "dav: homes or principal collections on an origin that is not allowed were ignored"
                );
            }
            kept
        };
        let calendar_homes = allowed(properties.calendar_home_set);
        let addressbook_homes = allowed(properties.addressbook_home_set);
        let principal_collections = allowed(properties.principal_collection_set);
        let schedule_outbox = properties
            .schedule_outbox_url
            .filter(|href| self.check_origin(href).is_ok());

        let mut capabilities = DavCapabilities::default();
        let mut asked: Vec<&Href> = Vec::new();
        for home in calendar_homes.iter().chain(&addressbook_homes) {
            if asked.iter().any(|done| done.same_location(home)) {
                continue;
            }
            asked.push(home);
            match self.options(home).await {
                Ok(found) => capabilities = capabilities.union(found),
                Err(error) if is_fatal(&error) => return Err(error),
                Err(error) => tracing::debug!(%error, "dav: OPTIONS on a home failed"),
            }
        }
        if asked.is_empty() {
            capabilities = self.options(&principal).await.unwrap_or_default();
        }
        self.set_webcal_cache(capabilities.nc_calendar_webcal_cache);
        Ok(Discovery {
            principal,
            calendar_homes,
            addressbook_homes,
            calendar_user_addresses: properties.calendar_user_address_set,
            schedule_outbox,
            principal_collections,
            principal_name: properties.display_name,
            principal_type: properties.calendar_user_type,
            capabilities,
        })
    }

    async fn find_principal(&self) -> Result<Href, DavError> {
        let base = &self.server().base;
        let mut candidates = Vec::with_capacity(4);
        if !base.segments().is_empty() {
            candidates.push(base.clone());
        }
        for path in ["/.well-known/caldav", "/.well-known/carddav", "/"] {
            candidates.push(Href::resolve(path, base)?);
        }
        for candidate in &candidates {
            match self.current_user_principal(candidate).await {
                Ok(Some(principal)) => {
                    self.check(&principal, true)?;
                    return Ok(principal.as_collection());
                }
                Ok(None) => {}
                Err(error) if is_fatal(&error) => return Err(error),
                Err(error) => {
                    tracing::debug!(%error, "dav: no principal there, trying the next place");
                }
            }
        }
        Err(DavError::Protocol("no current-user-principal found"))
    }

    async fn current_user_principal(&self, href: &Href) -> Result<Option<Href>, DavError> {
        let (multistatus, _final_url) = self
            .propfind_as(href, Depth::Zero, &[Property::CurrentUserPrincipal], true)
            .await?;
        Ok(multistatus
            .responses
            .into_iter()
            .find_map(|response| response.properties.current_user_principal))
    }
}
