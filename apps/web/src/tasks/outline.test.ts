// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  indentTarget,
  isWithin,
  outdentTarget,
  outlineRows,
  withSubtasks,
} from "./outline";
import { makeTask as task } from "./test-support";
import { nestTasks } from "./views";

// a
//   a1
//     a1x
//   a2
// b
// c (another list)
const a = task("a", { sortOrder: 1 });
const a1 = task("a1", { parentUid: "a@uid", sortOrder: 1 });
const a1x = task("a1x", { parentUid: "a1@uid" });
const a2 = task("a2", { parentUid: "a@uid", sortOrder: 2 });
const b = task("b", { sortOrder: 2 });
const c = task("c", { listKey: "c/other", sortOrder: 3 });
const all = [a, a1, a1x, a2, b, c];
const rows = outlineRows(nestTasks(all, "manual"));
const at = (id: string) => rows.findIndex((row) => row.task.id === id);

describe(outlineRows, () => {
  it("lists the tree depth first, with depth and parent", () => {
    expect(
      rows.map((row) => [row.task.id, row.depth, row.parent?.id ?? null])
    ).toStrictEqual([
      ["a", 0, null],
      ["a1", 1, "a"],
      ["a1x", 2, "a1"],
      ["a2", 1, "a"],
      ["b", 0, null],
      ["c", 0, null],
    ]);
    expect(rows[0]?.children).toBe(2);
  });
});

describe(indentTarget, () => {
  it("is the previous sibling, skipping its subtasks", () => {
    expect(indentTarget(rows, at("b"))?.id).toBe("a");
    expect(indentTarget(rows, at("a2"))?.id).toBe("a1");
  });

  it("is nothing for the first at its level or across lists", () => {
    expect(indentTarget(rows, at("a"))).toBeNull();
    expect(indentTarget(rows, at("a1"))).toBeNull();
    expect(indentTarget(rows, at("a1x"))).toBeNull();
    expect(indentTarget(rows, at("c"))).toBeNull();
    expect(indentTarget(rows, 99)).toBeNull();
  });

  it("is nothing across accounts or without a UID", () => {
    const other = task("o", { accountId: AccountId.parse("b") });
    const noUid = task("n", { sortOrder: 1, uid: null });
    const flat = outlineRows(
      nestTasks([noUid, task("p", { sortOrder: 2 }), other], "manual")
    );
    const index = (id: string) => flat.findIndex((row) => row.task.id === id);
    expect(indentTarget(flat, index("o"))).toBeNull();
    expect(indentTarget(flat, index("p"))).toBeNull();
  });
});

describe(outdentTarget, () => {
  it("is the grandparent, the top, or nothing at the top", () => {
    expect(outdentTarget(rows, at("a1x"))).toBe("a@uid");
    expect(outdentTarget(rows, at("a1"))).toBeNull();
    expect(outdentTarget(rows, at("b"))).toBeUndefined();
  });
});

describe(isWithin, () => {
  it("finds a task's subtasks at any depth", () => {
    expect(isWithin(all, a, "a1x@uid")).toBeTruthy();
    expect(isWithin(all, a, "a@uid")).toBeTruthy();
    expect(isWithin(all, a1, "a2@uid")).toBeFalsy();
    expect(isWithin(all, b, "a@uid")).toBeFalsy();
  });

  it("stops in a loop", () => {
    const x = task("x", { parentUid: "y@uid" });
    const y = task("y", { parentUid: "x@uid" });
    expect(isWithin([x, y], task("z"), "x@uid")).toBeFalsy();
  });

  it("answers for each tasks array, not the first one asked", () => {
    expect(isWithin(all, a, "a1x@uid")).toBeTruthy();
    // The same task under another parent, in a new array.
    const moved = all.map((item) =>
      item.id === "a1x" ? { ...item, parentUid: "b@uid" } : item
    );
    expect(isWithin(moved, a, "a1x@uid")).toBeFalsy();
    expect(isWithin(moved, b, "a1x@uid")).toBeTruthy();
  });
});

describe(withSubtasks, () => {
  it("gathers a task and everything under it", () => {
    expect(withSubtasks(all, a).map((item) => item.id)).toStrictEqual([
      "a",
      "a1",
      "a2",
      "a1x",
    ]);
    expect(withSubtasks(all, b).map((item) => item.id)).toStrictEqual(["b"]);
  });
});
