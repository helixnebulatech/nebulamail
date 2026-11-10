// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// DNS over HTTPS resolver (the JSON API, `application/dns-json`, as served by
// Cloudflare and Google) for runtimes without a DNS API, i.e. Workers. It only
// answers: every address is still validated by `resolveAndCheck`.

import type { DnsAnswer, Resolver } from "../resolve";
import { discard, readLimited } from "./stream";
import type { WebFetch } from "./stream";

export const DEFAULT_DOH_ENDPOINT = "https://cloudflare-dns.com/dns-query";

export interface DohResolverOptions {
  /** HTTPS endpoint of a DoH JSON API. Default: `DEFAULT_DOH_ENDPOINT`. */
  readonly endpoint?: string;
  /** Default: the global `fetch`. */
  readonly fetch?: WebFetch;
  /** Per query. Default 3 s. */
  readonly timeoutMs?: number;
  /** Answers are reused for min(record TTL, this). Default 60 s; 0 disables. */
  readonly maxCacheMs?: number;
  /** Cached names; the oldest are dropped beyond this. Default 1000. */
  readonly maxEntries?: number;
  readonly now?: () => number;
}

const RECORD_A = 1;
const RECORD_AAAA = 28;
const NOERROR = 0;
const NXDOMAIN = 3;
const MAX_ANSWER_BYTES = 64 * 1024;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

interface Records {
  readonly addresses: readonly string[];
  /** Smallest TTL of the records, in seconds. Infinity when there are none. */
  readonly ttl: number;
}

class DohError extends Error {
  override readonly name = "DohError";
}

/** Parse a DoH JSON answer. Throws on anything but NOERROR or NXDOMAIN. */
const parseAnswer = (raw: unknown, type: number): Records => {
  if (!isRecord(raw) || typeof raw.Status !== "number") {
    throw new DohError("malformed answer");
  }
  if (raw.Status === NXDOMAIN) {
    return { addresses: [], ttl: Number.POSITIVE_INFINITY };
  }
  if (raw.Status !== NOERROR) {
    throw new DohError("lookup failed");
  }
  const answers = raw.Answer ?? [];
  if (!Array.isArray(answers)) {
    throw new DohError("malformed answer");
  }
  const addresses: string[] = [];
  let ttl = Number.POSITIVE_INFINITY;
  for (const answer of answers) {
    if (!isRecord(answer)) {
      throw new DohError("malformed answer");
    }
    // CNAMEs (type 5) and other records are skipped: the A/AAAA records at
    // the end of the chain are in the same answer.
    if (answer.type === type) {
      if (typeof answer.data !== "string") {
        throw new DohError("malformed answer");
      }
      addresses.push(answer.data);
      ttl = Math.min(
        ttl,
        typeof answer.TTL === "number" && answer.TTL >= 0 ? answer.TTL : 0
      );
    }
  }
  return { addresses, ttl };
};

const checkEndpoint = (endpoint: string): URL => {
  const url = URL.parse(endpoint);
  if (
    url === null ||
    url.protocol !== "https:" ||
    url.username !== "" ||
    url.password !== ""
  ) {
    throw new TypeError("net-guard: the DoH endpoint must be an https URL");
  }
  return url;
};

/**
 * A `Resolver` over DoH: A and AAAA in parallel. A failed query (network,
 * non-200, SERVFAIL, malformed JSON) throws, which the guard turns into
 * `dns`. NXDOMAIN is an empty answer, also `dns` for the guard.
 */
export const createDohResolver = (
  options: DohResolverOptions = {}
): Resolver => {
  const endpoint = checkEndpoint(options.endpoint ?? DEFAULT_DOH_ENDPOINT);
  const doFetch: WebFetch =
    options.fetch ??
    (async (input, init) => await globalThis.fetch(input, init));
  const timeoutMs = options.timeoutMs ?? 3000;
  const maxCacheMs = options.maxCacheMs ?? 60_000;
  const maxEntries = options.maxEntries ?? 1000;
  const now = options.now ?? Date.now;
  const cache = new Map<string, { answer: DnsAnswer; expiresAt: number }>();

  const query = async (name: string, type: number): Promise<Records> => {
    const url = new URL(endpoint);
    url.searchParams.set("name", name);
    url.searchParams.set("type", String(type));
    const response = await doFetch(url.href, {
      headers: { accept: "application/dns-json" },
      method: "GET",
      // Workers reject `redirect: "error"` (TypeError at the edge). With
      // "manual" a 3xx is not followed and fails the status check below.
      redirect: "manual",
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (response.status !== 200) {
      await discard(response);
      throw new DohError("bad status");
    }
    const text = new TextDecoder().decode(
      await readLimited(response, MAX_ANSWER_BYTES)
    );
    return parseAnswer(JSON.parse(text), type);
  };

  const remember = (name: string, answer: DnsAnswer, ttl: number): void => {
    // Empty answers (Infinity TTL) are never cached.
    const keepMs = Number.isFinite(ttl) ? Math.min(ttl * 1000, maxCacheMs) : 0;
    if (keepMs <= 0) {
      return;
    }
    cache.set(name, { answer, expiresAt: now() + keepMs });
    for (const oldest of cache.keys()) {
      if (cache.size <= maxEntries) {
        break;
      }
      cache.delete(oldest);
    }
  };

  return async (hostname) => {
    // Names come lowercased and in punycode from WHATWG URL. A final dot is
    // dropped so `host.` and `host` share an entry (same name in DNS).
    const name = hostname.replace(/\.$/u, "");
    const hit = cache.get(name);
    if (hit !== undefined && hit.expiresAt > now()) {
      return hit.answer;
    }
    cache.delete(name);
    const [a, aaaa] = await Promise.all([
      query(name, RECORD_A),
      query(name, RECORD_AAAA),
    ]);
    const answer: DnsAnswer = { A: a.addresses, AAAA: aaaa.addresses };
    remember(name, answer, Math.min(a.ttl, aaaa.ttl));
    return answer;
  };
};
