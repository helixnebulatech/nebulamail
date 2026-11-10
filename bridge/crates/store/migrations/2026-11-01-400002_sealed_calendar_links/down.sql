-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

ALTER TABLE dav_collections DROP COLUMN publish_url_sealed;
ALTER TABLE dav_collections DROP COLUMN subscription_source_sealed;
ALTER TABLE dav_collections ADD COLUMN publish_url TEXT;
ALTER TABLE dav_collections ADD COLUMN subscription_source TEXT;
