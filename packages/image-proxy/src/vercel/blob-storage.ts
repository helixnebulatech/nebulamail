// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vercel Blob `ImageStorage`.
//
// - The client is injected: pass the `@vercel/blob` module (`import * as blob
//   from "@vercel/blob"`). We do not depend on it, because it pulls ~30
//   packages; only deployments that use Blob install it.
// - Blobs are **private** (`access: "private"`): never readable by URL
//   without the store token. Pathnames are `img/v1/<sha256>/<sha256>` (the
//   core's key), so a listing shows hashes only, never URLs or account ids.
// - Retention: a blob counts as expired `maxAgeMs` after upload (Blob's own
//   `uploadedAt`), 30 days by default, any value the admin sets. Expired
//   blobs are deleted on read and by
//   `sweep()` (a Vercel Cron job; Blob has no lifecycle rules).

import type { StoredImage, SweepableStorage } from "../config";
import { DEFAULT_LIMITS } from "../config";
import { STORAGE_KEY_PATTERN } from "../storage-key";
import { storedType } from "../stored-type";

/** What `get` returns: `@vercel/blob`'s `GetBlobResult`, only the parts we read. */
export interface BlobGetResult {
  readonly statusCode: number;
  readonly stream: ReadableStream<Uint8Array> | null;
  readonly blob: {
    readonly contentType: string | null;
    readonly size: number | null;
    readonly uploadedAt: Date;
  };
}

/** The functions of `@vercel/blob` (2.x) we use. The module object fits. */
export interface VercelBlobApi {
  readonly put: (
    pathname: string,
    body: Blob,
    options: {
      readonly access: "private";
      readonly addRandomSuffix: false;
      readonly allowOverwrite: true;
      readonly contentType: string;
      /** Seconds; 60 is Blob's minimum (used for the preload state). */
      readonly cacheControlMaxAge?: number;
    }
  ) => Promise<unknown>;
  readonly get: (
    pathname: string,
    options: { readonly access: "private" }
  ) => Promise<BlobGetResult | null>;
  readonly del: (urlOrPathname: string[] | string) => Promise<void>;
  readonly list: (options: {
    readonly prefix: string;
    readonly cursor?: string;
    readonly limit?: number;
  }) => Promise<{
    readonly blobs: readonly {
      readonly url: string;
      readonly uploadedAt: Date;
    }[];
    readonly cursor?: string;
    readonly hasMore: boolean;
  }>;
}

export interface BlobStorageOptions {
  /** Retention since upload. Default 30 days; no upper bound. */
  readonly maxAgeMs?: number;
  /** Blobs bigger than this are treated as missing. Default 5 MB. */
  readonly maxBytes?: number;
  readonly now?: () => number;
}

const PREFIX = "img/";

const pathnameOf = (key: string): string => {
  if (!STORAGE_KEY_PATTERN.test(key)) {
    throw new TypeError("image proxy: unexpected storage key");
  }
  return `${PREFIX}${key.replaceAll(":", "/")}`;
};

interface Chunk {
  readonly done: boolean;
  readonly value?: unknown;
}

/** Read a stream; `null` (and cancel) if it is not bytes or goes over `max`. */
export const readAtMost = async (
  stream: ReadableStream<Uint8Array>,
  max: number
): Promise<Uint8Array | null> => {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    // Chunks arrive in order.
    // oxlint-disable-next-line no-await-in-loop
    const chunk: Chunk = await reader.read();
    if (chunk.done) {
      break;
    }
    if (
      !(chunk.value instanceof Uint8Array) ||
      total + chunk.value.byteLength > max
    ) {
      // oxlint-disable-next-line no-await-in-loop
      await reader.cancel();
      return null;
    }
    total += chunk.value.byteLength;
    chunks.push(chunk.value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
};

export const createBlobStorage = (
  api: VercelBlobApi,
  options: BlobStorageOptions = {}
): SweepableStorage => {
  const maxAgeMs = options.maxAgeMs ?? DEFAULT_LIMITS.cacheTtlMs;
  if (!Number.isSafeInteger(maxAgeMs) || maxAgeMs <= 0) {
    throw new TypeError("image proxy: maxAgeMs must be a positive integer");
  }
  const maxBytes = options.maxBytes ?? DEFAULT_LIMITS.maxBytesPerImage;
  const now = options.now ?? Date.now;
  const expired = (uploadedAt: Date): boolean =>
    uploadedAt.getTime() + maxAgeMs <= now();

  return {
    delete: async (key) => {
      await api.del(pathnameOf(key));
    },
    get: async (key): Promise<StoredImage | null> => {
      const pathname = pathnameOf(key);
      const result = await api.get(pathname, { access: "private" });
      if (result === null) {
        return null;
      }
      const { blob, stream } = result;
      if (result.statusCode !== 200 || stream === null) {
        await stream?.cancel();
        return null;
      }
      if (expired(blob.uploadedAt)) {
        await stream.cancel();
        await api.del(pathname);
        return null;
      }
      const type = storedType(blob.contentType ?? undefined);
      if (type === undefined || (blob.size ?? 0) > maxBytes) {
        await stream.cancel();
        return null;
      }
      const bytes = await readAtMost(stream, maxBytes);
      return bytes === null
        ? null
        : { bytes, expiresAt: blob.uploadedAt.getTime() + maxAgeMs, type };
    },
    // Entries live `maxAgeMs` from upload, whatever the ttl (`fixedTtlMs`).
    fixedTtlMs: maxAgeMs,
    put: async (key, image) => {
      await api.put(pathnameOf(key), new Blob([Uint8Array.from(image.bytes)]), {
        access: "private",
        addRandomSuffix: false,
        allowOverwrite: true,
        contentType: image.type,
      });
    },
    // Deletes page by page. If the listing cursor were offset-based, a
    // deleted page could make one run skip some entries; the next run (cron)
    // deletes them.
    sweep: async () => {
      let deleted = 0;
      let cursor: string | undefined;
      do {
        // Pages are sequential: each needs the previous cursor.
        // oxlint-disable-next-line no-await-in-loop
        const page = await api.list({
          limit: 1000,
          prefix: PREFIX,
          ...(cursor === undefined ? {} : { cursor }),
        });
        const old = page.blobs
          .filter((blob) => expired(blob.uploadedAt))
          .map((blob) => blob.url);
        if (old.length > 0) {
          // oxlint-disable-next-line no-await-in-loop
          await api.del(old);
          deleted += old.length;
        }
        cursor = page.hasMore ? page.cursor : undefined;
      } while (cursor !== undefined);
      return deleted;
    },
  };
};
