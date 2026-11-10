// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { PRELOAD_TOKEN_PROPERTY, preloadTokenOf } from "./preload-token";

const RECEIVED = "2026-10-30T10:00:00Z";
const AT = Date.parse(RECEIVED) / 1000;
const token = (issuedAt: number): string =>
  `2.1.${String(issuedAt)}.${"a".repeat(64)}.${"B".repeat(43)}`;
const email = (values: readonly string[] | null, receivedAt = RECEIVED) => ({
  [PRELOAD_TOKEN_PROPERTY]: values,
  receivedAt,
});

describe(preloadTokenOf, () => {
  it("takes the first token made for this delivery", () => {
    expect(preloadTokenOf(email([token(AT - 3), token(AT)]))).toBe(
      token(AT - 3)
    );
    // Delivery that waited in the queue, or a proxy clock a bit ahead.
    expect(preloadTokenOf(email([token(AT - 50 * 60)]))).not.toBeNull();
    expect(preloadTokenOf(email([token(AT + 4 * 60)]))).not.toBeNull();
  });

  it("ignores a token copied from an older email", () => {
    expect(preloadTokenOf(email([token(AT - 2 * 60 * 60)]))).toBeNull();
    expect(preloadTokenOf(email([token(AT + 10 * 60)]))).toBeNull();
  });

  it("ignores what isn't a token", () => {
    expect(preloadTokenOf(email(null))).toBeNull();
    expect(
      preloadTokenOf(email([`1.1.${"a".repeat(64)}.${"B".repeat(43)}`]))
    ).toBeNull();
    expect(preloadTokenOf(email([token(AT)], "not a date"))).toBeNull();
  });
});
