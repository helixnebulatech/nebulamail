-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Accounts, OAuth and the JMAP change log.
-- Times are Unix seconds (UTC). Secrets and tokens are never stored in clear:
-- secrets are sealed by nebula-crypto, tokens are stored as SHA-256 hashes.

-- One row per mail account (an IMAP login on the configured server).
CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
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

-- Sealed secrets of an account (IMAP password…). Deleted when the last
-- session of the account ends.
CREATE TABLE account_secrets (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    field INTEGER NOT NULL,
    sealed BLOB NOT NULL,
    PRIMARY KEY (account_id, field)
) STRICT;

-- OAuth authorization codes: single use, a few minutes.
CREATE TABLE oauth_codes (
    code_hash BLOB PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    scope TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

-- One grant per login. Its refresh tokens form a family: reusing an old one
-- revokes the whole grant.
CREATE TABLE oauth_grants (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- The session ends at this time whatever happens.
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
) STRICT;

CREATE INDEX oauth_grants_by_account ON oauth_grants (account_id);

CREATE TABLE oauth_tokens (
    token_hash BLOB PRIMARY KEY,
    grant_id INTEGER NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    -- 1 = access token, 2 = refresh token.
    kind INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    -- Refresh tokens: when it was exchanged for a new one.
    used_at INTEGER
) STRICT;

CREATE INDEX oauth_tokens_by_grant ON oauth_tokens (grant_id);

-- JMAP state per account and data type: a counter that goes up with every
-- change. `oldest` is the first sequence number still in `changes`.
CREATE TABLE states (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    data_type INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    oldest INTEGER NOT NULL,
    PRIMARY KEY (account_id, data_type)
) STRICT;

-- One row per change, for `*/changes`.
CREATE TABLE changes (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    data_type INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    object_id INTEGER NOT NULL,
    -- 1 = created, 2 = updated, 3 = destroyed.
    kind INTEGER NOT NULL
) STRICT;

CREATE INDEX changes_by_seq ON changes (account_id, data_type, seq);
