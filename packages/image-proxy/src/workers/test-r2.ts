// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test-only in-memory R2 bucket with the behaviour we rely on: `uploaded`
// dates, content type metadata, batch delete and paginated list.

import type { R2BucketLike, R2ObjectLike } from "./r2-storage";

export interface FakeR2 extends R2BucketLike {
  readonly objects: Map<
    string,
    { bytes: Uint8Array; type: string; uploaded: Date }
  >;
  readonly calls: string[];
}

export const createFakeR2 = (
  now: () => number = Date.now,
  pageSize = 1000
): FakeR2 => {
  const objects = new Map<
    string,
    { bytes: Uint8Array; type: string; uploaded: Date }
  >();
  const calls: string[] = [];
  return {
    calls,
    delete: async (keys) => {
      await Promise.resolve();
      calls.push("delete");
      for (const key of Array.isArray(keys) ? keys : [keys]) {
        objects.delete(key);
      }
    },
    get: async (key): Promise<R2ObjectLike | null> => {
      await Promise.resolve();
      calls.push("get");
      const entry = objects.get(key);
      if (entry === undefined) {
        return null;
      }
      return {
        arrayBuffer: async () => {
          await Promise.resolve();
          return Uint8Array.from(entry.bytes).buffer;
        },
        httpMetadata: { contentType: entry.type },
        size: entry.bytes.byteLength,
        uploaded: entry.uploaded,
      };
    },
    list: async ({ prefix, cursor, limit }) => {
      await Promise.resolve();
      calls.push("list");
      const keys = [...objects.keys()]
        .filter((key) => key.startsWith(prefix))
        .toSorted();
      // Key-based cursor: the page after the last listed key.
      const rest =
        cursor === undefined ? keys : keys.filter((key) => key > cursor);
      const page = rest.slice(0, Math.min(limit ?? 1000, pageSize));
      const truncated = rest.length > page.length;
      return {
        objects: page.map((key) => ({
          key,
          uploaded: objects.get(key)?.uploaded ?? new Date(0),
        })),
        truncated,
        ...(truncated ? { cursor: page.at(-1) ?? "" } : {}),
      };
    },
    objects,
    put: async (key, value, options) => {
      await Promise.resolve();
      calls.push("put");
      objects.set(key, {
        bytes: new Uint8Array(value),
        type: options.httpMetadata.contentType,
        uploaded: new Date(now()),
      });
      return {};
    },
  };
};
