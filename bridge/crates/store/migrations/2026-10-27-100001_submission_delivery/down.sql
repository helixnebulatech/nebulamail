-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

DROP INDEX email_submissions_by_envid;
DROP INDEX email_submissions_by_message_id;
ALTER TABLE email_submissions DROP COLUMN mdn_emails;
ALTER TABLE email_submissions DROP COLUMN dsn_emails;
ALTER TABLE email_submissions DROP COLUMN delivery;
ALTER TABLE email_submissions DROP COLUMN envid;
ALTER TABLE email_submissions DROP COLUMN message_id;
