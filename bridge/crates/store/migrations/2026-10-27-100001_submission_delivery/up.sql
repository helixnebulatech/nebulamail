-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Delivery reports (DSN, RFC 3464) and read receipts (MDN, RFC 8098) that
-- arrive for a submission fill in its `deliveryStatus` (RFC 8621 §7).

-- `Message-ID` of the email sent (without `<>`), to match reports that
-- carry no ENVID.
ALTER TABLE email_submissions ADD COLUMN message_id TEXT;
-- The ENVID sent with `MAIL FROM`, if any: then a report must quote it.
ALTER TABLE email_submissions ADD COLUMN envid TEXT;
-- Status per recipient as JSON; NULL until a report changes it.
ALTER TABLE email_submissions ADD COLUMN delivery TEXT;
-- Ids of the emails holding the reports, as JSON arrays.
ALTER TABLE email_submissions ADD COLUMN dsn_emails TEXT NOT NULL DEFAULT '[]';
ALTER TABLE email_submissions ADD COLUMN mdn_emails TEXT NOT NULL DEFAULT '[]';

CREATE INDEX email_submissions_by_message_id ON email_submissions (account_id, message_id);
CREATE INDEX email_submissions_by_envid ON email_submissions (account_id, envid);
