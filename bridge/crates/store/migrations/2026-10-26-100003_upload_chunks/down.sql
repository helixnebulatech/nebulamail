-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Back to one BLOB per upload. Uploads are temporary (a day): only
-- complete ones of a single chunk are kept, the others are dropped.

CREATE TABLE uploads_old (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    media_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

INSERT INTO uploads_old (id, account_id, media_type, size, data, created_at, expires_at)
    SELECT u.id, u.account_id, u.media_type, u.size, coalesce(c.data, x''), u.created_at, u.expires_at
    FROM uploads u LEFT JOIN upload_chunks c ON c.upload_id = u.id AND c.seq = 0
    WHERE u.complete = 1
      AND (SELECT count(*) FROM upload_chunks WHERE upload_id = u.id) <= 1;

DELETE FROM sqlite_sequence WHERE name = 'uploads_old';
INSERT INTO sqlite_sequence (name, seq)
    SELECT 'uploads_old', seq FROM sqlite_sequence WHERE name = 'uploads';

DROP TABLE upload_chunks;
DROP TABLE uploads;
ALTER TABLE uploads_old RENAME TO uploads;

CREATE INDEX uploads_by_account ON uploads (account_id, expires_at);
CREATE INDEX uploads_by_expiry ON uploads (expires_at);
