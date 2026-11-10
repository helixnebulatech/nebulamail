// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Preload state in Vercel Blob: private blobs named
// `pre/v1/<kind>/<hex>` (hashes only), outside `img/`, so the cron sweep
// never deletes an opt-out. `cacheControlMaxAge: 60` (Blob's minimum):
// an overwritten record can be read stale for up to a minute, so an
// opt-out may let one more email be preloaded in that minute.

import type { PreloadStateStore } from "../preload/state";
import { checkStateKey, MAX_STATE_BYTES, stateKeyOf } from "../preload/state";
import { readAtMost } from "./blob-storage";
import type { VercelBlobApi } from "./blob-storage";

const PREFIX = "pre/v1/";

const pathnameOf = (key: string): string =>
  `${PREFIX}${checkStateKey(key).replace(":", "/")}`;

export const createBlobStateStore = (
  api: VercelBlobApi
): PreloadStateStore => ({
  delete: async (key) => {
    await api.del(pathnameOf(key));
  },
  get: async (key) => {
    const result = await api.get(pathnameOf(key), { access: "private" });
    if (result === null) {
      return null;
    }
    const { stream } = result;
    if (result.statusCode !== 200 || stream === null) {
      await stream?.cancel();
      return null;
    }
    const bytes = await readAtMost(stream, MAX_STATE_BYTES);
    if (bytes === null) {
      return null;
    }
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return null;
    }
  },
  list: async function* list(kind) {
    const prefix = `${PREFIX}${kind}/`;
    let cursor: string | undefined;
    do {
      // Pages are sequential: each needs the previous cursor.
      // oxlint-disable-next-line no-await-in-loop
      const page = await api.list({
        limit: 1000,
        prefix,
        ...(cursor === undefined ? {} : { cursor }),
      });
      for (const blob of page.blobs) {
        // `list` gives URLs; their path is the blob's pathname.
        const pathname = URL.parse(blob.url)?.pathname.slice(1) ?? "";
        const key = pathname.startsWith(prefix)
          ? stateKeyOf(kind, pathname.slice(prefix.length))
          : undefined;
        if (key !== undefined) {
          yield key;
        }
      }
      cursor = page.hasMore ? page.cursor : undefined;
    } while (cursor !== undefined);
  },
  put: async (key, value) => {
    const bytes = new TextEncoder().encode(value);
    if (bytes.byteLength > MAX_STATE_BYTES) {
      throw new RangeError("image proxy: state record too large");
    }
    await api.put(pathnameOf(key), new Blob([bytes]), {
      access: "private",
      addRandomSuffix: false,
      allowOverwrite: true,
      cacheControlMaxAge: 60,
      contentType: "application/json",
    });
  },
});
