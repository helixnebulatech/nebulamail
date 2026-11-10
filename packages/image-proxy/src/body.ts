// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Request bodies with a hard size limit, whatever Content-Length says.

export type BodyResult =
  | { readonly ok: true; readonly text: string }
  | { readonly ok: false; readonly code: "body-too-large" | "invalid-body" };

/** A body stream read result, checked before use (bodies are untyped). */
interface StreamChunk {
  readonly done: boolean;
  readonly value?: unknown;
}

/** Read at most `max` bytes of the body as UTF-8 (fatal on bad UTF-8). */
export const readLimited = async (
  request: Request,
  max: number
): Promise<BodyResult> => {
  const reader = request.body?.getReader();
  if (reader === undefined) {
    return { ok: true, text: "" };
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  let failure: "body-too-large" | "invalid-body" | undefined;
  while (failure === undefined) {
    // Stream chunks arrive in order.
    // oxlint-disable-next-line no-await-in-loop
    const chunk: StreamChunk = await reader.read();
    if (chunk.done) {
      break;
    }
    const { value } = chunk;
    if (!(value instanceof Uint8Array)) {
      failure = "invalid-body";
    } else if (total + value.byteLength > max) {
      failure = "body-too-large";
    } else {
      total += value.byteLength;
      chunks.push(value);
    }
  }
  if (failure !== undefined) {
    await reader.cancel();
    return { code: failure, ok: false };
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return {
      ok: true,
      text: new TextDecoder("utf-8", { fatal: true }).decode(body),
    };
  } catch {
    return { code: "invalid-body", ok: false };
  }
};

const digest = async (text: string): Promise<Uint8Array> =>
  new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))
  );

/**
 * Constant-time comparison of two secrets (WebCrypto only, so it runs on
 * every runtime): both are hashed first, so length and content leak nothing.
 */
export const sameSecret = async (
  given: string,
  expected: string
): Promise<boolean> => {
  const [a, b] = await Promise.all([digest(given), digest(expected)]);
  let diff = 0;
  for (const [i, byte] of a.entries()) {
    // oxlint-disable-next-line no-bitwise -- constant-time comparison.
    diff |= byte ^ (b[i] ?? 0);
  }
  return diff === 0;
};
