// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Rate limits for the core's `rateLimit` hook: per
// IP, per account and global. The token buckets live in net-guard and are
// in memory, so they count per process (Node), per isolate (Workers) or per
// instance (Vercel): with N replicas the real limit is about N × the rule.
// That is accepted (no Redis). On Workers, Cloudflare's Rate
// Limiting binding replaces them per rule (`workers/rate-limit.ts`).

import {
  checkAddress,
  createRateLimiter as createBuckets,
} from "@nebulamail/net-guard";
import type { RateLimiter, RateLimitRules } from "@nebulamail/net-guard";

export type {
  BucketRule,
  RateLimiter,
  RateLimitRules,
} from "@nebulamail/net-guard";

/** 4 for IPv4, 6 for IPv6, 0 otherwise (like `node:net` `isIP`, any range). */
const ipFamily = (text: string): 0 | 4 | 6 => {
  const verdict = checkAddress(text);
  if (verdict.ok) {
    return verdict.family;
  }
  if (verdict.range === "invalid") {
    return 0;
  }
  return text.includes(":") ? 6 : 4;
};

/**
 * Rules by key prefix. `*:global` keys are the prefix itself: one bucket for
 * everybody, so a botnet spread over many IPs and accounts is still capped.
 * `*:global` is charged only after a valid credential (ticket, email token
 * or the BFF's signature): it caps real work, so unauthenticated requests
 * can't spend it. `*:ip` is charged before, to brake the HMAC work.
 */
export const DEFAULT_RATE_LIMITS: RateLimitRules = Object.freeze({
  // A user opening emails quickly: bursts of 30, then one email every 2 s.
  "batch:account": { capacity: 60, refillPerSecond: 1 },
  // Everybody together: 20 emails per second sustained, bursts of 600.
  "batch:global": { capacity: 600, refillPerSecond: 20 },
  "batch:ip": { capacity: 30, refillPerSecond: 0.5 },
  // Cache-mode image GETs are cheap (storage only) but come 60 per email.
  "image:global": { capacity: 30_000, refillPerSecond: 1000 },
  "image:ip": { capacity: 1200, refillPerSecond: 20 },
  // Incoming emails to preload (Stalwart MTA Hook): 2 per second sustained.
  "preload:global": { capacity: 120, refillPerSecond: 2 },
  // BFF → proxy preference requests (signed): a few per user action.
  "preload-prefs:global": { capacity: 200, refillPerSecond: 20 },
  // As generous as the global one: the whole BFF comes from one IP.
  "preload-prefs:ip": { capacity: 200, refillPerSecond: 20 },
});

export interface RateLimiterOptions {
  readonly rules?: RateLimitRules;
  /** Tracked keys; the least recently used are forgotten beyond this. */
  readonly maxKeys?: number;
  readonly now?: () => number;
}

/** `(key) => allowed`, in memory. Keys without a matching rule are always allowed. */
export const createRateLimiter = (
  options: RateLimiterOptions = {}
): ((key: string) => boolean) =>
  createBuckets({
    rules: options.rules ?? DEFAULT_RATE_LIMITS,
    ...(options.maxKeys === undefined ? {} : { maxKeys: options.maxKeys }),
    ...(options.now === undefined ? {} : { now: options.now }),
  });

/** A limiter that asks `limiter` and treats a throw or rejection as allowed. */
export const failOpen =
  (limiter: RateLimiter): ((key: string) => Promise<boolean>) =>
  async (key) => {
    try {
      return await limiter(key);
    } catch {
      return true;
    }
  };

const expandIPv6 = (address: string): string[] | null => {
  const halves = address.split("::");
  if (halves.length > 2) {
    return null;
  }
  const head = halves[0] === "" ? [] : (halves[0] ?? "").split(":");
  const tail =
    halves.length === 2 && halves[1] !== "" ? (halves[1] ?? "").split(":") : [];
  const fill = 8 - head.length - tail.length;
  if (fill < 0 || (halves.length === 1 && fill !== 0)) {
    return null;
  }
  return [...head, ...Array.from({ length: fill }, () => "0"), ...tail];
};

/**
 * Rate-limit identity of a client address: IPv4 as is (also when mapped into
 * IPv6), IPv6 by its /64, since one host usually controls a whole /64.
 */
export const clientKey = (address: string | undefined): string | undefined => {
  if (address === undefined) {
    return undefined;
  }
  const bare = address.replace(/%.*$/u, "");
  const mapped = /^::ffff:(?<v4>\d+\.\d+\.\d+\.\d+)$/iu.exec(bare)?.groups?.v4;
  if (mapped !== undefined && ipFamily(mapped) === 4) {
    return mapped;
  }
  if (ipFamily(bare) === 4) {
    return bare;
  }
  if (ipFamily(bare) === 6) {
    const groups = expandIPv6(bare.toLowerCase());
    if (groups !== null) {
      return `${groups
        .slice(0, 4)
        .map((g) => g.replace(/^0+(?=.)/u, ""))
        .join(":")}::/64`;
    }
  }
  return undefined;
};
