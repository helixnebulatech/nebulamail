// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Nextcloud's calendar extras, only where
//! the server announced them ([`NextcloudSupport`]):
//!
//! - **Publishing**: [`GroupwareAccount::set_published`] on a calendar
//!   the store marks `publishable`; the public link comes back in the
//!   next listing (only shown, never fetched).
//! - **Subscriptions**: [`GroupwareAccount::subscribe`] creates one;
//!   Nextcloud fetches its source with its own rules. The bridge never
//!   connects to a subscription's URL, so it is only checked for shape
//!   ([`ExternalUrl::subscription`]).
//! - **Trash bin**: [`GroupwareAccount::trash`] lists the deleted
//!   calendars and events of the calendar homes;
//!   [`GroupwareAccount::restore_trashed`] and
//!   [`GroupwareAccount::delete_trashed`] act on some of them. Items are
//!   named by a number from their href (FNV-1a), never by the href.

use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{ExternalUrl, Href, ResourceName};
use nebula_dav::{DavError, DeletedEvent, TrashBin, TrashContents};
use nebula_store::{Collection, CollectionId, CollectionKind, DavHref, UnixTime};

use crate::GroupwareError;
use crate::core::account::{GroupwareAccount, NextcloudSupport};
use crate::model::convert::icalendar_index;
use crate::model::index::stored_href;
use crate::write::href;

/// What a trash bin item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrashKind {
    /// A deleted calendar.
    Calendar,
    /// A deleted event or task.
    Event,
}

/// One item of the trash bin, by kind and number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrashItem {
    /// Calendar or event.
    pub kind: TrashKind,
    /// FNV-1a of its href (stable while it stays in the bin).
    pub number: u64,
}

/// A calendar in the trash bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedCalendar {
    /// Its number.
    pub number: u64,
    /// `displayname`.
    pub name: Option<String>,
    /// `calendar-color` (`#RRGGBB`).
    pub color: Option<String>,
    /// When it was deleted.
    pub deleted_at: Option<UnixTime>,
}

/// An event or task in the trash bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedEvent {
    /// Its number.
    pub number: u64,
    /// Its `SUMMARY` (at most 256 characters).
    pub title: Option<String>,
    /// Its first start, if it has one.
    pub utc_start: Option<UnixTime>,
    /// The calendar it was in, if that one is still there.
    pub calendar: Option<CollectionId>,
    /// When it was deleted.
    pub deleted_at: Option<UnixTime>,
}

/// What the trash bin holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trash {
    /// How long things stay, in seconds.
    pub retention_secs: Option<u64>,
    /// Deleted calendars.
    pub calendars: Vec<TrashedCalendar>,
    /// Deleted events and tasks.
    pub events: Vec<TrashedEvent>,
}

