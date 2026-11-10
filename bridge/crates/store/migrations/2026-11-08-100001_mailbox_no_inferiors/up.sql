-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- `no_inferiors` (0 or 1): the server listed the mailbox with
-- `\Noinferiors` (RFC 9051 §7.3.1: "It is not possible for any child levels
-- of hierarchy to exist under this name"), so JMAP's `mayCreateChild` is
-- false (RFC 8621 §2). The next LIST sync fills it in.
ALTER TABLE mailboxes ADD COLUMN no_inferiors INTEGER NOT NULL DEFAULT 0;
