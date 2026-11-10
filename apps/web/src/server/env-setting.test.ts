// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { envSetting } from "./env-setting";

describe(envSetting, () => {
  it("treats unset, empty and blank values as unset", () => {
    expect(envSetting(undefined)).toBeUndefined();
    expect(envSetting("")).toBeUndefined();
    expect(envSetting("  ")).toBeUndefined();
  });

  it("keeps a real value as it is", () => {
    expect(envSetting("1:abc")).toBe("1:abc");
    expect(envSetting(" 1:abc ")).toBe(" 1:abc ");
  });
});
