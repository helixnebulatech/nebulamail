// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { cacheHeadersOf, keepForMs, storableTtlMs } from "./cache-policy";

const MAX = 30 * 24 * 3_600_000;

describe(storableTtlMs, () => {
  it.each([
    "no-store",
    "no-cache",
    "max-age=0",
    "max-age=60",
    "private, max-age=10",
    'max-age="10',
    "",
  ])(
    "in one account's cache, %j still gets our maximum (no open counting)",
    (value) => {
      expect(storableTtlMs({ "cache-control": value }, "private", MAX)).toBe(
        MAX
      );
    }
  );

  it.each([
    ["private", 0],
    ["PRIVATE, max-age=60", 0],
    ["no-store", 0],
    ['private="set-cookie"', 0],
    // Unreadable: kept out too.
    ['max-age="10', 0],
    // Anything else: our maximum, whatever its freshness.
    ["no-cache", MAX],
    ["max-age=60", MAX],
    ["public, s-maxage=1", MAX],
    ["", MAX],
  ])(
    "in the shared cache, %j → %i ms (other users' privacy)",
    (value, expected) => {
      expect(storableTtlMs({ "cache-control": value }, "shared", MAX)).toBe(
        expected
      );
    }
  );

  it("without Cache-Control: our maximum", () => {
    expect(storableTtlMs({}, "shared", MAX)).toBe(MAX);
  });
});

describe(keepForMs, () => {
  it("a fixed-lifetime storage gets our maximum only if it fits", () => {
    expect(keepForMs({}, "shared", { fixedTtlMs: MAX, maxMs: MAX })).toBe(MAX);
    expect(keepForMs({}, "shared", { fixedTtlMs: MAX + 1, maxMs: MAX })).toBe(
      0
    );
    expect(
      keepForMs({ cacheHeaders: { "cache-control": "private" } }, "shared", {
        fixedTtlMs: MAX,
        maxMs: MAX,
      })
    ).toBe(0);
  });
});

describe(cacheHeadersOf, () => {
  it("keeps only Cache-Control", () => {
    expect(
      cacheHeadersOf({
        age: "1",
        "cache-control": "no-store",
        "content-type": "image/png",
        expires: "0",
        "set-cookie": "x=1",
      })
    ).toStrictEqual({ "cache-control": "no-store" });
  });
});
