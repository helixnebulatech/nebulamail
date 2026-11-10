// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { JmapError, isMethodError } from "./errors";

const methodError = (type: string): JmapError =>
  new JmapError({
    arguments: {},
    callId: "c0",
    kind: "method",
    method: "Email/query",
    type,
  });

describe(isMethodError, () => {
  it("is true only for a method error of that type", () => {
    expect([
      isMethodError(methodError("unsupportedFilter"), "unsupportedFilter"),
      isMethodError(methodError("serverFail"), "unsupportedFilter"),
      isMethodError(new JmapError({ kind: "network" }), "unsupportedFilter"),
      isMethodError(new Error("unsupportedFilter"), "unsupportedFilter"),
      isMethodError(undefined, "unsupportedFilter"),
    ]).toStrictEqual([true, false, false, false, false]);
  });
});
