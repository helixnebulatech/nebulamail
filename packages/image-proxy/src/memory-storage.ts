// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// In-memory `ImageStorage` for tests and single-process development. Not for
// production: it is lost on restart and bounded only by `maxEntries`.

import type { ImageStorage, StoredImage } from "./config";

export interface MemoryStorageOptions {
  readonly now?: () => number;
  /** Oldest entries are dropped beyond this. Default 1000. */
  readonly maxEntries?: number;
}

export const createMemoryStorage = (
  options: MemoryStorageOptions = {}
): ImageStorage => {
  const now = options.now ?? Date.now;
  const maxEntries = options.maxEntries ?? 1000;
  const entries = new Map<string, { image: StoredImage; expiresAt: number }>();
  return {
    delete: async (key) => {
      await Promise.resolve();
      entries.delete(key);
    },
    get: async (key) => {
      await Promise.resolve();
      const entry = entries.get(key);
      if (entry === undefined) {
        return null;
      }
      if (entry.expiresAt <= now()) {
        entries.delete(key);
        return null;
      }
      return { ...entry.image, expiresAt: entry.expiresAt };
    },
    put: async (key, image, ttlMs) => {
      await Promise.resolve();
      entries.delete(key);
      entries.set(key, {
        expiresAt: now() + ttlMs,
        image: { bytes: Uint8Array.from(image.bytes), type: image.type },
      });
      for (const oldest of entries.keys()) {
        if (entries.size <= maxEntries) {
          break;
        }
        entries.delete(oldest);
      }
    },
  };
};
