-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- A calendar's default alarms (4.5): the `VALARM`s of
-- `default-alarm-vevent-datetime` and `-date` (draft-daboo-valarm-
-- extensions), as the bridge rewrote them (only alarms it understands);
-- NULL when there are none. The time zone stays in `time_zone`, now also
-- from `calendar-timezone-id` (RFC 7809).
ALTER TABLE dav_collections ADD COLUMN default_alarms_datetime TEXT;
ALTER TABLE dav_collections ADD COLUMN default_alarms_date TEXT;
