// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Cloudflare R2 `ImageStorage`, through a Worker binding.
//
// - Object keys are `img/` + the core's key (`v1:<sha256>:<sha256>`), so a
//   bucket listing shows hashes only, never URLs or account ids.
// - Retention: an object counts as expired `maxAgeMs` after it was uploaded
//   (R2's own `uploaded` date); 30 days by default, any value the admin sets.
//   Expired objects are deleted
//   on read and by `sweep()` (the Worker's cron). Add an R2 lifecycle rule
//   on `img/` as a second net (README).
// - Bytes are re-checked by the core before serving (type and size).

import type { StoredImage, SweepableStorage } from "../config";
import { DEFAULT_LIMITS } from "../config";
import { STORAGE_KEY_PATTERN } from "../storage-key";
import { storedType } from "../stored-type";

/** The parts of an R2 object we read. Matches `R2ObjectBody`. */
export interface R2ObjectLike {
  readonly size: number;
  readonly uploaded: Date;
  readonly httpMetadata?: { readonly contentType?: string };
  readonly arrayBuffer: () => Promise<ArrayBuffer>;
}

/** The parts of the R2 binding we use. Matches `R2Bucket`. */
export interface R2BucketLike {
  readonly get: (key: string) => Promise<R2ObjectLike | null>;
  readonly put: (
    key: string,
    value: ArrayBuffer,
    options: { readonly httpMetadata: { readonly contentType: string } }
  ) => Promise<unknown>;
  readonly delete: (keys: string | string[]) => Promise<void>;
  readonly list: (options: {
    readonly prefix: string;
    readonly cursor?: string;
    readonly limit?: number;
  }) => Promise<{
    readonly objects: readonly {
      readonly key: string;
      readonly uploaded: Date;
    }[];
    readonly truncated: boolean;
    readonly cursor?: string;
  }>;
}

export interface R2StorageOptions {
  /** Retention since upload. Default 30 days; no upper bound. */
  readonly maxAgeMs?: number;
  /** Objects bigger than this are treated as missing. Default 5 MB. */
  readonly maxBytes?: number;
  readonly now?: () => number;
}

const PREFIX = "img/";

/** Checks of the structural binding type, for env validation. */
export const isR2Bucket = (value: unknown): value is R2BucketLike =>
  typeof value === "object" &&
  value !== null &&
  ["get", "put", "delete", "list"].every(
    (name) => typeof Reflect.get(value, name) === "function"
  );

const objectKey = (key: string): string => {
  if (!STORAGE_KEY_PATTERN.test(key)) {
    throw new TypeError("image proxy: unexpected storage key");
  }
  return `${PREFIX}${key}`;
};

export const createR2Storage = (
  bucket: R2BucketLike,
  options: R2StorageOptions = {}
): SweepableStorage => {
  const maxAgeMs = options.maxAgeMs ?? DEFAULT_LIMITS.cacheTtlMs;
  if (!Number.isSafeInteger(maxAgeMs) || maxAgeMs <= 0) {
    throw new TypeError("image proxy: maxAgeMs must be a positive integer");
  }
  const maxBytes = options.maxBytes ?? DEFAULT_LIMITS.maxBytesPerImage;
  const now = options.now ?? Date.now;
  const expired = (uploaded: Date): boolean =>
    uploaded.getTime() + maxAgeMs <= now();

  return {
    get: async (key): Promise<StoredImage | null> => {
      const name = objectKey(key);
      const object = await bucket.get(name);
      if (object === null) {
        return null;
      }
      if (expired(object.uploaded)) {
        await bucket.delete(name);
        return null;
      }
      const type = storedType(object.httpMetadata?.contentType);
      if (type === undefined || object.size > maxBytes) {
        return null;
      }
      return {
        bytes: new Uint8Array(await object.arrayBuffer()),
        expiresAt: object.uploaded.getTime() + maxAgeMs,
        type,
      };
    },
    // Entries live `maxAgeMs` from upload, whatever the ttl (`fixedTtlMs`).
    fixedTtlMs: maxAgeMs,
    /** `ttlMs` above `maxAgeMs` is cut to it; entries live `maxAgeMs` from upload. */
    put: async (key, image) => {
      // A copy: the caller's buffer may be a view into a bigger one.
      const bytes = Uint8Array.from(image.bytes);
      await bucket.put(objectKey(key), bytes.buffer, {
        httpMetadata: { contentType: image.type },
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
        const page = await bucket.list({
          limit: 1000,
          prefix: PREFIX,
          ...(cursor === undefined ? {} : { cursor }),
        });
        const old = page.objects
          .filter((object) => expired(object.uploaded))
          .map((object) => object.key);
        if (old.length > 0) {
          // oxlint-disable-next-line no-await-in-loop
          await bucket.delete(old);
          deleted += old.length;
        }
        cursor = page.truncated ? page.cursor : undefined;
      } while (cursor !== undefined);
      return deleted;
    },
  };
};
