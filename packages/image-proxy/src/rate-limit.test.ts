// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { clientKey, createRateLimiter } from "./rate-limit";

describe(createRateLimiter, () => {
  it("allows a burst, then refills over time", () => {
    let t = 0;
    const allow = createRateLimiter({
      now: () => t,
      rules: { "batch:ip": { capacity: 2, refillPerSecond: 1 } },
    });
    expect([
      allow("batch:ip:a"),
      allow("batch:ip:a"),
      allow("batch:ip:a"),
    ]).toStrictEqual([true, true, false]);
    expect([allow("batch:ip:b")]).toStrictEqual([true]);
    t = 1000;
    expect([allow("batch:ip:a"), allow("batch:ip:a")]).toStrictEqual([
      true,
      false,
    ]);
  });

  it("keys without a rule are allowed", () => {
    const allow = createRateLimiter({ rules: {} });
    expect([allow("anything"), allow("anything")]).toStrictEqual([true, true]);
  });

  it("forgets the least recently used keys beyond maxKeys", () => {
    const allow = createRateLimiter({
      maxKeys: 2,
      now: () => 0,
      rules: { k: { capacity: 1, refillPerSecond: 0 } },
    });
    // After k:3, k:1 is forgotten (fresh bucket) but k:3 is still tracked.
    expect([
      allow("k:1"),
      allow("k:2"),
      allow("k:3"),
      allow("k:3"),
      allow("k:1"),
    ]).toStrictEqual([true, true, true, false, true]);
  });
});

describe(clientKey, () => {
  it.each([
    ["203.0.113.9", "203.0.113.9"],
    ["::ffff:203.0.113.9", "203.0.113.9"],
    ["2001:db8:1:2:3:4:5:6", "2001:db8:1:2::/64"],
    ["2001:db8:1:2::9", "2001:db8:1:2::/64"],
    ["2001:db8::1", "2001:db8:0:0::/64"],
    ["fe80::1%eth0", "fe80:0:0:0::/64"],
    ["not an ip", undefined],
    [undefined, undefined],
  ])("%s → %s", (input, expected) => {
    expect(clientKey(input)).toBe(expected);
  });
});
