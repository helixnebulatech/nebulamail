-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- The change log is trimmed now: `log_rows` counts the rows
-- of each (account, type) in `changes`, so the periodic trim only looks at
-- the ones over the limit instead of counting them every time.
ALTER TABLE states ADD COLUMN log_rows INTEGER NOT NULL DEFAULT 0;

-- `EmailDelivery` (11) is push only: nobody reads its rows.
DELETE FROM changes WHERE data_type = 11;

UPDATE states SET log_rows = (
    SELECT count(*) FROM changes c
    WHERE c.account_id = states.account_id AND c.data_type = states.data_type
);
