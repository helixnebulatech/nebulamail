// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which images belong to an email with an email token
// (tickets/email-token.ts): `{ v: 1, images: [sha256(url), …] }`,
// kept in the image storage itself, so it expires and is swept with the
// images (`IMAGE_CACHE_TTL_DAYS`). Only hashes: never a URL or an address.
// Its key, `storageKey("*mail", id)`, can't be an account's ("*" is never
// in a JMAP id) and its type isn't an image, so `GET /i/` and `/batch`
// never serve it.

import { z } from "zod";

import type { ImageStorage } from "../config";
import { storageKey } from "../storage-key";

/** The content type records are stored with (every storage keeps it: `stored-type.ts`). */
export const PRELOAD_RECORD_TYPE = "application/vnd.nebula.preload-record+json";

export const RECORD_ACCOUNT = "*mail";
const HASH = /^[0-9a-f]{64}$/u;
/** More than any email can have (`maxImagesPerEmail`). */
const MAX_IMAGES = 1000;

const Record = z.strictObject({
  images: z.array(z.string().regex(HASH)).max(MAX_IMAGES),
  v: z.literal(1),
});

export const recordKey = async (id: string): Promise<string> =>
  await storageKey(RECORD_ACCOUNT, id);

export const putRecord = async (
  storage: ImageStorage,
  id: string,
  images: readonly string[],
  ttlMs: number
): Promise<void> => {
  const record = Record.parse({ images: [...new Set(images)], v: 1 });
  await storage.put(
    await recordKey(id),
    {
      bytes: new TextEncoder().encode(JSON.stringify(record)),
      type: PRELOAD_RECORD_TYPE,
    },
    ttlMs
  );
};

/** The image hashes of an email, or null (none, expired, unreadable). */
export const getRecord = async (
  storage: ImageStorage,
  id: string
): Promise<ReadonlySet<string> | null> => {
  const stored = await storage.get(await recordKey(id));
  if (stored?.type !== PRELOAD_RECORD_TYPE) {
    return null;
  }
  try {
    const record = Record.safeParse(
      JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(stored.bytes))
    );
    return record.success ? new Set(record.data.images) : null;
  } catch {
    return null;
  }
};
