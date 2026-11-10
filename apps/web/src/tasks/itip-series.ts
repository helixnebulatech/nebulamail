// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A repeating task as the iTIP REQUEST to a delegate carries it.
// Pure, with tests
// (assignment.test.ts).
//
// - DTSTART is the series' `start` (the app always writes `start` = `due`
//   in a repeating task, for a Stalwart quirk: it anchors the
//   RRULE and the alerts), else its `due` (a task from another client with
//   only DUE). Times stay in the task's zone (`timeZone`), with a VTIMEZONE,
//   so a 9:00 task stays at 9:00 after a DST change; a series of days is
//   written as DATE values; a floating one, floating.
// - Occurrences taken out (`excluded`) → EXDATE. Every other override key
//   → RDATE (it may be an added occurrence, RFC 8984 §4.3.5; RRULE and
//   RDATE are a union, RFC 5545 §3.8.5.2). Occurrences moved, renamed or
//   done → one VTODO each with RECURRENCE-ID.
// - Not sent (lost): alerts and links (never sent, like a single task),
//   what else an override changes (notes, priority, alerts…), and the
//   delegator's own status in each occurrence. A series that can't be
//   written exactly (EXRULE, an override in another zone, an odd rule)
//   gives null: the app doesn't offer to pass it on.

import type { RecurrenceRule } from "@nebulamail/jmap-client";

import type { ICalTime } from "../calendar/ical-series";
import { rruleOf } from "../calendar/ical-series";
import type { ItipOverride, ItipSeries } from "../calendar/itip";
import { seriesWritable } from "../calendar/itip";
import { formatLocalDateTime, parseLocalDateTime } from "../calendar/time";

/** What `taskSeries` reads (a stored or a parsed task). */
export interface SeriesTask {
  readonly recurrenceRule?: RecurrenceRule | null | undefined;
  readonly excludedRecurrenceRules?: readonly unknown[] | null | undefined;
  readonly recurrenceOverrides?:
    | Readonly<Record<string, Readonly<Record<string, unknown>>>>
    | null
    | undefined;
  readonly start?: string | null | undefined;
  readonly due?: string | null | undefined;
  readonly timeZone?: string | null | undefined;
  readonly showWithoutTime?: boolean | null | undefined;
}

/** Whether a task repeats (has a rule). */
export const repeats = (task: SeriesTask): boolean =>
  task.recurrenceRule !== null && task.recurrenceRule !== undefined;

const MINUTE = 60_000;

/** Wall-clock ms of a local date-time (read as UTC), to add gaps. */
const wallMs = (local: string): number | null => {
  const parsed = parseLocalDateTime(local);
  return parsed === null
    ? null
    : Date.UTC(
        parsed.year,
        parsed.month - 1,
        parsed.day,
        parsed.hour,
        parsed.minute,
        parsed.second
      );
};

const wallLocal = (ms: number): string =>
  new Date(ms).toISOString().slice(0, 19);

/** Override keys that change nothing the message carries. */
const ZONE_KEYS = ["timeZone", "showWithoutTime", "recurrenceRule"] as const;

/** Reads the series' times: days, or wall times in its zone. */
type TimeReader = (value: string) => ICalTime | null;

const readTime = (
  value: string,
  allDay: boolean,
  zone: string | null
): ICalTime | null => {
  const parsed = parseLocalDateTime(value);
  if (parsed === null) {
    return null;
  }
  const local = formatLocalDateTime(parsed);
  return allDay
    ? { date: local.slice(0, 10), kind: "date" }
    : { kind: "local", local, timeZone: zone };
};

const timeReader = (task: SeriesTask): TimeReader => {
  const allDay = task.showWithoutTime === true;
  const zone = allDay ? null : (task.timeZone ?? null);
  return (value) => readTime(value, allDay, zone);
};

/** One override: taken out, one that differs, nothing to send, or invalid. */
type OverrideRead =
  | { readonly kind: "excluded"; readonly id: ICalTime }
  | { readonly kind: "override"; readonly override: ItipOverride }
  | { readonly kind: "same"; readonly id: ICalTime }
  | { readonly kind: "invalid" };

