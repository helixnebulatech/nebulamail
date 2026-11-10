// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Moving a task's due from the calendar: dragging it to
// another day or time (15-minute steps, like events) or Alt + arrows.
// Pure, with tests.
//
// A repeating task moves as a series (decided 2026-10-28, like every other
// change to a repeating task): the series' due shifts as much as the
// occurrence moved, so every occurrence moves.

import { fromAbsolute, toZoned } from "@internationalized/date";

import {
  formatLocalDateTime,
  parseLocalDate,
  parseLocalDateTime,
} from "../calendar/time";
import type { TaskDue } from "./model";

/** Where a task was dropped. */
export type DueDrop =
  /** A day of the month view: the same time (if any) on that day. */
  | { readonly kind: "day"; readonly date: string }
  /** The all-day row of the week or day view: a due day, no time. */
  | { readonly kind: "all-day"; readonly date: string }
  /** A time of the week or day view (minutes of that day, user's zone). */
  | { readonly kind: "time"; readonly date: string; readonly minutes: number };

const pad = (value: number): string => String(value).padStart(2, "0");

/** The wall date of a due ("YYYY-MM-DD"). */
const wallDate = (due: TaskDue): string =>
  due.kind === "date" ? due.date : due.local.slice(0, 10);

/** Whole days from `from` to `to` (dates); null if either isn't one. */
const daysBetween = (from: string, to: string): number | null => {
  const a = parseLocalDate(from);
  const b = parseLocalDate(to);
  return a === null || b === null ? null : b.compare(a);
};

const addDays = (date: string, days: number): string | null =>
  parseLocalDate(date)?.add({ days }).toString() ?? null;

/** The due a task gets when dropped on `drop` (`display`: user's zone). */
export const droppedDue = (
  due: TaskDue,
  drop: DueDrop,
  display: string,
  /** "YYYY-MM-DD" the task shows on now (user's zone). */
  shownDay: string
): TaskDue | null => {
  switch (drop.kind) {
    case "all-day": {
      return parseLocalDate(drop.date) === null
        ? null
        : { date: drop.date, kind: "date" };
    }
    case "day": {
      const days = daysBetween(shownDay, drop.date);
      if (days === null) {
        return null;
      }
      if (due.kind === "date") {
        const date = addDays(due.date, days);
        return date === null ? null : { date, kind: "date" };
      }
      const local = parseLocalDateTime(due.local);
      return local === null
        ? null
        : { ...due, local: formatLocalDateTime(local.add({ days })) };
    }
    case "time": {
      const date = parseLocalDate(drop.date);
      const minutes = Math.max(0, Math.min(1439, Math.round(drop.minutes)));
      if (date === null) {
        return null;
      }
      const wall = `${drop.date}T${pad(Math.floor(minutes / 60))}:${pad(minutes % 60)}:00`;
      // A timed task keeps its zone (or floating); a day becomes a time in
      // the user's zone, as the task panel does.
      const timeZone = due.kind === "datetime" ? due.timeZone : display;
      if (timeZone === null || timeZone === display) {
        return { kind: "datetime", local: wall, timeZone };
      }
      const instant = parseLocalDateTime(wall);
      if (instant === null) {
        return null;
      }
      const at = toZoned(instant, display).toDate().getTime();
      return {
        kind: "datetime",
        local: formatLocalDateTime(fromAbsolute(at, timeZone)),
        timeZone,
      };
    }
    default: {
      return drop satisfies never;
    }
  }
};

/**
 * The series' new due when one occurrence moved from `from` to `to`: the
 * same number of days, and the new time of day (or none).
 */
export const shiftedSeriesDue = (
  series: TaskDue,
  from: TaskDue,
  to: TaskDue
): TaskDue | null => {
  const days = daysBetween(wallDate(from), wallDate(to));
  if (days === null) {
    return null;
  }
  const date = addDays(wallDate(series), days);
  if (date === null) {
    return null;
  }
  return to.kind === "date"
    ? { date, kind: "date" }
    : { ...to, local: `${date}${to.local.slice(10)}` };
};

/**
 * Alt + arrows on a task in the calendar: ←/→ a day, ↑/↓ 15 minutes (a
 * timed task; a day moves a week, like the rows of the month). Null for
 * other keys.
 */
export const keyNudge = (event: {
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly key: string;
}): { readonly days: number; readonly minutes: number } | null => {
  if (!event.altKey || event.shiftKey) {
    return null;
  }
  switch (event.key) {
    case "ArrowLeft": {
      return { days: -1, minutes: 0 };
    }
    case "ArrowRight": {
      return { days: 1, minutes: 0 };
    }
    case "ArrowUp": {
      return { days: 0, minutes: -15 };
    }
    case "ArrowDown": {
      return { days: 0, minutes: 15 };
    }
    default: {
      return null;
    }
  }
};

/** A due moved by days and minutes (wall clock); a day moves ±7 for ↑/↓. */
export const nudgedDue = (
  due: TaskDue,
  nudge: { readonly days: number; readonly minutes: number }
): TaskDue | null => {
  if (due.kind === "date") {
    const days = nudge.days + Math.sign(nudge.minutes) * 7;
    const date = addDays(due.date, days);
    return date === null ? null : { date, kind: "date" };
  }
  const local = parseLocalDateTime(due.local);
  return local === null
    ? null
    : {
        ...due,
        local: formatLocalDateTime(
          local.add({ days: nudge.days, minutes: nudge.minutes })
        ),
      };
};
