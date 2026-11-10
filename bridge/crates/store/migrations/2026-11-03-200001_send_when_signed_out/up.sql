-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- `send_when_signed_out` (0 or 1): the user asked for this held message to
-- go out even if they sign out before it is due (vendor property
-- `sendWhenSignedOut`, `[submission] send_while_signed_out`). While such
-- a message waits, the account keeps running and its password stays
-- sealed in `account_secrets`.
ALTER TABLE held_submissions ADD COLUMN send_when_signed_out INTEGER NOT NULL DEFAULT 0;
