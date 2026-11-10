// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { ORDER_STEP, dropIndex, reorder, stepTarget } from "./ordering";

const task = (id: string, sortOrder: number | null) => ({
  baseId: CalendarEventId.parse(id),
  sortOrder,
});
const ids = (map: Map<string, number>) => Object.fromEntries(map);

describe(reorder, () => {
  it("writes only the moved task when there is room", () => {
    const siblings = [task("a", 100), task("b", 200), task("c", 300)];
    expect(ids(reorder(siblings, 2, 1))).toStrictEqual({ c: 150 });
    expect(ids(reorder(siblings, 0, 2))).toStrictEqual({ a: 300 + ORDER_STEP });
    expect(ids(reorder(siblings, 2, 0))).toStrictEqual({ c: 100 - ORDER_STEP });
  });

  it("goes after the last numbered task when the rest have no order", () => {
    const siblings = [task("a", 100), task("b", null), task("c", null)];
    expect(ids(reorder(siblings, 2, 1))).toStrictEqual({ c: 100 + ORDER_STEP });
  });

  it("numbers the level when there is no room or no order yet", () => {
    expect(
      ids(reorder([task("a", null), task("b", null), task("c", null)], 2, 0))
    ).toStrictEqual({ a: 2048, b: 3072, c: 1024 });
    expect(
      ids(reorder([task("a", 1), task("b", 2), task("c", 3)], 2, 1))
    ).toStrictEqual({ a: 1024, b: 3072, c: 2048 });
  });

  it("does nothing when it stays", () => {
    expect(reorder([task("a", 1), task("b", 2)], 1, 1).size).toBe(0);
    expect(reorder([task("a", 1)], 3, 0).size).toBe(0);
  });

  it("clamps the target", () => {
    expect(ids(reorder([task("a", 1), task("b", 2)], 0, 9))).toStrictEqual({
      a: 2 + ORDER_STEP,
    });
  });
});

describe(stepTarget, () => {
  it("moves one place inside the level", () => {
    expect(stepTarget(1, -1, 3)).toBe(0);
    expect(stepTarget(1, 1, 3)).toBe(2);
    expect(stepTarget(0, -1, 3)).toBeNull();
    expect(stepTarget(2, 1, 3)).toBeNull();
  });
});

describe(dropIndex, () => {
  it("counts the other siblings above the pointer", () => {
    const middles = [10, 30, 50, 70];
    expect(dropIndex(middles, 0, 5)).toBe(0);
    expect(dropIndex(middles, 0, 60)).toBe(2);
    expect(dropIndex(middles, 3, 40)).toBe(2);
    expect(dropIndex(middles, 3, 100)).toBe(3);
  });
});
