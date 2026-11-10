-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- `accounts.id` with AUTOINCREMENT: the JMAP accountId is `a` + this id, so
-- an id must never be given again to a new account after a row is deleted.
--
-- SQLite can't add AUTOINCREMENT to a column: the table is rebuilt (the
-- 12-step procedure of https://sqlite.org/lang_altertable.html#otheralter).
-- Many tables reference `accounts` with ON DELETE CASCADE, and dropping it
-- with foreign keys on would delete all their rows: so foreign keys are
-- switched off around the rebuild. That PRAGMA does nothing inside a
-- transaction, which is why this migration runs without one
-- (`metadata.toml`) and opens its own.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE accounts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- IMAP login as first accepted. NOCASE: "Alice" and "alice" are one account
    -- (only if the server accepts the password for both spellings).
    login TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- Address of the default identity.
    email TEXT NOT NULL,
    -- The account's data key (DEK), wrapped with the master key (KEK).
    wrapped_dek BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL
) STRICT;

INSERT INTO accounts_new (id, login, email, wrapped_dek, created_at, last_login_at)
    SELECT id, login, email, wrapped_dek, created_at, last_login_at FROM accounts;

DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;

COMMIT;

PRAGMA foreign_keys = ON;