/// FNV-1a (64 bits) of an href's text: the item's number.
fn number(href: &Href) -> u64 {
    href.url()
        .as_str()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-10-28T09:31:37+00:00` (or `Z`, or another offset) as Unix time;
/// `None` for anything else.
fn rfc3339(text: &str) -> Option<UnixTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.is_ascii() {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let shape = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')]
        .iter()
        .all(|(at, c)| bytes.get(*at) == Some(c))
        && matches!(bytes.get(10), Some(b'T' | b't' | b' '));
    if !shape {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Skip fractions of a second.
    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = fraction.get(digits..)?;
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes().get(3) != Some(&b':') {
                return None;
            }
            let hours: i64 = rest.get(1..3)?.parse().ok()?;
            let minutes: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(UnixTime::from_secs(
        days * 86_400 + hour * 3600 + minute * 60 + second - offset,
    ))
}

/// The first `SUMMARY` of an iCalendar text, unescaped, at most 256
/// characters; `None` without one.
fn summary(text: &str) -> Option<String> {
    // Unfold (RFC 5545 §3.1) only what is needed: lines start at a CRLF
    // not followed by a space or tab.
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(raw.to_owned()),
        }
    }
    let line = lines.iter().find(|line| {
        let name = line.split([':', ';']).next().unwrap_or("");
        name.eq_ignore_ascii_case("SUMMARY")
    })?;
    let (_, value) = line.split_once(':')?;
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        let c = match c {
            '\\' => match chars.next() {
                Some('n' | 'N') => ' ',
                Some(other) => other,
                None => break,
            },
            c if c.is_control() => ' ',
            c => c,
        };
        out.push(c);
        if out.chars().count() >= 256 {
            break;
        }
    }
    let out = out.trim().to_owned();
    (!out.is_empty()).then_some(out)
}

/// Occurrences expanded to find a trashed event's first start: a few,
/// not `max_occurrences` (the first start is all the bin shows). calcard
/// counts excluded dates against it too, so an event whose first 63
/// dates are all excluded has none.
const TRASH_OCCURRENCES: usize = 64;

/// The trash bin's events of `home`, with their title, first start and
/// calendar (one of `live`, by href). CPU work (calcard): off the async
/// threads.
fn trashed_events(
    home: &Href,
    events: Vec<DeletedEvent>,
    live: &[(DavHref, CollectionId)],
) -> Vec<TrashedEvent> {
    events
        .into_iter()
        .map(|event| {
            // The calendar it was in: a member of the same home with that
            // last segment.
            let calendar = event.calendar_uri.as_deref().and_then(|uri| {
                let wanted = ResourceName::try_from(uri).ok()?;
                let at = stored_href(&home.as_collection().member(&wanted, true).ok()?).ok()?;
                live.iter().find(|(href, _)| *href == at).map(|(_, id)| *id)
            });
            let data = event.data.as_deref().unwrap_or("");
            let utc_start = icalendar_index(data.as_bytes(), TRASH_OCCURRENCES)
                .ok()
                .and_then(|index| index.utc_start)
                .map(UnixTime::from_secs);
            TrashedEvent {
                number: number(&event.href),
                title: summary(data),
                utc_start,
                calendar,
                deleted_at: event.deleted_at.as_deref().and_then(rfc3339),
            }
        })
        .collect()
}

impl GroupwareAccount {
    fn calendar_homes_for(
        &self,
        wanted: impl Fn(NextcloudSupport) -> bool,
    ) -> Result<Vec<Href>, GroupwareError> {
        let found = self.discovered().ok_or(GroupwareError::NotDiscovered)?;
        if !wanted(found.nextcloud) || !self.config().calendars {
            return Err(GroupwareError::Unsupported);
        }
        Ok(found.calendar_homes)
    }

    /// The links of a calendar in clear (its public link, its
    /// subscription's source), opened with the account's data key. A link
    /// that does not open is left out.
    #[must_use]
    pub fn calendar_links(&self, calendar: &Collection) -> crate::model::links::OpenLinks {
        crate::model::links::open_all(&self.key, &calendar.props.href, &calendar.props.extras)
    }

    /// Publishes `calendar` at a public link or stops publishing it, then
    /// lists the homes again (the link comes in `CS:publish-url`).
    ///
    /// # Errors
    /// [`GroupwareError::Unsupported`] unless the store marks it
    /// publishable; the server's refusal.
    pub async fn set_published(
        &self,
        calendar: &Collection,
        publish: bool,
    ) -> Result<(), GroupwareError> {
        if calendar.props.kind != CollectionKind::Calendar || !calendar.props.extras.publishable {
            return Err(GroupwareError::Unsupported);
        }
        let at = href(&calendar.props.href)?.as_collection();
        self.client.publish(&at, publish).await?;
        self.relist().await
    }

    /// Subscribes to `source` in the first calendar home, as `name`, with
    /// `values` (display name, colour): Nextcloud fetches it. Returns the
    /// new calendar as stored.
    ///
    /// # Errors
    /// [`GroupwareError::Unsupported`] without subscriptions;
    /// [`GroupwareError::PropertiesRefused`] if the server would not set
    /// them; the server's refusal.
    pub async fn subscribe(
        &self,
        name: &ResourceName,
        source: &ExternalUrl,
        values: &[PropValue],
    ) -> Result<Collection, GroupwareError> {
        let homes = self.calendar_homes_for(|support| support.subscriptions)?;
        let home = homes.first().ok_or(GroupwareError::NotDiscovered)?;
        let at = home
            .as_collection()
            .member(name, true)
            .map_err(DavError::Href)?;
        match self.client.subscribe(&at, source, values).await {
            Ok(()) => {}
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

    /// The trash bins of the calendar homes, each with its home: with the
    /// events' data to show them, or only hrefs to restore or delete one
    /// (whatever the size of the events).
    async fn trash_bins(
        &self,
        contents: TrashContents,
    ) -> Result<Vec<(Href, TrashBin)>, GroupwareError> {
        let homes = self.calendar_homes_for(|support| support.trash_bin)?;
        let mut bins = Vec::with_capacity(homes.len());
        for home in homes {
            let bin = self.client.trash_bin(&home, contents).await?;
            bins.push((home, bin));
        }
        Ok(bins)
    }

    /// What the trash bin holds now (asked to the server each time).
    ///
    /// # Errors
    /// [`GroupwareError::Unsupported`] without a trash bin; the server's.
    pub async fn trash(&self) -> Result<Trash, GroupwareError> {
        let bins = self.trash_bins(TrashContents::WithData).await?;
        let live: Vec<Collection> = self
            .store()
            .dav_collections(self.id)
            .await?
            .into_iter()
            .filter(|c| c.orphaned_at.is_none() && c.props.kind == CollectionKind::Calendar)
            .collect();
        let live: Vec<(DavHref, CollectionId)> =
            live.into_iter().map(|c| (c.props.href, c.id)).collect();
        let mut trash = Trash::default();
        for (home, bin) in bins {
            trash.retention_secs = trash.retention_secs.or(bin.retention);
            for calendar in bin.calendars {
                trash.calendars.push(TrashedCalendar {
                    number: number(&calendar.href),
                    name: calendar.name,
                    color: calendar.color.map(|color| color.to_string()),
                    deleted_at: calendar.deleted_at.as_deref().and_then(rfc3339),
                });
            }
            // Up to MAX_TRASH_EVENTS objects to parse: off the async
            // threads.
            let live = live.clone();
            let events = bin.events;
            let found = tokio::task::spawn_blocking(move || trashed_events(&home, events, &live))
                .await
                .map_err(|_| GroupwareError::Task)?;
            trash.events.extend(found);
        }
        Ok(trash)
    }

    /// Where each item is now (one listing), with its home.
    async fn locate(
        &self,
        items: &[TrashItem],
    ) -> Result<Vec<Option<(Href, Href)>>, GroupwareError> {
        let bins = self.trash_bins(TrashContents::HrefsOnly).await?;
        Ok(items
            .iter()
            .map(|item| {
                bins.iter().find_map(|(home, bin)| {
                    let found = match item.kind {
                        TrashKind::Calendar => bin
                            .calendars
                            .iter()
                            .map(|c| &c.href)
                            .find(|h| number(h) == item.number),
                        TrashKind::Event => bin
                            .events
                            .iter()
                            .map(|e| &e.href)
                            .find(|h| number(h) == item.number),
                    };
                    found.map(|at| (home.clone(), at.clone()))
                })
            })
            .collect())
    }

    /// Restores these items (a MOVE each to the bin's `restore/`), then
    /// syncs. One result per item: [`GroupwareError::NotFound`] if it is
    /// not in the bin.
    ///
    /// # Errors
    /// [`GroupwareError::Unsupported`] without a trash bin; listing it.
    pub async fn restore_trashed(
        &self,
        items: &[TrashItem],
    ) -> Result<Vec<Result<(), GroupwareError>>, GroupwareError> {
        let places = self.locate(items).await?;
        let mut results = Vec::with_capacity(items.len());
        for place in places {
            results.push(match place {
                None => Err(GroupwareError::NotFound),
                Some((home, at)) => self
                    .client
                    .restore(&home, &at)
                    .await
                    .map_err(GroupwareError::from),
            });
        }
        if results.iter().any(Result::is_ok) {
            self.relist().await?;
        }
        Ok(results)
    }

    /// Deletes these items for good. One result per item, as
    /// [`GroupwareAccount::restore_trashed`].
    ///
    /// # Errors
    /// [`GroupwareError::Unsupported`] without a trash bin; listing it.
    pub async fn delete_trashed(
        &self,
        items: &[TrashItem],
    ) -> Result<Vec<Result<(), GroupwareError>>, GroupwareError> {
        let places = self.locate(items).await?;
        let mut results = Vec::with_capacity(items.len());
        for place in places {
            results.push(match place {
                None => Err(GroupwareError::NotFound),
                Some((_, at)) => match self.client.delete_forever(&at).await {
                    Ok(()) | Err(DavError::NotFound) => Ok(()),
                    Err(error) => Err(error.into()),
                },
            });
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::{days_from_civil, rfc3339, summary, trashed_events};
    use nebula_dav::DeletedEvent;
    use nebula_dav::proto::Href;
    use nebula_store::{CollectionId, DavHref};

    /// The first start of a trashed event, without expanding the whole
    /// series: with its DTSTART excluded, the day after.
    #[test]
    fn trashed_event_starts() {
        let home = Href::from_url(&url::Url::parse("http://h/dav/cal/alice/").unwrap()).unwrap();
        let at = |path: &str| {
            Href::from_url(&url::Url::parse(&format!("http://h/dav/cal/alice/{path}")).unwrap())
                .unwrap()
        };
        let work = CollectionId::from_raw(7).unwrap();
        let live = [(DavHref::new("http://h/dav/cal/alice/work/").unwrap(), work)];
        let endless = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
            UID:d\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:20261019T100000Z\r\nDURATION:PT1H\r\n\
            RRULE:FREQ=DAILY\r\nEXDATE:20261019T100000Z\r\nSUMMARY:Daily\r\n\
            END:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = vec![
            DeletedEvent {
                href: at("trashbin/objects/1.ics"),
                etag: None,
                calendar_uri: Some("work".to_owned()),
                deleted_at: None,
                data: Some(endless.to_owned()),
            },
            DeletedEvent {
                href: at("trashbin/objects/2.ics"),
                etag: None,
                calendar_uri: Some("gone".to_owned()),
                deleted_at: None,
                data: None,
            },
        ];
        let found = trashed_events(&home, events, &live);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].title.as_deref(), Some("Daily"));
        assert_eq!(
            found[0].utc_start.map(nebula_store::UnixTime::secs),
            Some(1_792_404_000 + 86_400)
        );
        assert_eq!(found[0].calendar, Some(work));
        assert_eq!(found[1].title, None);
        assert_eq!(found[1].utc_start, None);
        assert_eq!(found[1].calendar, None);
    }

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        let at = |t: &str| rfc3339(t).map(nebula_store::UnixTime::secs);
        assert_eq!(at("2026-10-28T09:31:37+00:00"), Some(1_793_179_897));
        assert_eq!(at("2026-10-28T09:31:37Z"), Some(1_793_179_897));
        assert_eq!(at("2026-10-28T11:31:37+02:00"), Some(1_793_179_897));
        assert_eq!(at("2026-10-28T09:31:37.123Z"), Some(1_793_179_897));
        for bad in [
            "",
            "2026-10-28",
            "2026-13-28T09:31:37Z",
            "2026-10-28T09:31:37",
            "2026-10-28T09:31:37+0000",
            "2026-10-28X09:31:37Z",
            "２026-10-28T09:31:37Z",
            "+026-10-28T09:31:37Z",
        ] {
            assert_eq!(at(bad), None, "{bad}");
        }
    }

    #[test]
    fn summaries() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY;LANGUAGE=es:Cena\\, con\r\n  amigos\\nya\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(summary(text).as_deref(), Some("Cena, con amigos ya"));
        assert_eq!(summary("BEGIN:VEVENT\r\nSUMMARY:\r\nEND:VEVENT"), None);
        assert_eq!(summary("BEGIN:VEVENT\r\nEND:VEVENT"), None);
        let long = format!("SUMMARY:{}", "é".repeat(1000));
        assert_eq!(summary(&long).map(|s| s.chars().count()), Some(256));
        assert_eq!(
            summary("summary:a\u{7}b").as_deref(),
            Some("a b"),
            "controls become spaces"
        );
    }
}
