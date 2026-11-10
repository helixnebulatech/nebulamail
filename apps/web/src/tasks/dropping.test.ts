// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { canNest, dropOf, zoneAt } from "./dropping";
import type { PlacedRow } from "./list-model";
import { listModel } from "./list-model";
import type { Task } from "./model";
import { makeTask as task } from "./test-support";

// A list: a, b (with b1, b2 under it), c; d in another list.
const tasks: Task[] = [
  task("a", { sortOrder: 1 }),
  task("b", { sortOrder: 2 }),
  task("b1", { parentUid: "b@uid", sortOrder: 1 }),
  task("b2", { parentUid: "b@uid", sortOrder: 2 }),
  task("c", { sortOrder: 3 }),
  task("done", { completed: true, sortOrder: 4 }),
];

const { rows } = listModel({
  kept: new Set(),
  now: 0,
  scope: { key: "c/l1", kind: "list" },
  search: "",
  showDone: true,
  tasks,
  zone: "UTC",
});

const row = (id: string): PlacedRow => {
  const found = rows.find((item) => item.task.id === id);
  if (found === undefined) {
    throw new Error(`no row ${id}`);
  }
  return found;
};

const drop = (
  from: string,
  over: string,
  zone: "before" | "inside" | "after"
) => dropOf({ dragged: row(from), over: row(over), rows, tasks, zone });

describe(zoneAt, () => {
  it("splits a row into its edges and its middle", () => {
    expect(zoneAt(100, 40, 105)).toBe("before");
    expect(zoneAt(100, 40, 120)).toBe("inside");
    expect(zoneAt(100, 40, 135)).toBe("after");
  });
});

describe(dropOf, () => {
  it("makes a task a subtask when dropped on the middle of another", () => {
    const result = drop("a", "c", "inside");
    expect(result?.kind).toBe("nest");
    expect(result?.kind === "nest" ? result.parent.task.id : null).toBe("c");
  });

  it("reorders among siblings on the edges", () => {
    const result = drop("a", "c", "after");
    expect(result).toMatchObject({ kind: "place", moves: false, to: 2 });
    expect(result?.kind === "place" ? result.parentUid : "x").toBeNull();
    expect(drop("a", "b", "before")).toBeNull();
  });

  it("takes a subtask out next to a top task (outdent)", () => {
    const result = drop("b1", "c", "before");
    expect(result).toMatchObject({ kind: "place", moves: true, to: 2 });
    expect(result?.kind === "place" ? result.parentUid : "x").toBeNull();
  });

  it("moves a task in among the subtasks of another", () => {
    const result = drop("a", "b2", "after");
    expect(result).toMatchObject({
      kind: "place",
      moves: true,
      parentUid: "b@uid",
      to: 2,
    });
  });

  it("nests on the bottom edge of a task with subtasks shown", () => {
    expect(drop("c", "b", "after")?.kind).toBe("nest");
  });

  it("never makes a loop or nests under its own parent again", () => {
    expect(drop("b", "b1", "inside")).toBeNull();
    expect(drop("b", "b1", "before")).toBeNull();
    expect(drop("b1", "b", "inside")).toBeNull();
    expect(drop("a", "a", "inside")).toBeNull();
  });

  it("never nests under a done task", () => {
    expect(drop("a", "done", "inside")).toBeNull();
  });

  it("stays in its list and account", () => {
    const other = task("d", { listKey: "c/l2" });
    const all = [...tasks, other];
    const over: PlacedRow = {
      children: 0,
      depth: 0,
      done: false,
      index: 0,
      parent: null,
      section: "open",
      task: other,
    };
    expect(
      dropOf({ dragged: row("a"), over, rows, tasks: all, zone: "inside" })
    ).toBeNull();
    expect(canNest(all, row("a").task, over)).toBeFalsy();
  });
});
