// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// How a task's due date reads in a list ("Today 17:30", "Tomorrow",
// "Overdue · yesterday", "Thu", "6 Nov") and the quick choices for a new
// due date. Pure, with tests: the words come from the UI.

import { dateInZone, parseLocalDate } from "../calendar/time";
import type { Task, TaskDue } from "./model";
import { dueDay } from "./views";

export type DueWhen =
  | "yesterday"
  | "today"
  | "tomorrow"
  /** Within the next six days: the weekday is enough. */
  | "this-week"
  | "date";

export interface DueLabel {
  /** "YYYY-MM-DD" in the user's zone. */
  readonly day: string;
  readonly when: DueWhen;
  /** Epoch ms of a due time (null: a day without time). */
  readonly time: number | null;
  /** Not done and past due. */
  readonly overdue: boolean;
  /** Another year than now's (show the year). */
  readonly otherYear: boolean;
}

/** Days from `from` to `to` ("YYYY-MM-DD"), in whole days. */
const daysBetween = (from: string, to: string): number | null => {
  const a = parseLocalDate(from);
  const b = parseLocalDate(to);
  return a === null || b === null ? null : b.compare(a);
};

/** The chip of a task's due (null: no due). */
export const dueLabel = (
  task: Pick<Task, "due" | "dueAt" | "completed">,
  now: number,
  zone: string
): DueLabel | null => {
  const day = dueDay(task, zone);
  if (day === null || task.due === null) {
    return null;
  }
  const today = dateInZone(now, zone).toString();
  const offset = daysBetween(today, day) ?? 0;
  let when: DueWhen = "date";
  if (offset === -1) {
    when = "yesterday";
  } else if (offset === 0) {
    when = "today";
  } else if (offset === 1) {
    when = "tomorrow";
  } else if (offset > 1 && offset < 7) {
    when = "this-week";
  }
  const timed = task.due.kind === "datetime" && task.dueAt !== null;
  const overdue =
    !task.completed &&
    (offset < 0 || (timed && task.dueAt !== null && task.dueAt < now));
  return {
    day,
    otherYear: day.slice(0, 4) !== today.slice(0, 4),
    overdue,
    time: timed ? task.dueAt : null,
    when,
  };
};

export type DueChoice = "today" | "tomorrow" | "next-week" | "none";

/**
 * A due for a quick choice: a day (keeping the time of a due with one),
 * or none. "Next week" is the next Monday.
 */
export const dueForChoice = (
  choice: DueChoice,
  current: TaskDue | null,
  now: number,
  zone: string
): TaskDue | null => {
  if (choice === "none") {
    return null;
  }
  const today = dateInZone(now, zone);
  let day = today;
  if (choice === "tomorrow") {
    day = today.add({ days: 1 });
  } else if (choice === "next-week") {
    // ISO weekday: Monday 1 … Sunday 7 (Date's getUTCDay: Sunday 0).
    const weekday =
      new Date(Date.UTC(today.year, today.month - 1, today.day)).getUTCDay() ||
      7;
    day = today.add({ days: 8 - weekday });
  }
  const date = day.toString();
  if (current?.kind === "datetime") {
    return {
      kind: "datetime",
      local: `${date}${current.local.slice(10)}`,
      timeZone: current.timeZone,
    };
  }
  return { date, kind: "date" };
};
