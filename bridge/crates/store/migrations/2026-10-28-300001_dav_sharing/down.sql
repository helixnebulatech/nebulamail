-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

ALTER TABLE dav_collections DROP COLUMN shares;
ALTER TABLE dav_collections DROP COLUMN share_mode;
ALTER TABLE dav_collections DROP COLUMN owner;
