-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Calendars and contacts from a CalDAV/CardDAV server.
-- Times are Unix seconds (UTC). Hrefs are full URLs, as the bridge
-- normalised them. Ids of collections and resources are JMAP ids, never
-- reused: AUTOINCREMENT.

-- What discovery found for an account (one DAV account per mail account,
-- so the mail account's id is the key). Lists are JSON arrays of strings.
CREATE TABLE dav_accounts (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    principal TEXT NOT NULL,
    calendar_homes TEXT NOT NULL,
    addressbook_homes TEXT NOT NULL,
    schedule_outbox TEXT,
    -- `calendar-user-address-set` (`mailto:…`).
    user_addresses TEXT NOT NULL,
    -- The server announced `calendar-auto-schedule` (RFC 6638).
    auto_schedule INTEGER NOT NULL,
    discovered_at INTEGER NOT NULL
) STRICT;

-- Calendars (kind 1) and address books (kind 2).
CREATE TABLE dav_collections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    kind INTEGER NOT NULL,
    href TEXT NOT NULL,
    name TEXT,
    -- `#RRGGBB` or `#RRGGBBAA`.
    color TEXT,
    description TEXT,
    sort_order INTEGER,
    -- TZID of `calendar-timezone`.
    time_zone TEXT,
    -- `current-user-privilege-set` as bits (NULL: the server did not say).
    rights INTEGER,
    -- `supported-calendar-component-set` as bits (NULL: any).
    components INTEGER,
    -- `supported-address-data` versions as bits (NULL: not said).
    vcard_versions INTEGER,
    -- `sync-collection` (RFC 6578) works here.
    supports_sync INTEGER NOT NULL,
    -- Where the last complete sync left off.
    sync_token TEXT,
    ctag TEXT,
    synced_at INTEGER,
    -- Gone from the server: hidden, deleted after a day unless it returns.
    orphaned_at INTEGER,
    -- Only in the bridge (JMAP `isVisible`, `isSubscribed`, `isDefault`).
    is_visible INTEGER NOT NULL DEFAULT 1,
    is_subscribed INTEGER NOT NULL DEFAULT 1,
    is_default INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE UNIQUE INDEX dav_collections_by_href ON dav_collections (account_id, href);

-- iCalendar objects and vCards. `data` is sealed with the account's data
-- key; only the indexes are in clear.
CREATE TABLE dav_resources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    collection_id INTEGER NOT NULL REFERENCES dav_collections (id) ON DELETE CASCADE,
    href TEXT NOT NULL,
    etag TEXT,
    uid TEXT,
    data BLOB NOT NULL,
    -- Bytes of the iCalendar or vCard text.
    size INTEGER NOT NULL,
    -- First start and last end of every occurrence. NULL start: unknown;
    -- NULL end: open-ended (or more occurrences than were expanded).
    utc_start INTEGER,
    utc_end INTEGER,
    recurring INTEGER NOT NULL,
    -- Contacts: the name to sort by (lower case).
    sort_name TEXT,
    -- The text does not parse: kept, but not shown over JMAP.
    invalid INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- Gone from the server: hidden, deleted after a day unless it returns
    -- (a move by another client keeps its id).
    orphaned_at INTEGER
) STRICT;

CREATE UNIQUE INDEX dav_resources_by_href ON dav_resources (collection_id, href);
CREATE INDEX dav_resources_by_uid ON dav_resources (account_id, uid);
CREATE INDEX dav_resources_by_start ON dav_resources (account_id, utc_start);

-- Alerts already shown, so a restart does not show them again.
-- `occurrence` is the UTC start of the occurrence the alert belongs to.
CREATE TABLE calendar_alerts_fired (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    resource_id INTEGER NOT NULL REFERENCES dav_resources (id) ON DELETE CASCADE,
    alert_id TEXT NOT NULL,
    occurrence INTEGER NOT NULL,
    fired_at INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX calendar_alerts_fired_once
    ON calendar_alerts_fired (resource_id, alert_id, occurrence);
