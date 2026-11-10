// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Abuse limits around guarded fetches,
// runtime-agnostic:
//
// - `createRateLimiter`: token buckets by key prefix (per user, per IP,
//   global). In memory, so one process / isolate / instance only: with
//   several replicas each one counts on its own and the real limit is
//   approximate (N replicas ≈ N × the limit). Workers can plug in
//   Cloudflare's Rate Limiting binding instead (`@nebulamail/image-proxy`).
// - `createFetchBudget`: requests and bytes for one unit of work (one email).
//   Every hop of a guarded fetch takes a request, so redirects count too.
// - `createConcurrencyLimiter`: at most N tasks at once with a bounded queue,
//   so a flood of work waits (or is refused) instead of opening sockets.

/** `true` = allowed. May be async (e.g. a remote limiter). */
export type RateLimiter = (key: string) => boolean | Promise<boolean>;

export interface BucketRule {
  /** Burst size. */
  readonly capacity: number;
  readonly refillPerSecond: number;
}

/** Rules by key prefix: a key `batch:ip:1.2.3.4` matches the rule `batch:ip`. */
export type RateLimitRules = Readonly<Record<string, BucketRule>>;

export interface RateLimiterOptions {
  readonly rules: RateLimitRules;
  /** Tracked keys; the least recently used are forgotten beyond this. */
  readonly maxKeys?: number;
  readonly now?: () => number;
}

/** The rule (and its prefix) for `key`: the longest matching prefix wins. */
export const ruleFor = (
  rules: RateLimitRules,
  key: string
): { readonly prefix: string; readonly rule: BucketRule } | undefined => {
  let best: { prefix: string; rule: BucketRule } | undefined;
  for (const [prefix, rule] of Object.entries(rules)) {
    if (
      (key === prefix || key.startsWith(`${prefix}:`)) &&
      (best === undefined || prefix.length > best.prefix.length)
    ) {
      best = { prefix, rule };
    }
  }
  return best;
};

const checkRule = (prefix: string, rule: BucketRule): void => {
  if (
    !Number.isFinite(rule.capacity) ||
    rule.capacity < 1 ||
    !Number.isFinite(rule.refillPerSecond) ||
    rule.refillPerSecond < 0
  ) {
    throw new TypeError(`net-guard: invalid rate limit rule ${prefix}`);
  }
};

/**
 * In-memory token buckets. Keys without a matching rule are always allowed.
 * Synchronous (returns a boolean), so it also fits a `RateLimiter`.
 */
export const createRateLimiter = (
  options: RateLimiterOptions
): ((key: string) => boolean) => {
  const { rules } = options;
  for (const [prefix, rule] of Object.entries(rules)) {
    checkRule(prefix, rule);
  }
  const maxKeys = options.maxKeys ?? 50_000;
  const now = options.now ?? Date.now;
  const buckets = new Map<string, { tokens: number; at: number }>();
  return (key) => {
    const match = ruleFor(rules, key);
    if (match === undefined) {
      return true;
    }
    const { rule } = match;
    const t = now();
    const bucket = buckets.get(key) ?? { at: t, tokens: rule.capacity };
    buckets.delete(key);
    bucket.tokens = Math.min(
      rule.capacity,
      bucket.tokens + (Math.max(0, t - bucket.at) / 1000) * rule.refillPerSecond
    );
    bucket.at = t;
    const allowed = bucket.tokens >= 1;
    if (allowed) {
      bucket.tokens -= 1;
    }
    buckets.set(key, bucket);
    for (const oldest of buckets.keys()) {
      if (buckets.size <= maxKeys) {
        break;
      }
      buckets.delete(oldest);
    }
    return allowed;
  };
};

/**
 * Requests and bytes for one unit of work (e.g. all images of one email).
 * Shared by concurrent fetches; JavaScript runs one callback at a time, so
 * the counters need no locking.
 */
export interface FetchBudget {
  /** Take one request (one hop). `false` = over budget, do not send it. */
  readonly takeRequest: () => boolean;
  /**
   * Take `bytes`. `false` = they do not fit; the budget is then exhausted
   * and every later take fails too.
   */
  readonly takeBytes: (bytes: number) => boolean;
  readonly remainingBytes: () => number;
  readonly remainingRequests: () => number;
  /** A `takeBytes` failed: callers stop the remaining work. */
  readonly exhausted: () => boolean;
}

export interface FetchBudgetOptions {
  readonly maxRequests: number;
  readonly maxBytes: number;
}

const checkPositive = (name: string, value: number): void => {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`net-guard: ${name} must be a positive integer`);
  }
};

export const createFetchBudget = (options: FetchBudgetOptions): FetchBudget => {
  checkPositive("maxRequests", options.maxRequests);
  checkPositive("maxBytes", options.maxBytes);
  let requests = options.maxRequests;
  let bytes = options.maxBytes;
  let exhausted = false;
  return {
    exhausted: () => exhausted,
    remainingBytes: () => bytes,
    remainingRequests: () => requests,
    takeBytes: (n) => {
      if (exhausted || !Number.isSafeInteger(n) || n < 0 || n > bytes) {
        exhausted = true;
        return false;
      }
      bytes -= n;
      return true;
    },
    takeRequest: () => {
      if (requests <= 0) {
        return false;
      }
      requests -= 1;
      return true;
    },
  };
};

export class QueueFullError extends Error {
  override readonly name = "QueueFullError";
}

export interface ConcurrencyLimiter {
  /**
   * Run `task` when a slot is free. Rejects with `QueueFullError` when the
   * queue is full, and with the signal's reason when `signal` aborts while
   * waiting (the task then never runs).
   */
  readonly run: <T>(task: () => Promise<T>, signal?: AbortSignal) => Promise<T>;
  readonly active: () => number;
  readonly queued: () => number;
}

export interface ConcurrencyLimiterOptions {
  readonly max: number;
  /** Waiting tasks beyond this are refused. Default `max * 16`. */
  readonly maxQueue?: number;
}

export const createConcurrencyLimiter = (
  options: ConcurrencyLimiterOptions
): ConcurrencyLimiter => {
  checkPositive("max", options.max);
  const maxQueue = options.maxQueue ?? options.max * 16;
  if (!Number.isSafeInteger(maxQueue) || maxQueue < 0) {
    throw new TypeError("net-guard: maxQueue must be a non-negative integer");
  }
  let active = 0;
  const waiting: PromiseWithResolvers<undefined>[] = [];

  const release = (): void => {
    active -= 1;
    const turn = waiting.shift();
    if (turn !== undefined) {
      // The slot passes straight to the next waiter.
      active += 1;
      turn.resolve(undefined);
    }
  };

  const acquire = async (signal?: AbortSignal): Promise<void> => {
    signal?.throwIfAborted();
    if (active < options.max) {
      active += 1;
      return;
    }
    if (waiting.length >= maxQueue) {
      throw new QueueFullError("net-guard: too many queued tasks");
    }
    const turn = Promise.withResolvers<undefined>();
    const onAbort = (): void => {
      const index = waiting.indexOf(turn);
      if (index !== -1) {
        waiting.splice(index, 1);
        turn.reject(signal?.reason);
      }
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    waiting.push(turn);
    try {
      await turn.promise;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }
  };

  return {
    active: () => active,
    queued: () => waiting.length,
    run: async (task, signal) => {
      await acquire(signal);
      try {
        return await task();
      } finally {
        release();
      }
    },
  };
};
