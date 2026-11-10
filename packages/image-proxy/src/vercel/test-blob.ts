// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test-only in-memory stand-in for the `@vercel/blob` functions we use.

import type { BlobGetResult, VercelBlobApi } from "./blob-storage";

export interface FakeBlob extends VercelBlobApi {
  readonly blobs: Map<
    string,
    { bytes: Uint8Array; type: string; uploadedAt: Date; access: string }
  >;
}

const URL_BASE = "https://store.private.blob.vercel-storage.test/";

const pathOf = (urlOrPathname: string): string =>
  urlOrPathname.startsWith(URL_BASE)
    ? urlOrPathname.slice(URL_BASE.length)
    : urlOrPathname;

export const createFakeBlob = (
  now: () => number = Date.now,
  pageSize = 1000
): FakeBlob => {
  const blobs: FakeBlob["blobs"] = new Map();
  return {
    blobs,
    del: async (urls) => {
      await Promise.resolve();
      for (const url of Array.isArray(urls) ? urls : [urls]) {
        blobs.delete(pathOf(url));
      }
    },
    get: async (pathname, options): Promise<BlobGetResult | null> => {
      await Promise.resolve();
      const entry = blobs.get(pathname);
      if (entry === undefined || entry.access !== options.access) {
        return null;
      }
      return {
        blob: {
          contentType: entry.type,
          size: entry.bytes.byteLength,
          uploadedAt: entry.uploadedAt,
        },
        statusCode: 200,
        stream: new Blob([Uint8Array.from(entry.bytes)]).stream(),
      };
    },
    list: async ({ prefix, cursor, limit }) => {
      await Promise.resolve();
      const names = [...blobs.keys()]
        .filter((name) => name.startsWith(prefix))
        .toSorted();
      const rest =
        cursor === undefined ? names : names.filter((name) => name > cursor);
      const page = rest.slice(0, Math.min(limit ?? 1000, pageSize));
      const hasMore = rest.length > page.length;
      return {
        blobs: page.map((name) => ({
          uploadedAt: blobs.get(name)?.uploadedAt ?? new Date(0),
          url: `${URL_BASE}${name}`,
        })),
        hasMore,
        ...(hasMore ? { cursor: page.at(-1) ?? "" } : {}),
      };
    },
    put: async (pathname, body, options) => {
      blobs.set(pathname, {
        access: options.access,
        bytes: new Uint8Array(await body.arrayBuffer()),
        type: options.contentType,
        uploadedAt: new Date(now()),
      });
      return {};
    },
  };
};
