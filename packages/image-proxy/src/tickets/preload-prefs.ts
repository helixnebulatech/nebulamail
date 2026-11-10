// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Signed preload-preference requests: the BFF tells the
// proxy "these addresses opt out of preload" (or asks their state) with
// `POST /preload/preferences`. The proxy cannot read the user's
// preferences, so the BFF, which already signs image tickets, signs this
// with the same keys (`IMAGE_PROXY_KEYS`).
//
// - `Authorization: Nebula-Preload <keyVersion>.<base64url HMAC>`; the MAC
//   is HMAC-SHA256 over a domain string (different from the tickets', so
//   a ticket MAC never verifies here and vice versa) and SHA-256 of the
//   exact body bytes. Checked with `crypto.subtle.verify` (constant time).
// - Body: `{ v: 1, addresses, optOut?, at, expiresAt }`. `expiresAt` at
//   most 2 minutes away; `at` orders choices (an older one is ignored, so
//   a replayed request cannot undo a newer choice).
// - The addresses must come from JMAP with the user's own token (session
//   and identities), never from the browser. The proxy hashes them and
//   stores only the hashes.

import { z } from "zod";

import { fromBase64Url, toBase64Url } from "../base64";
import type { TicketKeys } from "./ticket";
import { TICKET_CLOCK_SKEW_MS, TicketError } from "./ticket";

export const PREFERENCES_PATH = "/preload/preferences";
export const MAX_PREFERENCE_TTL_MS = 2 * 60 * 1000;
export const MAX_PREFERENCE_ADDRESSES = 50;
export const MAX_PREFERENCE_BODY_BYTES = 32 * 1024;

const SCHEME = "Nebula-Preload";
const DOMAIN = new TextEncoder().encode(
  "nebulamail/image-proxy/preload-prefs\0"
);
const AUTH =
  /^Nebula-Preload (?<version>[1-9][0-9]{0,8})\.(?<mac>[A-Za-z0-9_-]{43})$/u;

export const PreferenceBody = z.strictObject({
  addresses: z
    .array(z.string().min(3).max(320))
    .min(1)
    .max(MAX_PREFERENCE_ADDRESSES),
  at: z.number().int().positive(),
  expiresAt: z.number().int().positive(),
  optOut: z.boolean().optional(),
  v: z.literal(1),
});

export type PreferenceRequest = z.infer<typeof PreferenceBody>;

const macInput = async (body: string): Promise<Uint8Array<ArrayBuffer>> => {
  const hash = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(body))
  );
  const out = new Uint8Array(DOMAIN.length + hash.length);
  out.set(DOMAIN, 0);
  out.set(hash, DOMAIN.length);
  return out;
};

/** BFF side: the body and `Authorization` header of one request. */
export const signPreferenceRequest = async (
  keys: TicketKeys,
  input: {
    readonly addresses: readonly string[];
    /** Omit to only read the current state. */
    readonly optOut?: boolean;
    readonly now?: number;
  }
): Promise<{ readonly body: string; readonly authorization: string }> => {
  const key = keys.keys.get(keys.current);
  if (key === undefined) {
    throw new TicketError("image proxy keys: the current key is missing");
  }
  const now = input.now ?? Date.now();
  const parsed = PreferenceBody.safeParse({
    addresses: [...input.addresses],
    at: now,
    expiresAt: now + MAX_PREFERENCE_TTL_MS,
    ...(input.optOut === undefined ? {} : { optOut: input.optOut }),
    v: 1,
  });
  if (!parsed.success) {
    throw new TicketError("preload preference request is invalid");
  }
  const body = JSON.stringify(parsed.data);
  const mac = await crypto.subtle.sign("HMAC", key, await macInput(body));
  return {
    authorization: `${SCHEME} ${keys.current}.${toBase64Url(new Uint8Array(mac))}`,
    body,
  };
};

export type PreferenceVerdict =
  | { readonly ok: true; readonly request: PreferenceRequest }
  | { readonly ok: false; readonly reason: "unauthorized" | "invalid" };

/** Proxy side. Never throws; failures carry no input data. */
export const verifyPreferenceRequest = async (
  keys: TicketKeys,
  authorization: string | null,
  body: string,
  now: number
): Promise<PreferenceVerdict> => {
  const groups = AUTH.exec(authorization ?? "")?.groups;
  const key =
    groups?.version === undefined
      ? undefined
      : keys.keys.get(Number(groups.version));
  const mac = groups?.mac === undefined ? null : fromBase64Url(groups.mac);
  if (key === undefined || mac?.length !== 32) {
    return { ok: false, reason: "unauthorized" };
  }
  const authentic = await crypto.subtle.verify(
    "HMAC",
    key,
    new Uint8Array(mac),
    await macInput(body)
  );
  if (!authentic) {
    return { ok: false, reason: "unauthorized" };
  }
  let raw: unknown;
  try {
    raw = JSON.parse(body);
  } catch {
    return { ok: false, reason: "invalid" };
  }
  const parsed = PreferenceBody.safeParse(raw);
  if (
    !parsed.success ||
    parsed.data.expiresAt <= now ||
    parsed.data.expiresAt - now >
      MAX_PREFERENCE_TTL_MS + TICKET_CLOCK_SKEW_MS ||
    parsed.data.at > now + TICKET_CLOCK_SKEW_MS
  ) {
    return { ok: false, reason: "invalid" };
  }
  return { ok: true, request: parsed.data };
};
