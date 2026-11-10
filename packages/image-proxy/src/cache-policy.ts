// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Whether, and for how long, a downloaded image may be kept.
//
// A deliberate deviation from RFC 9111, decided by the user (2026-11-06)
// the sender's freshness is NOT followed. `no-store`,
// `no-cache`, `max-age`, `s-maxage`, `Expires` and `Age` don't shorten
// how long we keep an image in a cache of one account: an image kept for
// our own maximum (`cacheTtlMs`) is downloaded once, so the sender can't
// count how many times an email is opened (each download would tell it,
// through the proxy's IP). A remote image in an email is a tracker more
// often than content that changes; privacy wins.
//
// What protects other users is kept: in the cache shared between users
// (the `preload` entries, `PRELOAD_ACCOUNT`), a response marked `private`
// (RFC 9111 §5.2.2.7: "MUST NOT store" in a shared cache) or `no-store`
// (§5.2.2.5: it may be meant for one recipient) is never stored. An
// unreadable Cache-Control there counts as either: not stored.

/** The response fields that decide caching (lowercase names). */
export interface CacheHeaders {
  readonly "cache-control"?: string;
}

export const CACHE_HEADER_NAMES = ["cache-control"] as const;

/** Picks `CACHE_HEADER_NAMES` from a lowercase header record. */
export const cacheHeadersOf = (
  headers: Readonly<Record<string, string>>
): CacheHeaders =>
  Object.fromEntries(
    CACHE_HEADER_NAMES.flatMap((name) => {
      const value = headers[name];
      return value === undefined ? [] : [[name, value]];
    })
  );

/** `private`: dedicated to one account; `shared`: between users. */
export type CacheScope = "private" | "shared";

type Directives = ReadonlyMap<string, readonly (string | null)[]>;

/**
 * Cache-Control (RFC 9111 §5.2, list syntax of RFC 9110 §5.6.1): directive
 * names are case-insensitive, an argument is a token or a quoted-string.
 * Commas inside quoted strings don't split. Null for a value that isn't a
 * well-formed list (treated as "don't store" by the caller).
 */
const parseCacheControl = (value: string): Directives | null => {
  const directives = new Map<string, (string | null)[]>();
  const item =
    /^\s*(?<name>[!#$%&'*+.^`|~\w-]+)(?:\s*=\s*(?:(?<token>[!#$%&'*+.^`|~\w-]+)|"(?<quoted>(?:[^"\\]|\\.)*)"))?\s*(?:,|$)/u;
  let rest = value;
  while (rest.trim() !== "") {
    if (/^\s*,/u.test(rest)) {
      // Empty list elements are allowed (RFC 9110 §5.6.1).
      rest = rest.replace(/^\s*,/u, "");
      continue;
    }
    const match = item.exec(rest);
    const name = match?.groups?.name;
    if (match === null || name === undefined) {
      return null;
    }
    const argument =
      match.groups?.token ??
      match.groups?.quoted?.replaceAll(/\\(?<char>.)/gu, "$<char>") ??
      null;
    const key = name.toLowerCase();
    directives.set(key, [...(directives.get(key) ?? []), argument]);
    rest = rest.slice(match[0].length);
  }
  return directives;
};

/**
 * In the shared cache: never what is `private` (RFC 9111 §5.2.2.7) or
 * `no-store` (§5.2.2.5), and never with an unreadable Cache-Control.
 */
const keptOutOfShared = (raw: string | undefined): boolean => {
  if (raw === undefined) {
    return false;
  }
  const directives = parseCacheControl(raw);
  return (
    directives === null ||
    directives.has("private") ||
    directives.has("no-store")
  );
};

/**
 * How long (ms) this response may be kept, 0 if it must not be stored:
 * always our maximum, except what stays out of the shared cache (see the
 * top of this file for why the sender's freshness is not followed).
 */
export const storableTtlMs = (
  headers: CacheHeaders,
  scope: CacheScope,
  maxMs: number
): number =>
  scope === "shared" && keptOutOfShared(headers["cache-control"]) ? 0 : maxMs;

/**
 * How long a downloaded image may be stored, 0 for not at all: the
 * sender's caching fields within `maxMs`. A storage whose entries live a
 * fixed time whatever the ttl (R2, S3, Blob: `fixedTtlMs`) only gets what
 * may be kept that long.
 */
export const keepForMs = (
  image: { readonly cacheHeaders?: CacheHeaders },
  scope: CacheScope,
  options: {
    readonly maxMs: number;
    readonly fixedTtlMs?: number | undefined;
  }
): number => {
  const ttlMs = storableTtlMs(image.cacheHeaders ?? {}, scope, options.maxMs);
  return options.fixedTtlMs !== undefined && ttlMs < options.fixedTtlMs
    ? 0
    : ttlMs;
};
