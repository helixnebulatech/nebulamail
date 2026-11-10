// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Rate limits on Workers (no
// Redis): Cloudflare's native Rate Limiting binding, one binding per rule,
// shared by every isolate of the Worker in a Cloudflare location
// (`[[ratelimits]]` in wrangler.toml). Rules without a binding fall back
// to the in-memory buckets of each isolate (weak: isolates come and go).
//
// Cloudflare's own caveats: counts are per location and eventually
// consistent, so a burst can pass a little over the limit and a client
// spread over many locations gets the limit in each. That is fine for abuse
// limits; nothing here needs an exact count.

import { ruleFor } from "@nebulamail/net-guard";
import type { RateLimiter, RateLimitRules } from "@nebulamail/net-guard";

import { createRateLimiter, DEFAULT_RATE_LIMITS } from "../rate-limit";

/** `env.<NAME>` of a `[[ratelimits]]` binding: `limit({ key })` → `{ success }`. */
export interface RateLimitBinding {
  readonly limit: (options: {
    readonly key: string;
  }) => Promise<{ readonly success: boolean }>;
}

/** Binding name per rule (wrangler.example.toml uses these). */
export const RATE_LIMIT_BINDINGS: Readonly<Record<string, string>> =
  Object.freeze({
    "batch:account": "RATE_LIMIT_BATCH_ACCOUNT",
    "batch:global": "RATE_LIMIT_BATCH_GLOBAL",
    "batch:ip": "RATE_LIMIT_BATCH_IP",
    "image:global": "RATE_LIMIT_IMAGE_GLOBAL",
    "image:ip": "RATE_LIMIT_IMAGE_IP",
  });

export const isRateLimitBinding = (value: unknown): value is RateLimitBinding =>
  typeof value === "object" &&
  value !== null &&
  typeof Reflect.get(value, "limit") === "function";

/**
 * Bindings found in `env`, by rule. A value under one of our names that is
 * not a binding is a configuration error (`EnvError` from the caller).
 */
export const rateLimitBindings = (
  env: Readonly<Record<string, unknown>>
): {
  readonly bindings: ReadonlyMap<string, RateLimitBinding>;
  readonly invalid: readonly string[];
} => {
  const bindings = new Map<string, RateLimitBinding>();
  const invalid: string[] = [];
  for (const [rule, name] of Object.entries(RATE_LIMIT_BINDINGS)) {
    const value = env[name];
    if (value === undefined) {
      continue;
    }
    if (isRateLimitBinding(value)) {
      bindings.set(rule, value);
    } else {
      invalid.push(name);
    }
  }
  return { bindings, invalid };
};

/**
 * Each key goes to its rule's binding when there is one, else to the
 * in-memory buckets. A binding that throws falls back to memory for that
 * call (never blocks the proxy, never lets everything through either).
 */
export const createWorkersRateLimiter = (
  bindings: ReadonlyMap<string, RateLimitBinding>,
  rules: RateLimitRules = DEFAULT_RATE_LIMITS
): RateLimiter => {
  const memory = createRateLimiter({ rules });
  // Which binding a key goes to depends only on the binding names, not on
  // the in-memory rules (those may be customised or leave a prefix out).
  const bound: RateLimitRules = Object.fromEntries(
    [...bindings.keys()].map((prefix) => [
      prefix,
      { capacity: 1, refillPerSecond: 0 },
    ])
  );
  return async (key) => {
    const match = ruleFor(bound, key);
    const binding =
      match === undefined ? undefined : bindings.get(match.prefix);
    if (binding === undefined) {
      return memory(key);
    }
    try {
      const { success } = await binding.limit({ key });
      return success;
    } catch {
      return memory(key);
    }
  };
};
