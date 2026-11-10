-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- A Web Push subscription belongs to the login session (OAuth grant) that
-- created it, as RFC 8620 §7.2 says: only that session sees and changes
-- it, and it goes when the session is revoked or deleted. The rows from
-- before have no session: they are dropped (the web app subscribes again
-- when it opens).
DROP TABLE push_subscriptions;

CREATE TABLE push_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    grant_id INTEGER NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    device_client_id TEXT NOT NULL,
    sealed BLOB,
    types TEXT,
    expires INTEGER NOT NULL,
    verification_code TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX push_subscriptions_account ON push_subscriptions (account_id);
CREATE INDEX push_subscriptions_grant ON push_subscriptions (grant_id);
