-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- A calendar's public link and a webcal subscription's source are
-- capabilities: like the resources' text they
-- are kept only sealed with the account's data key, bound to the
-- collection's href and to which link it is. The links in clear go; the
-- next sync stores them sealed (one Calendar update each).
ALTER TABLE dav_collections DROP COLUMN publish_url;
ALTER TABLE dav_collections DROP COLUMN subscription_source;
ALTER TABLE dav_collections ADD COLUMN publish_url_sealed BLOB;
ALTER TABLE dav_collections ADD COLUMN subscription_source_sealed BLOB;
