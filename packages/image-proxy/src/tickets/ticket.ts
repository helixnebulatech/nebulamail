// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Signed image tickets. The BFF signs one ticket per image
// URL of an email it downloaded itself; the proxy only fetches URLs that
// arrive inside a valid ticket. Only WebCrypto is used, so this module runs
// in Node, Workers and browsers.
//
// Token: `1.<keyVersion>.<payload>.<mac>`
// - `payload`: base64url of the JSON array `[accountId, emailId, url, expiresAt]`
//   (the proxy needs the URL; the ids and expiry are needed to check the MAC).
// - `mac`: base64url of HMAC-SHA256 over a length-prefixed binary encoding of
//   (domain, token version, key version, accountId, emailId, sha256(url),
//   expiresAt). Length prefixes make it unambiguous: ("ab", "c") and
//   ("a", "bc") never produce the same input.
//
// Everything outside the token pattern is refused before any crypto, and the
// MAC is checked with `crypto.subtle.verify` (constant time).

import { z } from "zod";

import { fromBase64Url, toBase64Url } from "../base64";

/** Signing keys. `current` signs; every key in `keys` verifies (rotation). */
export interface TicketKeys {
  readonly current: number;
  readonly keys: ReadonlyMap<number, CryptoKey>;
}

export interface TicketClaims {
  readonly accountId: string;
  readonly emailId: string;
  /** Absolute http(s) image URL. Still checked by net-guard at fetch time. */
  readonly url: string;
  /** Unix time in milliseconds. */
  readonly expiresAt: number;
}

export type TicketFailure =
  | "malformed"
  | "unknown-key"
  | "bad-signature"
  | "expired";

export type TicketResult =
  | { readonly ok: true; readonly claims: TicketClaims }
  | { readonly ok: false; readonly reason: TicketFailure };

/** Longest lifetime `signTicket` accepts. */
export const MAX_TICKET_TTL_MS = 10 * 60 * 1000;
/** Clock skew tolerated between the BFF and the proxy for future expiries. */
export const TICKET_CLOCK_SKEW_MS = 60 * 1000;
/** Tokens longer than this are refused before any parsing or crypto. */
export const MAX_TICKET_LENGTH = 8 * 1024;
/** Same cap as mail-render: longer image URLs are blocked there anyway. */
export const MAX_TICKET_URL_LENGTH = 2048;
/** Characters a token can contain (URL-safe, no escaping needed). */
export const TICKET_PATTERN = /^[A-Za-z0-9_.-]+$/u;

const TOKEN_VERSION = 1;
const DOMAIN = new TextEncoder().encode("nebulamail/image-proxy/ticket\0");
const KEY_BYTES = 32;
const MAC_BYTES = 32;
/** Key versions: 1..999999999, no leading zeros (one spelling per version). */
const KEY_VERSION = /^[1-9][0-9]{0,8}$/u;
/** JMAP `Id` (RFC 8620 §1.2): 1–255 characters of base64url. */
const JMAP_ID = /^[A-Za-z0-9_-]{1,255}$/u;
const TOKEN =
  /^1\.(?<keyVersion>[1-9][0-9]{0,8})\.(?<payload>[A-Za-z0-9_-]+)\.(?<mac>[A-Za-z0-9_-]+)$/u;

/** Thrown by `parseTicketKeys` and `signTicket`. Never contains key material or URLs. */
export class TicketError extends Error {
  override readonly name = "TicketError";
}

const encoder = new TextEncoder();

const isHttpUrl = (url: string): boolean => {
  if (url.length === 0 || url.length > MAX_TICKET_URL_LENGTH) {
    return false;
  }
  const parsed = URL.parse(url);
  return (
    parsed !== null &&
    (parsed.protocol === "https:" || parsed.protocol === "http:")
  );
};

const validClaims = (claims: TicketClaims): boolean =>
  JMAP_ID.test(claims.accountId) &&
  JMAP_ID.test(claims.emailId) &&
  isHttpUrl(claims.url) &&
  Number.isSafeInteger(claims.expiresAt) &&
  claims.expiresAt > 0;

