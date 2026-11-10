// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { Task } from "./model";
import {
  allView,
  completedView,
  listView,
  nestTasks,
  openCounts,
  todayCount,
  todayView,
  upcomingView,
} from "./views";

const zone = "Europe/Madrid";
// Thursday 29 October 2026, 12:00 in Madrid.
const now = Date.UTC(2026, 9, 29, 11);
const at = (day: string, time = "00:00") =>
  Date.parse(`${day}T${time}:00+01:00`);

const task = (id: string, extra: Partial<Task> = {}): Task => ({
  accountId: AccountId.parse("c"),
  alerts: [],
  baseId: CalendarEventId.parse(id),
  calendarId: null,
  completed: false,
  completedAt: null,
  created: null,
  due: null,
  dueAt: null,
  estimatedMinutes: null,
  id: CalendarEventId.parse(id),
  keywords: [],
  links: [],
  listKey: "c/l1",
  notes: null,
  parentUid: null,
  priority: "none",
  progress: "needs-action",
  recurrenceId: null,
  recurring: false,
  sortOrder: null,
  title: id,
  uid: `${id}@uid`,
  updated: null,
  ...extra,
});

const onDay = (id: string, day: string, extra: Partial<Task> = {}) =>
  task(id, { due: { date: day, kind: "date" }, dueAt: at(day), ...extra });

const occurrence = (rid: string, day: string, completed = false) =>
  onDay(`w-${day}`, day, {
    baseId: CalendarEventId.parse("w"),
    completed,
    recurrenceId: rid,
    recurring: true,
    title: "weekly",
  });

const titles = (nodes: readonly { task: Task }[]) =>
  nodes.map((node) => node.task.title);

describe(todayView, () => {
  it("has overdue and today's tasks, high priority first", () => {
    const view = todayView(
      [
        onDay("late", "2026-10-27"),
        onDay("today-low", "2026-10-29", { priority: "low" }),
        onDay("today-high", "2026-10-29", { priority: "high" }),
        task("timed", {
          due: {
            kind: "datetime",
            local: "2026-10-29T18:00:00",
            timeZone: zone,
          },
          dueAt: at("2026-10-29", "18:00"),
        }),
        onDay("tomorrow", "2026-10-30"),
        onDay("done", "2026-10-29", { completed: true }),
        task("undated"),
      ],
      now,
      zone
    );
    expect(titles(view.overdue)).toStrictEqual(["late"]);
    expect(titles(view.today)).toStrictEqual([
      "today-high",
      "today-low",
      "timed",
    ]);
  });

  it("shows one occurrence per series: the earliest not done", () => {
    const tasks = [
      occurrence("a", "2026-10-15", true),
      occurrence("b", "2026-10-22"),
      occurrence("c", "2026-10-29"),
      occurrence("d", "2026-11-05"),
    ];
    const view = todayView(tasks, now, zone);
    expect(view.overdue.map((node) => node.task.id)).toStrictEqual([
      "w-2026-10-22",
    ]);
    expect(view.today).toStrictEqual([]);
    // Upcoming shows each one.
    expect(
      upcomingView(tasks, now, zone).map((group) => group.day)
    ).toStrictEqual(["2026-11-05"]);
    expect(completedView(tasks).map((item) => item.id)).toStrictEqual([
      "w-2026-10-15",
    ]);
  });
});

describe(todayCount, () => {
  it("counts Today's top rows, and answers the same view again", () => {
    const tasks = [
      onDay("late", "2026-10-27"),
      onDay("today", "2026-10-29"),
      task("sub", {
        due: { date: "2026-10-29", kind: "date" },
        parentUid: "today@uid",
      }),
      onDay("tomorrow", "2026-10-30"),
    ];
    expect(todayCount(tasks, now, zone)).toBe(2);
    const view = todayView(tasks, now, zone);
    expect(todayView(tasks, now + 60_000, zone)).toBe(view);
    expect(todayView([...tasks], now, zone)).not.toBe(view);
  });
});

describe(upcomingView, () => {
  it("groups the next 30 days by day, from tomorrow", () => {
    const groups = upcomingView(
      [
        onDay("today", "2026-10-29"),
        onDay("b", "2026-10-31"),
        onDay("a", "2026-10-30"),
        onDay("a2", "2026-10-30", { priority: "high" }),
        onDay("far", "2026-12-15"),
      ],
      now,
      zone
    );
    expect(
      groups.map((group) => [group.day, titles(group.tasks)])
    ).toStrictEqual([
      ["2026-10-30", ["a2", "a"]],
      ["2026-10-31", ["b"]],
    ]);
  });
});

describe(nestTasks, () => {
  it("puts subtasks under their parent, sorted", () => {
    const tree = nestTasks(
      [
        task("child-b", { parentUid: "parent@uid", sortOrder: 2 }),
        task("parent"),
        task("child-a", { parentUid: "parent@uid", sortOrder: 1 }),
        task("orphan", { parentUid: "gone@uid" }),
      ],
      "manual"
    );
    expect(titles(tree)).toStrictEqual(["orphan", "parent"]);
    expect(titles(tree[1]?.children ?? [])).toStrictEqual([
      "child-a",
      "child-b",
    ]);
  });

  it("shows every task of a loop of parents, once", () => {
    const tree = nestTasks(
      [task("a", { parentUid: "b@uid" }), task("b", { parentUid: "a@uid" })],
      "due"
    );
    expect(titles(tree)).toStrictEqual(["a"]);
    expect(titles(tree[0]?.children ?? [])).toStrictEqual(["b"]);
  });
});

describe("all, list and completed", () => {
  const tasks = [
    onDay("soon", "2026-10-30"),
    task("someday"),
    onDay("done", "2026-10-20", {
      completed: true,
      completedAt: at("2026-10-21"),
    }),
    task("older", { completed: true, completedAt: at("2026-10-01") }),
    task("other", { listKey: "c/l2" }),
  ];

  it("lists open tasks by due, undated last", () => {
    expect(titles(allView(tasks))).toStrictEqual(["soon", "other", "someday"]);
    expect(titles(listView(tasks, "c/l2"))).toStrictEqual(["other"]);
  });

  it("lists completed ones newest first", () => {
    expect(completedView(tasks).map((item) => item.title)).toStrictEqual([
      "done",
      "older",
    ]);
  });

  it("counts open tasks per list", () => {
    expect([...openCounts(tasks)]).toStrictEqual([
      ["c/l1", 2],
      ["c/l2", 1],
    ]);
  });
});
