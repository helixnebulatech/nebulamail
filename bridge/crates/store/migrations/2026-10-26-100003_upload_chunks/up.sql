-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Uploads in chunks, so a blob is written while it arrives and read while
-- it is sent, never whole in memory. An upload is written
-- by several short transactions (one per chunk, so the write lock isn't
-- held while the client sends); until the last one it is `complete = 0`
-- and nobody can read it, but its bytes already count against the
-- account's cap.
--
-- No table references `uploads`, so it can be rebuilt inside the
-- migration's transaction. Its AUTOINCREMENT counter is carried over:
-- upload ids are blob ids and are never reused.

CREATE TABLE uploads_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    media_type TEXT NOT NULL,
    -- Bytes stored so far (all of them once complete).
    size INTEGER NOT NULL,
    complete INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    -- A pending upload gets a short expiry, so one that never finishes
    -- (client gone, bridge restarted) is deleted by the next cleanup.
    expires_at INTEGER NOT NULL
) STRICT;

INSERT INTO uploads_new (id, account_id, media_type, size, complete, created_at, expires_at)
    SELECT id, account_id, media_type, size, 1, created_at, expires_at FROM uploads;

-- The old data waits here until the chunks table exists: it can only be
-- created (referencing the new `uploads`) after the rename.
CREATE TEMP TABLE upload_data AS SELECT id, data FROM uploads;

DELETE FROM sqlite_sequence WHERE name = 'uploads_new';
INSERT INTO sqlite_sequence (name, seq)
    SELECT 'uploads_new', seq FROM sqlite_sequence WHERE name = 'uploads';

DROP TABLE uploads;
ALTER TABLE uploads_new RENAME TO uploads;

CREATE INDEX uploads_by_account ON uploads (account_id, expires_at);
CREATE INDEX uploads_by_expiry ON uploads (expires_at);

CREATE TABLE upload_chunks (
    upload_id INTEGER NOT NULL REFERENCES uploads (id) ON DELETE CASCADE,
    -- 0, 1, 2… in order.
    seq INTEGER NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (upload_id, seq)
) STRICT;

INSERT INTO upload_chunks (upload_id, seq, data)
    SELECT id, 0, data FROM temp.upload_data WHERE length(data) > 0;

DROP TABLE temp.upload_data;
