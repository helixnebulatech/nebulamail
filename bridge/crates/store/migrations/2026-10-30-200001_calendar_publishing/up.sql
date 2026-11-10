-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Nextcloud's calendar extras: whether the
-- user may publish a calendar at a public link and that link
-- (`CS:publish-url`, only shown); whether it is a webcal subscription
-- (read-only, fetched by the server) and its source (`CS:source`, only
-- shown, never fetched by the bridge).
ALTER TABLE dav_collections ADD COLUMN publishable INTEGER NOT NULL DEFAULT 0;
ALTER TABLE dav_collections ADD COLUMN publish_url TEXT;
ALTER TABLE dav_collections ADD COLUMN is_subscription INTEGER NOT NULL DEFAULT 0;
ALTER TABLE dav_collections ADD COLUMN subscription_source TEXT;
