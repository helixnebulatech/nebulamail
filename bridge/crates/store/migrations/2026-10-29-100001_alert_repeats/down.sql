-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

DELETE FROM calendar_alerts_fired WHERE repetition <> 0;
DROP INDEX calendar_alerts_fired_once;
CREATE UNIQUE INDEX calendar_alerts_fired_once
    ON calendar_alerts_fired (resource_id, alert_id, occurrence);
ALTER TABLE calendar_alerts_fired DROP COLUMN repetition;
