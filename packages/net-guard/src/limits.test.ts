// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { setTimeout as sleep } from "node:timers/promises";

import { describe, expect, it } from "vitest";

import {
  createConcurrencyLimiter,
  createFetchBudget,
  createRateLimiter,
  QueueFullError,
  ruleFor,
} from "./limits";

describe(createRateLimiter, () => {
  it("allows a burst, then refills over time", () => {
    let t = 0;
    const allow = createRateLimiter({
      now: () => t,
      rules: { "batch:ip": { capacity: 2, refillPerSecond: 1 } },
    });
    const burst = [
      allow("batch:ip:1.2.3.4"),
      allow("batch:ip:1.2.3.4"),
      allow("batch:ip:1.2.3.4"),
      // Another key has its own bucket.
      allow("batch:ip:5.6.7.8"),
    ];
    t = 1000;
    const refilled = [allow("batch:ip:1.2.3.4"), allow("batch:ip:1.2.3.4")];
    expect([burst, refilled]).toStrictEqual([
      [true, true, false, true],
      [true, false],
    ]);
  });

  it("a global key is the prefix itself; keys without a rule pass", () => {
    const allow = createRateLimiter({
      rules: { "batch:global": { capacity: 1, refillPerSecond: 0 } },
    });
    expect([
      allow("batch:global"),
      allow("batch:global"),
      allow("other"),
    ]).toStrictEqual([true, false, true]);
  });

  it("forgets the least recently used keys beyond maxKeys", () => {
    const allow = createRateLimiter({
      maxKeys: 1,
      rules: { k: { capacity: 1, refillPerSecond: 0 } },
    });
    // `k:a` is forgotten when `k:b` arrives: a fresh bucket.
    expect([allow("k:a"), allow("k:b"), allow("k:a")]).toStrictEqual([
      true,
      true,
      true,
    ]);
  });

  it("refuses invalid rules and picks the longest prefix", () => {
    expect(() =>
      createRateLimiter({ rules: { x: { capacity: 0, refillPerSecond: 1 } } })
    ).toThrow(TypeError);
    const rules = {
      a: { capacity: 1, refillPerSecond: 1 },
      "a:b": { capacity: 2, refillPerSecond: 1 },
    };
    expect([
      ruleFor(rules, "a:b:c")?.prefix,
      ruleFor(rules, "a:c")?.prefix,
      ruleFor(rules, "ab")?.prefix,
    ]).toStrictEqual(["a:b", "a", undefined]);
  });
});

describe(createFetchBudget, () => {
  it("counts requests and bytes; a failed byte take exhausts it", () => {
    const budget = createFetchBudget({ maxBytes: 100, maxRequests: 2 });
    const requests = [
      budget.takeRequest(),
      budget.takeRequest(),
      budget.takeRequest(),
    ];
    const first = [
      budget.takeBytes(60),
      budget.remainingBytes(),
      budget.exhausted(),
    ];
    const second = [
      budget.takeBytes(41),
      budget.remainingBytes(),
      budget.exhausted(),
      budget.takeBytes(-1),
      // Exhausted for good: even what would fit is refused.
      budget.takeBytes(1),
    ];
    expect([requests, first, second]).toStrictEqual([
      [true, true, false],
      [true, 40, false],
      [false, 40, true, false, false],
    ]);
  });

  it("refuses invalid sizes", () => {
    expect(() => createFetchBudget({ maxBytes: 0, maxRequests: 1 })).toThrow(
      TypeError
    );
    expect(() =>
      createFetchBudget({ maxBytes: 1, maxRequests: Number.NaN })
    ).toThrow(TypeError);
  });
});

describe(createConcurrencyLimiter, () => {
  it("runs at most `max` tasks at once, in order", async () => {
    const limiter = createConcurrencyLimiter({ max: 2 });
    let running = 0;
    let peak = 0;
    const task = (id: number) => async () => {
      running += 1;
      peak = Math.max(peak, running);
      await sleep(5);
      running -= 1;
      return id;
    };
    const results = await Promise.all(
      [1, 2, 3, 4, 5].map(async (id) => await limiter.run(task(id)))
    );
    expect([results, peak, limiter.active()]).toStrictEqual([
      [1, 2, 3, 4, 5],
      2,
      0,
    ]);
  });

  it("refuses beyond the queue and releases a slot after a failure", async () => {
    const limiter = createConcurrencyLimiter({ max: 1, maxQueue: 1 });
    const gate = Promise.withResolvers<undefined>();
    const first = limiter.run(async () => {
      await gate.promise;
    });
    const second = limiter.run(async () => await Promise.resolve("second"));
    await expect(
      limiter.run(async () => await Promise.resolve("third"))
    ).rejects.toBeInstanceOf(QueueFullError);
    expect(limiter.queued()).toBe(1);
    gate.resolve(undefined);
    await first;
    await expect(second).resolves.toBe("second");
    await expect(
      limiter.run(async () => await Promise.reject(new Error("x")))
    ).rejects.toThrow("x");
    expect(limiter.active()).toBe(0);
  });

  it("a waiting task whose signal aborts never runs", async () => {
    const limiter = createConcurrencyLimiter({ max: 1 });
    const gate = Promise.withResolvers<undefined>();
    const first = limiter.run(async () => {
      await gate.promise;
    });
    const abort = new AbortController();
    let ran = false;
    const waiting = limiter.run(async () => {
      ran = true;
      await Promise.resolve();
    }, abort.signal);
    abort.abort(new Error("gone"));
    await expect(waiting).rejects.toThrow("gone");
    const queued = limiter.queued();
    gate.resolve(undefined);
    await first;
    expect([queued, ran, limiter.active()]).toStrictEqual([0, false, 0]);
  });
});
