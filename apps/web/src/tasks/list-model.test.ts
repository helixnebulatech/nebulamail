// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  fallbackList,
  listModel,
  stepIndex,
  subtaskCounts,
} from "./list-model";
import { searchTasks } from "./nav";
import { makeTask as task } from "./test-support";

const zone = "UTC";
const now = Date.UTC(2026, 9, 29, 12);
const today = { date: "2026-10-29", kind: "date" as const };
const dueToday = { due: today, dueAt: Date.UTC(2026, 9, 29) };

describe(listModel, () => {
  const tasks = [
    task("a", dueToday),
    task("b", { completed: true, ...dueToday }),
    task("c", { parentUid: "a@uid", ...dueToday }),
  ];

  it("lays out a view's rows with their sections", () => {
    const model = listModel({
      kept: new Set(),
      now,
      scope: { kind: "view", view: "today" },
      search: "",
      showDone: false,
      tasks,
      zone,
    });
    expect(
      model.rows.map((row) => [row.task.id, row.depth, row.section, row.index])
    ).toStrictEqual([
      ["a", 0, "today", 0],
      ["c", 1, "today", 1],
    ]);
    expect(model.placedBySection.get("today")).toStrictEqual(model.rows);
  });

  it("keeps a task ticked here where it was, shown as done", () => {
    const model = listModel({
      kept: new Set(["b"]),
      now,
      scope: { kind: "view", view: "today" },
      search: "",
      showDone: false,
      tasks,
      zone,
    });
    expect(model.rows.find((row) => row.task.id === "b")?.done).toBeTruthy();
  });

  it("searches titles and notes, accents aside", () => {
    const model = listModel({
      kept: new Set(),
      now,
      scope: { kind: "view", view: "all" },
      search: "cafe",
      showDone: false,
      tasks: [task("x", { title: "Café" }), task("y", { notes: "no" })],
      zone,
    });
    expect(model.rows.map((row) => row.task.id)).toStrictEqual(["x"]);
  });
});

describe(subtaskCounts, () => {
  it("counts subtasks done per parent, a series once", () => {
    const counts = subtaskCounts([
      task("c1", { parentUid: "p@uid" }),
      task("c2", { completed: true, parentUid: "p@uid" }),
      task("s1", { baseId: task("s").baseId, parentUid: "p@uid" }),
      task("s2", { baseId: task("s").baseId, parentUid: "p@uid" }),
    ]);
    expect(counts.get("c/p@uid")).toStrictEqual({ all: 3, done: 1 });
  });
});

describe(stepIndex, () => {
  it("starts at an end and stays inside", () => {
    expect(stepIndex(-1, 1, 3)).toBe(0);
    expect(stepIndex(-1, -1, 3)).toBe(2);
    expect(stepIndex(2, 1, 3)).toBe(2);
    expect(stepIndex(0, -1, 3)).toBe(0);
    expect(stepIndex(-1, 1, 0)).toBe(-1);
  });
});

describe(fallbackList, () => {
  it("takes the default list, else the user's default calendar", () => {
    const calendars = [
      { isDefault: false, key: "a/x", mayWrite: true, owner: null },
      { isDefault: true, key: "a/y", mayWrite: true, owner: null },
      { isDefault: false, key: "b/z", mayWrite: true, owner: "Bob" },
    ];
    expect(fallbackList("a/l", calendars)).toBe("a/l");
    expect(fallbackList(null, calendars)).toBe("a/y");
    expect(fallbackList(null, calendars.slice(2))).toBeNull();
  });
});

describe(searchTasks, () => {
  it("matches every word, accents and case aside", () => {
    const tasks = [
      task("a", { notes: "Comprar PAN", title: "Árbol" }),
      task("b", { title: "Otra" }),
    ];
    expect(
      searchTasks(tasks, "arbol pan").map((item) => item.id)
    ).toStrictEqual(["a"]);
    expect(searchTasks(tasks, "  ")).toBe(tasks);
  });

  it("sees a changed task (a new object) by its new words", () => {
    const before = task("a", { title: "Antes" });
    expect(searchTasks([before], "antes")).toHaveLength(1);
    const after = { ...before, title: "Después" };
    expect(searchTasks([after], "antes")).toHaveLength(0);
    expect(searchTasks([after], "despues")).toHaveLength(1);
  });
});
