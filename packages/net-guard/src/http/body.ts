// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Response bodies for the pinned adapters (Node and Bun): size limits on the
// raw AND the decoded bytes, and our own decompression.
//
// We send `Accept-Encoding: identity` (images are already compressed, and it
// avoids work), but a hostile server can compress anyway to build a bomb.
// Passing compressed bytes through would hand the bomb to the next consumer,
// so we decode ourselves with a hard limit on the OUTPUT size (and on the
// input size). Unknown or stacked encodings are refused.

import { pipeline } from "node:stream";
import type { Transform } from "node:stream";
import {
  createBrotliDecompress,
  createGunzip,
  createInflate,
  createZstdDecompress,
} from "node:zlib";

import { GuardError } from "../errors";

/** Response headers we never pass on. */
export const DROPPED_HEADERS: ReadonlySet<string> = new Set([
  "connection",
  "content-encoding",
  "content-length",
  "keep-alive",
  "set-cookie",
  "set-cookie2",
  "transfer-encoding",
]);

/** A decoder for `Content-Encoding`, `null` for none, or `"unsupported"`. */
export const decoderFor = (
  header: string | undefined
): Transform | null | "unsupported" => {
  const codings = (header ?? "")
    .split(",")
    .map((c) => c.trim().toLowerCase())
    .filter((c) => c !== "" && c !== "identity");
  if (codings.length === 0) {
    return null;
  }
  if (codings.length > 1) {
    return "unsupported";
  }
  switch (codings[0] ?? "") {
    case "gzip":
    case "x-gzip": {
      return createGunzip();
    }
    case "deflate": {
      return createInflate();
    }
    case "br": {
      return createBrotliDecompress();
    }
    case "zstd": {
      return createZstdDecompress();
    }
    default: {
      return "unsupported";
    }
  }
};

// A generator cannot be an arrow function.
// oxlint-disable-next-line func-style
async function* limited(
  source: AsyncIterable<Uint8Array>,
  maxBytes: number,
  charge?: (bytes: number) => boolean
): AsyncGenerator<Uint8Array> {
  let total = 0;
  for await (const chunk of source) {
    // Charged before the size check: those bytes were read either way.
    if (charge !== undefined && !charge(chunk.byteLength)) {
      throw new GuardError("size");
    }
    total += chunk.byteLength;
    if (total > maxBytes) {
      throw new GuardError("size");
    }
    yield chunk;
  }
}

/**
 * Read a body, decoding it if needed; more than `maxBytes` (raw or decoded)
 * is `size`. `charge` is called with every DECODED chunk as it is read, kept
 * or not (a shared budget: bytes read count even if the result is then
 * refused); `false` stops with `size`.
 */
export const readBody = async (
  body: AsyncIterable<Uint8Array>,
  decoder: Transform | null,
  maxBytes: number,
  charge?: (bytes: number) => boolean
): Promise<Uint8Array> => {
  // Without a decoder the raw bytes are the decoded ones: one limiter.
  let decoded: AsyncIterable<Uint8Array> = body;
  if (decoder !== null) {
    const raw = limited(body, maxBytes);
    // `pipeline` propagates errors and destroys both ends on failure.
    decoded = pipeline(raw, decoder, () => {
      // Errors surface through the iteration below.
    });
  }
  const chunks: Uint8Array[] = [];
  for await (const chunk of limited(decoded, maxBytes, charge)) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
};
