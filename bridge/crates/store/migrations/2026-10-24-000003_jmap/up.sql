-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- JMAP objects that live only in the bridge: uploaded
-- blobs, identities and sent submissions. Times are Unix seconds (UTC).
-- Ids are JMAP ids, never reused: AUTOINCREMENT.

-- Blobs uploaded by the client (attachments, messages to import). They
-- expire: a blob is only needed until the email that uses it is created.
CREATE TABLE uploads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    media_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX uploads_by_account ON uploads (account_id, expires_at);
CREATE INDEX uploads_by_expiry ON uploads (expires_at);

-- Sending identities (RFC 8621 §6). Address lists are JSON arrays of
-- `EmailAddress` objects.
CREATE TABLE identities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    reply_to TEXT,
    bcc TEXT,
    text_signature TEXT NOT NULL,
    html_signature TEXT NOT NULL,
    may_delete INTEGER NOT NULL
) STRICT;

CREATE INDEX identities_by_account ON identities (account_id);

-- Sent messages (RFC 8621 §7), kept for `EmailSubmission/get` until they
-- expire. `email_id` and `identity_id` have no foreign key: the email or
-- identity may be destroyed while the submission is still listed.
CREATE TABLE email_submissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    identity_id INTEGER NOT NULL,
    email_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL,
    -- The SMTP envelope as JSON (`Envelope` of RFC 8621 §7).
    envelope TEXT NOT NULL,
    send_at INTEGER NOT NULL,
    -- The server's final reply to the message (`250 2.0.0 OK …`).
    smtp_reply TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX email_submissions_by_account ON email_submissions (account_id);
CREATE INDEX email_submissions_by_expiry ON email_submissions (expires_at);