const sha256 = async (
  data: Uint8Array<ArrayBuffer>
): Promise<Uint8Array<ArrayBuffer>> =>
  new Uint8Array(await crypto.subtle.digest("SHA-256", data));

/** Hex SHA-256 of a URL; also used to name cache entries. */
export const hashUrl = async (url: string): Promise<string> =>
  Array.from(await sha256(encoder.encode(url)), (b) =>
    b.toString(16).padStart(2, "0")
  ).join("");

/** The exact bytes the MAC covers. */
const macInput = async (
  keyVersion: number,
  claims: TicketClaims
): Promise<Uint8Array<ArrayBuffer>> => {
  const account = encoder.encode(claims.accountId);
  const email = encoder.encode(claims.emailId);
  const urlHash = await sha256(encoder.encode(claims.url));
  const size =
    DOMAIN.length + 1 + 4 + 4 + account.length + 4 + email.length + 32 + 8;
  const out = new Uint8Array(size);
  const view = new DataView(out.buffer);
  let offset = 0;
  out.set(DOMAIN, offset);
  offset += DOMAIN.length;
  view.setUint8(offset, TOKEN_VERSION);
  offset += 1;
  view.setUint32(offset, keyVersion);
  offset += 4;
  for (const part of [account, email]) {
    view.setUint32(offset, part.length);
    offset += 4;
    out.set(part, offset);
    offset += part.length;
  }
  out.set(urlHash, offset);
  offset += 32;
  view.setBigUint64(offset, BigInt(claims.expiresAt));
  return out;
};

const encodePayload = (claims: TicketClaims): string =>
  toBase64Url(
    encoder.encode(
      JSON.stringify([
        claims.accountId,
        claims.emailId,
        claims.url,
        claims.expiresAt,
      ])
    )
  );

const Payload = z.tuple([z.string(), z.string(), z.string(), z.number()]);

const decodePayload = (text: string): TicketClaims | null => {
  const bytes = fromBase64Url(text);
  if (bytes === null) {
    return null;
  }
  let json: unknown;
  try {
    json = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    return null;
  }
  const parsed = Payload.safeParse(json);
  if (!parsed.success) {
    return null;
  }
  const [accountId, emailId, url, expiresAt] = parsed.data;
  const claims: TicketClaims = { accountId, emailId, expiresAt, url };
  // One encoding per claim set: the payload must be what we would have written.
  return validClaims(claims) && encodePayload(claims) === text ? claims : null;
};

/**
 * Parse `IMAGE_PROXY_KEYS`: `"<version>:<base64url of 32 bytes>,..."`.
 * The first entry signs; all entries verify. Throws `TicketError` on an empty
 * list, a malformed entry or a duplicate version.
 */
export const parseTicketKeys = async (value: string): Promise<TicketKeys> => {
  const entries = value.split(",").map((entry) => entry.trim());
  const keys = new Map<number, CryptoKey>();
  let current: number | undefined;
  for (const [index, entry] of entries.entries()) {
    const separator = entry.indexOf(":");
    const versionText = entry.slice(0, separator);
    const secret =
      separator > 0 ? fromBase64Url(entry.slice(separator + 1)) : null;
    if (
      !KEY_VERSION.test(versionText) ||
      secret === null ||
      secret.length !== KEY_BYTES
    ) {
      // Never echo the entry: it may be a secret.
      throw new TicketError(
        `image proxy keys: entry ${index + 1} is not "<version>:<base64url of 32 bytes>"`
      );
    }
    const version = Number(versionText);
    if (keys.has(version)) {
      throw new TicketError(
        `image proxy keys: version ${version} appears twice`
      );
    }
    const signs = current === undefined;
    // Keys are imported in order: the first one is the signing key.
    // oxlint-disable-next-line no-await-in-loop
    const key = await crypto.subtle.importKey(
      "raw",
      secret,
      { hash: "SHA-256", name: "HMAC" },
      false,
      signs ? ["sign", "verify"] : ["verify"]
    );
    keys.set(version, key);
    current ??= version;
  }
  if (current === undefined) {
    throw new TicketError("image proxy keys: no key configured");
  }
  return { current, keys };
};

