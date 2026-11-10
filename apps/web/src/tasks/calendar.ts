// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks in the calendar ("Show tasks", on by
// default, per device): each dated task on its day, done ones too (struck
// through), every occurrence of a repeating one. Pure, with tests.

import type { Task } from "./model";
import { PRIORITY_RANK } from "./model";
import { dueDay } from "./views";

/** Where older versions kept "Show tasks" (localStorage; lib/use-layout.ts moves it). */
export const SHOW_TASKS_KEY = "wm-calendar-tasks";

export interface DayTasks {
  /** Due that day without a time. */
  readonly allDay: readonly Task[];
  /** Due at a time that day, by time. */
  readonly timed: readonly Task[];
}

const EMPTY: DayTasks = { allDay: [], timed: [] };

/**
 * Tasks by "YYYY-MM-DD" of the user's zone, for the days asked. Open ones
 * first, then by priority and title (all-day) or by time (timed).
 */
export const tasksByDay = (
  tasks: readonly Task[],
  days: readonly string[],
  zone: string
): Map<string, DayTasks> => {
  const wanted = new Set(days);
  const found = new Map<string, Task[]>();
  for (const task of tasks) {
    const day = dueDay(task, zone);
    if (day !== null && wanted.has(day)) {
      const list = found.get(day);
      if (list === undefined) {
        found.set(day, [task]);
      } else {
        list.push(task);
      }
    }
  }
  const out = new Map<string, DayTasks>();
  for (const [day, list] of found) {
    const allDay = list
      .filter((task) => task.due?.kind === "date")
      .toSorted(
        (a, b) =>
          Number(a.completed) - Number(b.completed) ||
          PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority] ||
          a.title.localeCompare(b.title)
      );
    const timed = list
      .filter((task) => task.due?.kind === "datetime")
      .toSorted(
        (a, b) =>
          (a.dueAt ?? 0) - (b.dueAt ?? 0) || a.title.localeCompare(b.title)
      );
    out.set(day, { allDay, timed });
  }
  return out;
};

/** The tasks of one day (none: empty lists). */
export const tasksOn = (
  byDay: ReadonlyMap<string, DayTasks>,
  day: string
): DayTasks => byDay.get(day) ?? EMPTY;
