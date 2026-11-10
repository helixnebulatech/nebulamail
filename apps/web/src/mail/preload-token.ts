// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Option D of the static build: the image proxy's MTA hook
// preloads an email's images when it arrives and adds a signed token to
// it (image-proxy `tickets/email-token.ts`). With it the browser asks the
// proxy for those images, without any token of the user's:
//   <proxy>/p/<email token>/<sha256 of the image URL>
// The proxy only serves that email's own images, from storage.

import { z } from "zod";

/** The proxy the static build was built with, or "" (none). */
export const PRELOAD_PROXY_ORIGIN = import.meta.env.NEBULA_IMAGE_PROXY_ORIGIN;

export const PRELOAD_TOKEN_PROPERTY =
  "header:X-Nebula-Preload:asText:all" as const;

const TOKEN =
  /^2\.[1-9][0-9]{0,8}\.(?<issuedAt>[1-9][0-9]{0,11})\.[0-9a-f]{64}\.[\w-]{43}$/u;
const Texts = z.array(z.string()).nullable().catch(null);

/**
 * How far the token's `issuedAt` (the proxy's clock, at DATA) may be from
 * the email's `receivedAt` (Stalwart's clock, at delivery): a little after
 * it (clocks differ), up to an hour before it (a delivery that waited in
 * the queue). Outside that, the header was copied from another email.
 */
const MAX_CLOCK_SKEW_MS = 5 * 60 * 1000;
const MAX_DELIVERY_DELAY_MS = 60 * 60 * 1000;

/**
 * The email's token: the first header, if it was made for this delivery.
 * The hook removes any the email already had; when the hook didn't answer
 * (proxy down, timeout), one copied from an older email can stay, and its
 * `issuedAt` gives it away. A token only ever gives its own email's images.
 */
export const preloadTokenOf = (
  email: Readonly<Record<string, unknown>>
): string | null => {
  const first = Texts.parse(email[PRELOAD_TOKEN_PROPERTY])?.[0]?.trim();
  const issuedAt = TOKEN.exec(first ?? "")?.groups?.issuedAt;
  const receivedAt =
    typeof email.receivedAt === "string"
      ? Date.parse(email.receivedAt)
      : Number.NaN;
  if (
    first === undefined ||
    issuedAt === undefined ||
    Number.isNaN(receivedAt)
  ) {
    return null;
  }
  const delay = receivedAt - Number(issuedAt) * 1000;
  return delay >= -MAX_CLOCK_SKEW_MS && delay <= MAX_DELIVERY_DELAY_MS
    ? first
    : null;
};

const hex = (bytes: ArrayBuffer): string =>
  Array.from(new Uint8Array(bytes), (byte) =>
    byte.toString(16).padStart(2, "0")
  ).join("");

/** Image URL in the email → its preloaded copy on the proxy. */
export const preloadedUrls = async (
  origin: string,
  token: string,
  urls: readonly string[]
): Promise<ReadonlyMap<string, string>> =>
  new Map(
    await Promise.all(
      urls.map(
        async (url) =>
          [
            url,
            `${origin}/p/${token}/${hex(
              await crypto.subtle.digest(
                "SHA-256",
                new TextEncoder().encode(url)
              )
            )}`,
          ] as const
      )
    )
  );
