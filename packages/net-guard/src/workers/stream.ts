// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Body helpers for `fetch`-based runtimes (Workers, tests on Node).

import { GuardError } from "../errors";

/** The `fetch` we call. Workers' global `fetch` fits; tests pass a fake. */
export type WebFetch = (input: string, init: RequestInit) => Promise<Response>;

/** A stream read result, checked before use. */
interface Chunk {
  readonly done: boolean;
  readonly value?: unknown;
}

/**
 * Read a response body, stopping (and cancelling the stream) as soon as it
 * goes over `maxBytes`. The runtime has already decoded any gzip/br, so this
 * counts DECODED bytes: a compression bomb is cut at `maxBytes`. `charge` is
 * called with every chunk as it is read, kept or not; `false` stops with
 * `size`.
 */
export const readLimited = async (
  response: Response,
  maxBytes: number,
  charge?: (bytes: number) => boolean
): Promise<Uint8Array> => {
  const reader = response.body?.getReader();
  if (reader === undefined) {
    return new Uint8Array(0);
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      // Chunks arrive in order.
      // oxlint-disable-next-line no-await-in-loop
      const chunk: Chunk = await reader.read();
      if (chunk.done) {
        break;
      }
      if (!(chunk.value instanceof Uint8Array)) {
        throw new GuardError("network");
      }
      // Charged before the size check: those bytes were read either way.
      if (charge !== undefined && !charge(chunk.value.byteLength)) {
        throw new GuardError("size");
      }
      total += chunk.value.byteLength;
      if (total > maxBytes) {
        throw new GuardError("size");
      }
      chunks.push(chunk.value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {
      // Already failed; nothing else to release.
    });
    throw error;
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
};

/** Release a body we will not read. */
export const discard = async (response: Response): Promise<void> => {
  await response.body?.cancel().catch(() => {
    // Nothing to release.
  });
};
