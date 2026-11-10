-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

DROP TABLE held_submissions;
DROP INDEX email_submissions_by_undo_status;
ALTER TABLE email_submissions DROP COLUMN undo_status;
