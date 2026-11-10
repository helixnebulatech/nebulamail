-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Calendar series indexed before calcard's `truncated`:
-- an endless series with an EXDATE was indexed with an end (`utc_end`).
-- The text is the same on the server, so nothing would fetch it again:
-- forget their etag (fetched again, and a fetch of the same text now
-- compares the index too) and their collection's token and ctag (a full
-- listing names them).
UPDATE dav_collections SET sync_token = NULL, ctag = NULL
    WHERE kind = 1 AND id IN (
        SELECT collection_id FROM dav_resources
            WHERE recurring AND utc_end IS NOT NULL AND NOT invalid
    );
UPDATE dav_resources SET etag = NULL
    WHERE recurring AND utc_end IS NOT NULL AND NOT invalid
        AND collection_id IN (SELECT id FROM dav_collections WHERE kind = 1);