/** A fresh random key entry for `IMAGE_PROXY_KEYS`, e.g. `"1:3q2-…"`. */
export const generateTicketKeyEntry = (version: number): string => {
  if (!KEY_VERSION.test(String(version))) {
    throw new TicketError("key version must be an integer from 1 to 999999999");
  }
  return `${version}:${toBase64Url(crypto.getRandomValues(new Uint8Array(KEY_BYTES)))}`;
};

/**
 * Signs without the claim and lifetime checks. Internal: tests use it to
 * forge tickets a leaked key could produce. Not exported from the package.
 */
export const signTicketUnchecked = async (
  keys: TicketKeys,
  claims: TicketClaims
): Promise<string> => {
  const key = keys.keys.get(keys.current);
  if (key === undefined) {
    throw new TicketError("image proxy keys: the current key is missing");
  }
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    await macInput(keys.current, claims)
  );
  return `${TOKEN_VERSION}.${keys.current}.${encodePayload(claims)}.${toBase64Url(new Uint8Array(mac))}`;
};

/**
 * Sign a ticket for one image of one email. Throws `TicketError` if the ids
 * are not JMAP ids, the URL is not absolute http(s) or is too long, or the
 * expiry is in the past or more than `MAX_TICKET_TTL_MS` away.
 */
export const signTicket = async (
  keys: TicketKeys,
  claims: TicketClaims,
  now: number = Date.now()
): Promise<string> => {
  if (!validClaims(claims)) {
    throw new TicketError("ticket claims are invalid");
  }
  if (claims.expiresAt <= now || claims.expiresAt - now > MAX_TICKET_TTL_MS) {
    throw new TicketError(
      `ticket expiry must be within ${MAX_TICKET_TTL_MS} ms from now`
    );
  }
  const token = await signTicketUnchecked(keys, claims);
  if (token.length > MAX_TICKET_LENGTH) {
    throw new TicketError("ticket would be too long");
  }
  return token;
};

const malformed: TicketResult = { ok: false, reason: "malformed" };

/** Verify a token. Never throws; failures carry no input data. */
export const verifyTicket = async (
  keys: TicketKeys,
  token: string,
  now: number
): Promise<TicketResult> => {
  if (typeof token !== "string" || token.length > MAX_TICKET_LENGTH) {
    return malformed;
  }
  const groups = TOKEN.exec(token)?.groups;
  const versionText = groups?.keyVersion;
  const payloadText = groups?.payload;
  const macText = groups?.mac;
  if (
    versionText === undefined ||
    payloadText === undefined ||
    macText === undefined
  ) {
    return malformed;
  }
  const keyVersion = Number(versionText);
  const key = keys.keys.get(keyVersion);
  if (key === undefined) {
    return { ok: false, reason: "unknown-key" };
  }
  const mac = fromBase64Url(macText);
  const claims = decodePayload(payloadText);
  if (mac === null || mac.length !== MAC_BYTES || claims === null) {
    return malformed;
  }
  const authentic = await crypto.subtle.verify(
    "HMAC",
    key,
    new Uint8Array(mac),
    await macInput(keyVersion, claims)
  );
  if (!authentic) {
    return { ok: false, reason: "bad-signature" };
  }
  if (claims.expiresAt <= now) {
    return { ok: false, reason: "expired" };
  }
  if (claims.expiresAt - now > MAX_TICKET_TTL_MS + TICKET_CLOCK_SKEW_MS) {
    // Authentic but longer-lived than `signTicket` allows: not ours.
    return malformed;
  }
  return { claims, ok: true };
};

/** Image URL the email iframe uses in cache mode: `${proxyOrigin}/i/${token}`. */
export const imageUrl = (proxyOrigin: string, token: string): string => {
  const origin = URL.parse(proxyOrigin)?.origin;
  if (origin !== proxyOrigin) {
    throw new TicketError(
      "proxy origin must be a bare origin, e.g. https://img.example.com"
    );
  }
  if (token.length > MAX_TICKET_LENGTH || !TICKET_PATTERN.test(token)) {
    throw new TicketError("not a ticket");
  }
  return `${origin}/i/${token}`;
};
