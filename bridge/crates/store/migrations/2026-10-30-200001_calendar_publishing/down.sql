-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

ALTER TABLE dav_collections DROP COLUMN subscription_source;
ALTER TABLE dav_collections DROP COLUMN is_subscription;
ALTER TABLE dav_collections DROP COLUMN publish_url;
ALTER TABLE dav_collections DROP COLUMN publishable;
