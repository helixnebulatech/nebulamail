// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test-only: security/ssrf-vectors.json and a helper that runs one blocked
// URL vector through any adapter as a CORRECTLY SIGNED ticket (as if the
// signing key had leaked). Shared by the Node, Workers and Vercel tests.

import { readFileSync } from "node:fs";

import { z } from "zod";

import { APP_ORIGIN } from "./test-fixtures";
import { signTicketUnchecked } from "./tickets/ticket";
import type { TicketKeys } from "./tickets/ticket";

const Vector = z.object({
  dns: z
    .object({
      A: z.array(z.string()).optional(),
      AAAA: z.array(z.string()).optional(),
    })
    .optional(),
  expect: z.enum(["allow", "block"]),
  id: z.string(),
  input: z.string(),
});

export type UrlVector = z.infer<typeof Vector>;

const Vectors = z.object({ url: z.array(Vector) });

const vectors = Vectors.parse(
  JSON.parse(
    readFileSync(
      new URL("../../../security/ssrf-vectors.json", import.meta.url),
      "utf-8"
    )
  )
);

export const BLOCKED_VECTORS: readonly UrlVector[] = vectors.url.filter(
  (v) => v.expect === "block"
);

const Body = z.object({
  error: z.string().optional(),
  images: z
    .record(z.string(), z.object({ error: z.string().optional() }))
    .optional(),
});

/**
 * POST one signed ticket for `vector.input` to `/batch` and return what the
 * proxy answered: `{ status, error }` (request error or the image's error).
 */
export const runBlockedVector = async (
  keys: TicketKeys,
  vector: UrlVector,
  handle: (request: Request) => Promise<Response>,
  base = "http://proxy.test"
): Promise<{ readonly status: number; readonly error: string | undefined }> => {
  const t = await signTicketUnchecked(keys, {
    accountId: "acc1",
    emailId: "mail1",
    expiresAt: Date.now() + 60_000,
    url: vector.input,
  });
  const response = await handle(
    new Request(`${base}/batch`, {
      body: JSON.stringify({ tickets: [t] }),
      headers: { "content-type": "application/json", origin: APP_ORIGIN },
      method: "POST",
    })
  );
  const body = Body.parse(await response.json());
  return {
    error: body.error ?? body.images?.[t]?.error,
    status: response.status,
  };
};

/** What every adapter must answer for a blocked vector. */
export const expectedForBlocked = (
  vector: UrlVector
): { readonly status: number; readonly error: string } => {
  // Not absolute http(s): refused when the ticket is verified (400).
  // Otherwise the fetch is attempted and net-guard refuses it.
  const protocol = URL.parse(vector.input)?.protocol;
  return protocol === "http:" || protocol === "https:"
    ? { error: "blocked", status: 200 }
    : { error: "invalid-ticket", status: 400 };
};
