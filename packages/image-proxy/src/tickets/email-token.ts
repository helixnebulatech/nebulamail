// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Email tokens: how the static build, which has no
// server of its own to sign tickets, shows preloaded images. When the MTA
// hook preloads an email it adds a header with a token to it:
//
//   X-Nebula-Preload: 2.<keyVersion>.<issuedAt>.<id: 64 hex>.<base64url HMAC>
//
// The id is random; the proxy keeps the hashes of that email's images
// under it (preload/records.ts). The app reads the header with JMAP and
// asks for `GET /p/<token>/<sha256(url)>`: the proxy checks the MAC and
// that the image belongs to that email, and serves it from storage only.
//
// - MAC: HMAC-SHA256 with `IMAGE_PROXY_KEYS` over a domain string of its
//   own (a ticket or preference MAC never verifies here), the key version,
//   `issuedAt` and the id.
// - `issuedAt` (Unix seconds, the proxy's clock) ties the token to its
//   delivery: the app only uses a token whose `issuedAt` matches the
//   email's `receivedAt`, which Stalwart sets and no sender controls. If
//   the hook didn't answer (proxy down, timeout), a header copied from an
//   older email stays in the message, and the app ignores it.
// - No expiry at the proxy: an email lasts; its record and images expire
//   with the cache (`IMAGE_CACHE_TTL_DAYS`).
// - Anyone who has the email has the token. It only gives that email's
//   own images, already downloaded by the proxy: never a download, never
//   another email's images.

import { fromBase64Url, toBase64Url } from "../base64";
import type { TicketKeys } from "./ticket";
import { TicketError } from "./ticket";

export const EMAIL_TOKEN_HEADER = "X-Nebula-Preload";

const DOMAIN = new TextEncoder().encode(
  "nebulamail/image-proxy/email-token/2\0"
);
const ID = /^[0-9a-f]{64}$/u;
/** The token, unanchored (for routes that embed it). */
export const EMAIL_TOKEN_SOURCE =
  "2\\.(?<version>[1-9][0-9]{0,8})\\.(?<issuedAt>[1-9][0-9]{0,11})\\.(?<id>[0-9a-f]{64})\\.(?<mac>[A-Za-z0-9_-]{43})";
export const EMAIL_TOKEN_PATTERN = new RegExp(`^${EMAIL_TOKEN_SOURCE}$`, "u");

const macInput = (
  keyVersion: number,
  issuedAt: number,
  id: string
): Uint8Array<ArrayBuffer> => {
  const idBytes = new TextEncoder().encode(id);
  const out = new Uint8Array(DOMAIN.length + 12 + idBytes.length);
  out.set(DOMAIN, 0);
  const view = new DataView(out.buffer);
  view.setUint32(DOMAIN.length, keyVersion);
  view.setBigUint64(DOMAIN.length + 4, BigInt(issuedAt));
  out.set(idBytes, DOMAIN.length + 12);
  return out;
};

/** A new random email id (64 hex). */
export const newEmailId = (): string =>
  Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) =>
    byte.toString(16).padStart(2, "0")
  ).join("");

/** `now` in milliseconds, as from `Date.now()`. */
export const signEmailToken = async (
  keys: TicketKeys,
  id: string,
  now: number
): Promise<string> => {
  if (!ID.test(id)) {
    throw new TicketError("email token: the id must be 64 hex characters");
  }
  const issuedAt = Math.floor(now / 1000);
  if (!Number.isSafeInteger(issuedAt) || issuedAt < 1) {
    throw new TicketError("email token: invalid time");
  }
  const key = keys.keys.get(keys.current);
  if (key === undefined) {
    throw new TicketError("image proxy keys: the current key is missing");
  }
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    macInput(keys.current, issuedAt, id)
  );
  return `2.${keys.current}.${issuedAt}.${id}.${toBase64Url(new Uint8Array(mac))}`;
};

/** The email id of an authentic token, else null. Never throws. */
export const verifyEmailToken = async (
  keys: TicketKeys,
  token: string
): Promise<string | null> => {
  const groups = EMAIL_TOKEN_PATTERN.exec(token)?.groups;
  if (
    groups?.version === undefined ||
    groups.issuedAt === undefined ||
    groups.id === undefined ||
    groups.mac === undefined
  ) {
    return null;
  }
  const keyVersion = Number(groups.version);
  const issuedAt = Number(groups.issuedAt);
  const key = keys.keys.get(keyVersion);
  const mac = fromBase64Url(groups.mac);
  if (key === undefined || mac?.length !== 32) {
    return null;
  }
  const authentic = await crypto.subtle.verify(
    "HMAC",
    key,
    mac,
    macInput(keyVersion, issuedAt, groups.id)
  );
  return authentic ? groups.id : null;
};
