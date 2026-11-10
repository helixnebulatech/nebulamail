-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- SQLite can't drop a column with a foreign key: rebuild the table (no
-- other table references it). Used codes are dropped.
CREATE TABLE oauth_codes_old (
    code_hash BLOB PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    scope TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;
INSERT INTO oauth_codes_old
    SELECT code_hash, account_id, client_id, redirect_uri, code_challenge, scope, expires_at
    FROM oauth_codes WHERE used_at IS NULL;
DROP TABLE oauth_codes;
ALTER TABLE oauth_codes_old RENAME TO oauth_codes;
