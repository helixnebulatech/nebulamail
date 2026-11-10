// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// JSON request bodies for the BFF endpoints, read with a hard size limit.
// The body is read as a stream and dropped as soon as it passes `maxBytes`
// (`readLimited`), so a huge or endless body costs at most that much memory,
// whatever Content-Length says.

import { readLimited } from "@nebulamail/image-proxy/body";

export type JsonBody =
  | { readonly ok: true; readonly value: unknown }
  | { readonly ok: false; readonly status: 400 | 413 };

const DECIMAL = /^\d+$/u;

export const readJsonBody = async (
  request: Request,
  maxBytes: number,
  { emptyAs }: { readonly emptyAs?: unknown } = {}
): Promise<JsonBody> => {
  const length = request.headers.get("content-length");
  if (length !== null) {
    const trimmed = length.trim();
    if (!DECIMAL.test(trimmed)) {
      await request.body?.cancel();
      return { ok: false, status: 400 };
    }
    if (Number(trimmed) > maxBytes) {
      await request.body?.cancel();
      return { ok: false, status: 413 };
    }
  }
  const body = await readLimited(request, maxBytes);
  if (!body.ok) {
    return { ok: false, status: body.code === "body-too-large" ? 413 : 400 };
  }
  if (body.text === "") {
    return emptyAs === undefined
      ? { ok: false, status: 400 }
      : { ok: true, value: emptyAs };
  }
  try {
    return { ok: true, value: JSON.parse(body.text) as unknown };
  } catch {
    return { ok: false, status: 400 };
  }
};

/** The answer for a body `readJsonBody` refused. */
export const bodyError = (status: 400 | 413): { readonly error: string } => ({
  error: status === 413 ? "too-large" : "invalid",
});
