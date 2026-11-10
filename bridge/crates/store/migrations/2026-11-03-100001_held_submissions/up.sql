-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Held submissions (RFC 4865 FUTURERELEASE: undo send with HOLDFOR,
-- scheduled send with HOLDUNTIL). `undo_status` is the JMAP one, plus
-- `sending` while the bridge is handing the message to the SMTP server
-- (shown as `pending`, but it can no longer be canceled). Rows sent at once
-- are `final`, as every row before this migration.
ALTER TABLE email_submissions ADD COLUMN undo_status TEXT NOT NULL DEFAULT 'final';

-- The message of each `pending` or `sending` submission, until it goes out
-- or is canceled. Never in clear: sealed with the account's data key and
-- bound to the submission (`nebula-crypto`, field `HeldMessage`). `size` is
-- the length of the message in clear, for the per-account limits.
-- `next_attempt_at` is when the scheduler looks at it next: `send_at` at
-- first, later after a temporary failure or while the account is not
-- signed in. `sent_copy` (0 or 1): append a copy to Sent once it is sent.
CREATE TABLE held_submissions (
    submission_id INTEGER PRIMARY KEY REFERENCES email_submissions (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    message BLOB NOT NULL,
    size INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    sent_copy INTEGER NOT NULL
) STRICT;

CREATE INDEX held_submissions_by_attempt ON held_submissions (next_attempt_at);
CREATE INDEX held_submissions_by_account ON held_submissions (account_id);
CREATE INDEX email_submissions_by_undo_status ON email_submissions (undo_status);
