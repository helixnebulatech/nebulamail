-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

DROP TABLE push_subscriptions;

CREATE TABLE push_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    device_client_id TEXT NOT NULL,
    sealed BLOB,
    types TEXT,
    expires INTEGER NOT NULL,
    verification_code TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX push_subscriptions_account ON push_subscriptions (account_id);
