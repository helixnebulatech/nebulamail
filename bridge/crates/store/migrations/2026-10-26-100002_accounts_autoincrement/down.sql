-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE accounts_old (
    id INTEGER PRIMARY KEY,
    login TEXT NOT NULL UNIQUE COLLATE NOCASE,
    email TEXT NOT NULL,
    wrapped_dek BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL
) STRICT;

INSERT INTO accounts_old (id, login, email, wrapped_dek, created_at, last_login_at)
    SELECT id, login, email, wrapped_dek, created_at, last_login_at FROM accounts;

DROP TABLE accounts;
ALTER TABLE accounts_old RENAME TO accounts;

COMMIT;

PRAGMA foreign_keys = ON;
