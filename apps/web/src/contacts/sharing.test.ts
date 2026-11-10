// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { bookLevelOf, bookRightsFor } from "./sharing";

describe(bookLevelOf, () => {
  it("reads the levels with Stalwart's maySetMetadata", () => {
    for (const level of ["read", "write", "manage"] as const) {
      const rights = { ...bookRightsFor(level), maySetMetadata: false };
      expect(bookLevelOf(rights)).toBe(level);
    }
  });

  it("never asks for more than the level by itself", () => {
    expect(bookRightsFor("read")).toStrictEqual({
      mayDelete: false,
      mayRead: true,
      mayShare: false,
      mayWrite: false,
    });
  });
});
