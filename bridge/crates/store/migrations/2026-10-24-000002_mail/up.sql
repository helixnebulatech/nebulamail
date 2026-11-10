-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Mail data synced from IMAP. Only
-- metadata: message bodies are never stored. Times are Unix seconds (UTC).
--
-- Visibility: an email exists for JMAP only while it has at least one row in
-- `email_locations`, and a thread only while it has a visible email. Rows
-- without locations are kept a while when they have a server id, so a copy
-- that reappears (moved by another client, UIDVALIDITY reset) keeps its id.
--
-- Ids are JMAP ids, so they are never reused: AUTOINCREMENT (a plain
-- INTEGER PRIMARY KEY gives the id of a deleted last row again).

-- One row per IMAP folder.
CREATE TABLE mailboxes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- Full name in modified UTF-7 wire form. Names with invalid UTF-7 are
    -- kept as received, so they still round-trip to the server.
    imap_name TEXT NOT NULL,
    -- Display name: the part of the full name below the parent.
    name TEXT NOT NULL,
    -- Hierarchy delimiter, NULL for a flat namespace.
    delimiter TEXT,
    -- Nearest listed ancestor. The sync recomputes it before deleting rows,
    -- so SET NULL never hides a change.
    parent_id INTEGER REFERENCES mailboxes (id) ON DELETE SET NULL,
    -- `Role` code, at most one mailbox per role.
    role INTEGER,
    subscribed INTEGER NOT NULL,
    -- 0 for `\Noselect` / `\NonExistent` folders.
    selectable INTEGER NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- IMAP sync state. `highest_modseq` is only written after a complete
    -- sync, so an interrupted sync never skips changes.
    uid_validity INTEGER,
    uid_next INTEGER,
    highest_modseq INTEGER,
    messages INTEGER,
    synced_at INTEGER,
    UNIQUE (account_id, imap_name)
) STRICT;

CREATE UNIQUE INDEX mailboxes_by_role ON mailboxes (account_id, role) WHERE role IS NOT NULL;
CREATE INDEX mailboxes_by_parent ON mailboxes (parent_id);

CREATE TABLE threads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- THREADID / X-GM-THRID; NULL for threads computed by the bridge.
    server_id TEXT,
    UNIQUE (account_id, server_id)
) STRICT;

CREATE TABLE emails (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    -- EMAILID / OBJECTID / X-GM-MSGID. Copies with the same id are one email.
    server_id TEXT,
    message_id TEXT,
    -- JSON arrays of message ids (without angle brackets).
    in_reply_to TEXT NOT NULL,
    references_ids TEXT NOT NULL,
    subject TEXT,
    -- JSON arrays of `{"name", "email"}` (RFC 8621 EmailAddress), NULL when
    -- the header field is missing.
    from_addresses TEXT,
    to_addresses TEXT,
    cc_addresses TEXT,
    bcc_addresses TEXT,
    reply_to_addresses TEXT,
    sender_addresses TEXT,
    -- Date header: UTC seconds and the original offset in minutes.
    sent_at INTEGER,
    sent_offset INTEGER,
    -- INTERNALDATE.
    received_at INTEGER NOT NULL,
    size INTEGER NOT NULL,
    has_attachment INTEGER NOT NULL,
    preview TEXT,
    -- The fetched header fields, raw (for `header:List-Unsubscribe` etc.).
    headers BLOB NOT NULL,
    -- Sort keys: base subject and first sender, lowercased.
    sort_subject TEXT NOT NULL,
    sort_from TEXT NOT NULL,
    -- When the last location went away (rows kept for their server id).
    orphaned_at INTEGER,
    UNIQUE (account_id, server_id)
) STRICT;

CREATE INDEX emails_by_thread ON emails (thread_id);
CREATE INDEX emails_by_received ON emails (account_id, received_at);
CREATE INDEX emails_by_message_id ON emails (account_id, message_id);
CREATE INDEX emails_orphaned ON emails (account_id, orphaned_at) WHERE orphaned_at IS NOT NULL;

-- Where the copies of an email are: one row per (mailbox, UID).
CREATE TABLE email_locations (
    mailbox_id INTEGER NOT NULL REFERENCES mailboxes (id) ON DELETE CASCADE,
    uid INTEGER NOT NULL,
    email_id INTEGER NOT NULL REFERENCES emails (id) ON DELETE CASCADE,
    modseq INTEGER,
    PRIMARY KEY (mailbox_id, uid)
) STRICT, WITHOUT ROWID;

CREATE INDEX email_locations_by_email ON email_locations (email_id, mailbox_id);

-- JMAP keywords, lowercase (`$seen`, `$flagged`…).
CREATE TABLE email_keywords (
    email_id INTEGER NOT NULL REFERENCES emails (id) ON DELETE CASCADE,
    keyword TEXT NOT NULL,
    PRIMARY KEY (email_id, keyword)
) STRICT, WITHOUT ROWID;

CREATE INDEX email_keywords_by_keyword ON email_keywords (keyword, email_id);

-- Local threading without THREADID: every Message-ID, In-Reply-To and
-- References id seen points to its thread.
CREATE TABLE thread_refs (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    message_id TEXT NOT NULL,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX thread_refs_by_thread ON thread_refs (thread_id);
