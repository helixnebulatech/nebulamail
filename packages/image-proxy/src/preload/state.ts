// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Small JSON records that `preload` keeps next to the image cache
// per-recipient usage (quota), per-image references (which
// recipients still use a shared preloaded entry) and per-recipient
// preferences (opt-out). Keys are `<kind>:<64 hex>`: a recipient hash
// (`recipients.ts`) or a URL hash, never an address or a URL.
//
// Stores: memory (tests), disk (`node/disk-state.ts`), S3
// (`s3/s3-storage.ts`), Vercel Blob (`vercel/blob-state.ts`). They live
// apart from the images (`pre/` instead of `img/`), so the image sweep,
// the disk size cap and bucket lifecycle rules never delete an opt-out.

export const STATE_KEY_PATTERN = /^(?:usage|refs|prefs):[0-9a-f]{64}$/u;

/** Largest record any store reads back; bigger ones count as missing. */
export const MAX_STATE_BYTES = 4 * 1024 * 1024;

export type SweptStateKind = "usage" | "refs";

export interface PreloadStateStore {
  readonly get: (key: string) => Promise<string | null>;
  readonly put: (key: string, value: string) => Promise<void>;
  readonly delete: (key: string) => Promise<void>;
  /**
   * Every key of this kind (`<kind>:<64 hex>`), for the sweep. Keys added
   * or deleted while listing may or may not be seen.
   */
  readonly list: (kind: SweptStateKind) => AsyncIterable<string>;
}

/** The key, if `name` is `<64 hex>` (plus `suffix`): what `list` keeps. */
export const stateKeyOf = (
  kind: SweptStateKind,
  name: string,
  suffix = ""
): string | undefined => {
  if (!name.endsWith(suffix)) {
    return undefined;
  }
  const hex = name.slice(0, name.length - suffix.length);
  return /^[0-9a-f]{64}$/u.test(hex) ? `${kind}:${hex}` : undefined;
};

export const checkStateKey = (key: string): string => {
  if (!STATE_KEY_PATTERN.test(key)) {
    throw new TypeError("image proxy: unexpected state key");
  }
  return key;
};

/** In memory: tests and development only (an opt-out is lost on restart). */
export const createMemoryStateStore = (): PreloadStateStore => {
  const records = new Map<string, string>();
  return {
    delete: async (key) => {
      await Promise.resolve();
      records.delete(checkStateKey(key));
    },
    get: async (key) => {
      await Promise.resolve();
      return records.get(checkStateKey(key)) ?? null;
    },
    list: async function* list(kind) {
      await Promise.resolve();
      // Deleting while a Map is iterated is safe (the sweep does).
      for (const key of records.keys()) {
        if (key.startsWith(`${kind}:`)) {
          yield key;
        }
      }
    },
    put: async (key, value) => {
      await Promise.resolve();
      if (new TextEncoder().encode(value).byteLength > MAX_STATE_BYTES) {
        throw new RangeError("image proxy: state record too large");
      }
      records.set(checkStateKey(key), value);
    },
  };
};
