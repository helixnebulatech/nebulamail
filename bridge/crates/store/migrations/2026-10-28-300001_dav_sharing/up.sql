-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors
-- SPDX-License-Identifier: AGPL-3.0-only

-- Sharing calendars and address books.
-- `owner`: the owner's principal (a URL) when the collection is someone
-- else's, shared with the user; NULL for the user's own. `share_mode`: how
-- the user may share it (1 WebDAV ACL, 2 Calendar Server sharing; NULL:
-- not at all). `shares`: who it is shared with, a JSON array of
-- `[who, rights]` (who: a principal URL or a `mailto:`; rights: the bits
-- of `rights`), NULL when nobody (or the server does not say).
ALTER TABLE dav_collections ADD COLUMN owner TEXT;
ALTER TABLE dav_collections ADD COLUMN share_mode INTEGER;
ALTER TABLE dav_collections ADD COLUMN shares TEXT;
