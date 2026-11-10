// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A calendar's links (Nextcloud's public link, a webcal subscription's
//! source): capabilities to read it, so the store keeps them only sealed
//! with the account's data key as `Field::DavCollectionLink`, bound to the
//! account, the collection's href and which link it is.
//! The href and the kind are known before the collection has a row, and
//! never change for it.

use nebula_crypto::{AccountKey, Binding, CryptoError, Field, SealedField};
use nebula_store::{CalendarExtras, DavHref, ExternalUrl, SealedLink};
use secrecy::ExposeSecret as _;

use crate::GroupwareError;

/// Which link of a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The public link of a published calendar (`CS:publish-url`).
    Publish,
    /// A subscription's source (`CS:source`).
    Source,
}

impl Link {
    const fn name(self) -> &'static [u8] {
        match self {
            Self::Publish => b"publish",
            Self::Source => b"source",
        }
    }
}

/// The bytes of the binding: the href, a NUL (never in an href) and the
/// link's name.
fn binding(href: &DavHref, link: Link) -> Vec<u8> {
    let name = link.name();
    let mut bytes = Vec::with_capacity(href.as_str().len().saturating_add(1 + name.len()));
    bytes.extend_from_slice(href.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(name);
    bytes
}

/// Seals a link of the calendar at `href`.
///
/// # Errors
/// [`GroupwareError::Crypto`] if sealing fails; [`GroupwareError::Store`]
/// if the result is over [`SealedLink::MAX_LEN`] (never for an
/// [`ExternalUrl`]).
pub fn seal(
    key: &AccountKey,
    href: &DavHref,
    link: Link,
    url: &ExternalUrl,
) -> Result<SealedLink, GroupwareError> {
    let bytes = binding(href, link);
    let sealed = key.seal_bound(
        Field::DavCollectionLink,
        &Binding::new(&bytes)?,
        url.as_str().as_bytes(),
    )?;
    Ok(SealedLink::new(sealed.as_bytes().to_vec())?)
}

/// Opens a stored link of the calendar at `href`.
///
/// # Errors
/// [`CryptoError`] if it was sealed for another account, calendar or
/// link (or changed); `CryptoError::Decrypt` too if it is not a URL.
pub fn open(
    key: &AccountKey,
    href: &DavHref,
    link: Link,
    stored: &SealedLink,
) -> Result<ExternalUrl, CryptoError> {
    let sealed = SealedField::from_bytes(stored.as_bytes())?;
    let bytes = binding(href, link);
    let plain = key.open_bound(Field::DavCollectionLink, &Binding::new(&bytes)?, &sealed)?;
    std::str::from_utf8(plain.expose_secret())
        .ok()
        .and_then(|text| ExternalUrl::new(text).ok())
        .ok_or(CryptoError::Decrypt)
}

/// A calendar's links in clear, for JMAP. A link that does not open
/// (another key, a changed row) is left out and logged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OpenLinks {
    /// The public link.
    pub publish_url: Option<ExternalUrl>,
    /// The subscription's source.
    pub subscription_source: Option<ExternalUrl>,
}

impl std::fmt::Debug for OpenLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenLinks")
            .field("publish_url", &self.publish_url.is_some())
            .field("subscription_source", &self.subscription_source.is_some())
            .finish()
    }
}

/// Opens both links of a calendar's extras.
#[must_use]
pub fn open_all(key: &AccountKey, href: &DavHref, extras: &CalendarExtras) -> OpenLinks {
    let one = |link: Link, stored: Option<&SealedLink>| {
        let stored = stored?;
        match open(key, href, link, stored) {
            Ok(url) => Some(url),
            Err(error) => {
                tracing::warn!(%error, ?link, "a calendar link does not open; left out");
                None
            }
        }
    };
    OpenLinks {
        publish_url: one(Link::Publish, extras.publish_url.as_ref()),
        subscription_source: one(Link::Source, extras.subscription_source.as_ref()),
    }
}

/// Seals `url` unless `previous` already holds it: then the stored bytes
/// are kept, so a sync round with nothing new records no update (a new
/// seal has a new nonce).
///
/// # Errors
/// As [`seal`].
pub(crate) fn seal_or_keep(
    key: &AccountKey,
    href: &DavHref,
    link: Link,
    url: Option<&ExternalUrl>,
    previous: Option<&SealedLink>,
) -> Result<Option<SealedLink>, GroupwareError> {
    let Some(url) = url else {
        return Ok(None);
    };
    if let Some(previous) = previous
        && open(key, href, link, previous).is_ok_and(|old| old == *url)
    {
        return Ok(Some(previous.clone()));
    }
    seal(key, href, link, url).map(Some)
}

#[cfg(test)]
mod tests {
    use nebula_crypto::{AccountId, DataKey};

    use super::*;

    fn key() -> AccountKey {
        AccountKey::new(AccountId::new(b"a").unwrap(), DataKey::generate().unwrap())
    }

    #[test]
    fn bound_to_calendar_and_link() {
        let key = key();
        let work = DavHref::new("https://dav.example/cal/work/").unwrap();
        let home = DavHref::new("https://dav.example/cal/home/").unwrap();
        let url = ExternalUrl::new("https://cloud.example/p/SECRET").unwrap();
        let sealed = seal(&key, &work, Link::Publish, &url).unwrap();
        assert!(
            !sealed
                .as_bytes()
                .windows(6)
                .any(|window| window == b"SECRET")
        );
        assert_eq!(open(&key, &work, Link::Publish, &sealed).unwrap(), url);
        assert!(open(&key, &work, Link::Source, &sealed).is_err());
        assert!(open(&key, &home, Link::Publish, &sealed).is_err());
        assert!(open(&self::key(), &work, Link::Publish, &sealed).is_err());
    }

    #[test]
    fn unchanged_links_keep_their_bytes() {
        let key = key();
        let work = DavHref::new("https://dav.example/cal/work/").unwrap();
        let url = ExternalUrl::new("webcal://example.com/h.ics").unwrap();
        let first = seal_or_keep(&key, &work, Link::Source, Some(&url), None)
            .unwrap()
            .unwrap();
        let again = seal_or_keep(&key, &work, Link::Source, Some(&url), Some(&first))
            .unwrap()
            .unwrap();
        assert_eq!(again, first);
        let other = ExternalUrl::new("webcal://example.com/other.ics").unwrap();
        let changed = seal_or_keep(&key, &work, Link::Source, Some(&other), Some(&first))
            .unwrap()
            .unwrap();
        assert_ne!(changed, first);
        assert_eq!(open(&key, &work, Link::Source, &changed).unwrap(), other);
        assert!(
            seal_or_keep(&key, &work, Link::Source, None, Some(&first))
                .unwrap()
                .is_none()
        );
        let extras = CalendarExtras {
            subscription_source: Some(changed),
            ..CalendarExtras::default()
        };
        let links = open_all(&key, &work, &extras);
        assert_eq!(links.subscription_source, Some(other));
        assert_eq!(links.publish_url, None);
    }
}
