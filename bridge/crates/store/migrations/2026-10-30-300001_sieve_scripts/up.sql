-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Sieve scripts seen on the account's ManageSieve server:
-- only what JMAP needs to keep ids stable (ManageSieve knows scripts by
-- name) and to tell changes apart. The content stays on the server; here
-- only the first 64 bits of its SHA-256, in hex.
CREATE TABLE sieve_scripts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    active INTEGER NOT NULL,
    content_hash TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX sieve_scripts_by_name ON sieve_scripts (account_id, name);
