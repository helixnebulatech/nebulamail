// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `test:bun` and `test:deno` set EXPECT_RUNTIME, so CI proves the suite
// really ran inside that runtime (and not, say, in Node workers).

import { describe, expect, it } from "vitest";

const runtime = (): string => {
  if (process.versions.bun !== undefined) {
    return "bun";
  }
  return Reflect.get(globalThis, "Deno") === undefined ? "node" : "deno";
};

describe("runtime", () => {
  it.runIf(process.env.EXPECT_RUNTIME !== undefined)(
    "is the expected one",
    () => {
      expect(runtime()).toBe(process.env.EXPECT_RUNTIME);
    }
  );
});
