-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- A used authorization code is kept (marked) until a while after it
-- expires, so a second use can revoke the tokens issued from it
-- (RFC 6749 §4.1.2).

-- When the code was exchanged; NULL while it is still unused.
ALTER TABLE oauth_codes ADD COLUMN used_at INTEGER;
-- 1 once someone presented the code again after it was used.
ALTER TABLE oauth_codes ADD COLUMN reused INTEGER NOT NULL DEFAULT 0;
-- The grant created from the code (set by the exchange, in the same
-- transaction that creates the grant).
ALTER TABLE oauth_codes ADD COLUMN grant_id INTEGER REFERENCES oauth_grants (id) ON DELETE SET NULL;
