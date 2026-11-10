-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Web Push from the bridge (extension of 2026-10-28).

-- JMAP PushSubscription (RFC 8620 §7.2). The push URL and the browser's
-- keys are a capability: `sealed` holds them encrypted with the
-- account's data key, bound to the row id (NULL only inside the
-- transaction that creates the row). `types` is a comma-separated list
-- of JMAP type names (NULL: every type). Times are Unix seconds.
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

-- Secrets of the bridge itself (the VAPID private key): sealed with a data
-- key of their own, stored wrapped by the master key like an account's.
CREATE TABLE system_secrets (
    name TEXT PRIMARY KEY NOT NULL,
    wrapped_dek BLOB NOT NULL,
    sealed BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
