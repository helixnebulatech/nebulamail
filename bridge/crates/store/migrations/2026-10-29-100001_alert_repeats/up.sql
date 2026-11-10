-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Repeated alarms (`REPEAT`/`DURATION` of `VALARM`): each
-- ring of an alarm is sent once, so the ring number is part of the key.
-- 0 is the first ring (every row from before).
ALTER TABLE calendar_alerts_fired ADD COLUMN repetition INTEGER NOT NULL DEFAULT 0;
DROP INDEX calendar_alerts_fired_once;
CREATE UNIQUE INDEX calendar_alerts_fired_once
    ON calendar_alerts_fired (resource_id, alert_id, occurrence, repetition);
