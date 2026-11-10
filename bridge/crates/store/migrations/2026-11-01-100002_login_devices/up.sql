-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Browsers that logged in to an account: the
-- SHA-256 of a random cookie. A known browser gets its own failed-login
-- window, so others failing on the login can't lock its owner out. One
-- browser can be known for several accounts (the same cookie).
CREATE TABLE login_devices (
    token_hash BLOB NOT NULL,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    last_login_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (token_hash, account_id)
) STRICT;

CREATE INDEX login_devices_by_account ON login_devices (account_id, last_login_at);
