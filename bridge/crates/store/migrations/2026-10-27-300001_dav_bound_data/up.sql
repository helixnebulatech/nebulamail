-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- The sealed copy of each DAV resource is now bound to the resource too
-- (its collection id and href), not only to the account.
-- The old copies (field tag 5) no longer open, and nothing is
-- worth rewrapping: they were development only and no JMAP client saw
-- these ids (no calendars or contacts capability was announced). So they
-- are dropped and every collection syncs again from scratch at the next
-- round. The collections themselves (and their ids) stay.
DELETE FROM calendar_alerts_fired;
DELETE FROM dav_resources;
UPDATE dav_collections SET sync_token = NULL, ctag = NULL, synced_at = NULL;