const readOverride = (
  key: string,
  patch: Readonly<Record<string, unknown>>,
  timeOf: TimeReader,
  /** Minutes from start to due in the series (null: no due). */
  gap: number | null
): OverrideRead => {
  const id = timeOf(key);
  if (id === null || ZONE_KEYS.some((name) => name in patch)) {
    return { kind: "invalid" };
  }
  if (patch.excluded === true) {
    return { id, kind: "excluded" };
  }
  const startValue = typeof patch.start === "string" ? patch.start : key;
  const startMs = wallMs(startValue);
  let dueValue: string | null =
    typeof patch.due === "string" ? patch.due : null;
  if (dueValue === null && gap !== null && startMs !== null) {
    dueValue = wallLocal(startMs + gap * MINUTE);
  }
  const start = timeOf(startValue);
  const due = dueValue === null ? null : timeOf(dueValue);
  if (start === null || (dueValue !== null && due === null)) {
    return { kind: "invalid" };
  }
  const moved = startMs !== wallMs(key) || typeof patch.due === "string";
  const title = typeof patch.title === "string" ? patch.title : null;
  const completed = patch.progress === "completed";
  // Only what the delegate sees differently from the series.
  return moved || title !== null || completed
    ? {
        kind: "override",
        override: { completed, due, recurrenceId: id, start, title },
      }
    : { id, kind: "same" };
};

/** Wall-clock minutes from `from` to `to` (null if either is missing). */
const gapMinutes = (
  from: string,
  to: string | null | undefined
): number | null => {
  const start = wallMs(from);
  const end = typeof to === "string" ? wallMs(to) : null;
  return start === null || end === null
    ? null
    : Math.round((end - start) / MINUTE);
};

/** DTSTART, DUE and RRULE of a series; null if any can't be read. */
const seriesHead = (
  task: SeriesTask,
  timeOf: TimeReader
): Pick<ItipSeries, "start" | "due" | "rrule"> | null => {
  const rule = task.recurrenceRule;
  const anchor = task.start ?? task.due ?? null;
  if (rule === null || rule === undefined || anchor === null) {
    return null;
  }
  const start = timeOf(anchor);
  const due = typeof task.due === "string" ? timeOf(task.due) : null;
  const rrule = start === null ? null : rruleOf(rule, start);
  if (
    start === null ||
    rrule === null ||
    (typeof task.due === "string" && due === null)
  ) {
    return null;
  }
  return { due, rrule, start };
};

/**
 * The series of a repeating task for an iTIP message; null if the task
 * doesn't repeat or the series can't be written exactly.
 */
export const taskSeries = (task: SeriesTask): ItipSeries | null => {
  const timeOf = timeReader(task);
  const head =
    (task.excludedRecurrenceRules?.length ?? 0) > 0
      ? null
      : seriesHead(task, timeOf);
  if (head === null) {
    return null;
  }
  const gap = gapMinutes(task.start ?? task.due ?? "", task.due);
  const excluded: ICalTime[] = [];
  const added: ICalTime[] = [];
  const overrides: ItipOverride[] = [];
  const entries = Object.entries(task.recurrenceOverrides ?? {}).toSorted(
    ([one], [other]) => one.localeCompare(other)
  );
  for (const [key, patch] of entries) {
    const read = readOverride(key, patch, timeOf, gap);
    if (read.kind === "invalid") {
      return null;
    }
    if (read.kind === "excluded") {
      excluded.push(read.id);
    } else if (read.kind === "override") {
      added.push(read.override.recurrenceId);
      overrides.push(read.override);
    } else {
      added.push(read.id);
    }
  }
  // Every occurrence that isn't taken out goes as an RDATE too: a key the
  // rule doesn't make is an added occurrence (RFC 8984 §4.3.5), and the
  // set is the union of RRULE and RDATE with duplicates ignored (RFC 5545
  // §3.8.5.2), so the delegate gets exactly the same set without the app
  // expanding the rule itself.
  const series: ItipSeries = {
    ...head,
    ...(added.length > 0 ? { added } : {}),
    excluded,
    overrides,
  };
  return seriesWritable(series) ? series : null;
};
