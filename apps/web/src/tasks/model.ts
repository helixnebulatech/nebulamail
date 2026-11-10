// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// View types of Tasks (the UI never sees JMAP) and
// the mapping from stored JSCalendar tasks. Pure, with tests. Task text
// (title, notes) may come from other people's clients: show it with
// `cleanDisplayText` inside `<bdi>`, never as HTML.
//
// Stalwart (checked live): occurrences of an
// expanded query keep `@type` and move `start`, but not `due` (every
// occurrence has the series' due); the occurrence's due is worked out
// here from its `utcStart` and the series' gap between start and due.

import { fromAbsolute, toZoned } from "@internationalized/date";
import type {
  AccountId,
  CalendarEventId,
  CalendarId,
  Lenient,
  StoredTask,
} from "@nebulamail/jmap-client";
import {
  completedAt,
  icalValue,
  parentUids,
  taskUtc,
} from "@nebulamail/jmap-client";

import { calendarKey } from "../calendar/model";
import {
  durationMinutes,
  eventZone,
  formatLocalDateTime,
  parseDuration,
  parseLocalDate,
  parseLocalDateTime,
  startOfDayMs,
} from "../calendar/time";
import { dayMinutesBefore } from "./day-reminders";

/** A stored task or occurrence as the api reads it. */
export type RawTask = Lenient<StoredTask> & { readonly id: CalendarEventId };

export type TaskPriority = "none" | "low" | "medium" | "high";

/** RFC 5545 PRIORITY of each level (decided 2026-10-28: 0/9/5/1). */
export const PRIORITY_VALUES: Readonly<Record<TaskPriority, number>> = {
  high: 1,
  low: 9,
  medium: 5,
  none: 0,
};

/** RFC 5545 §3.8.1.9: 1–4 high, 5 medium, 6–9 low, 0 or none undefined. */
export const toPriority = (value: number | null | undefined): TaskPriority => {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "none";
  }
  const level = Math.trunc(value);
  if (level >= 1 && level <= 4) {
    return "high";
  }
  if (level === 5) {
    return "medium";
  }
  return level >= 6 && level <= 9 ? "low" : "none";
};

/** High first, none last. */
export const PRIORITY_RANK: Readonly<Record<TaskPriority, number>> = {
  high: 0,
  low: 2,
  medium: 1,
  none: 3,
};

/** When a task is due: a day, or a day and time (in a zone or floating). */
export type TaskDue =
  | { readonly kind: "date"; readonly date: string }
  | {
      readonly kind: "datetime";
      /** Wall time, "2026-11-06T17:30:00". */
      readonly local: string;
      /** IANA zone; null: floating (the same wall time everywhere). */
      readonly timeZone: string | null;
    };

/** A reminder: at a moment, or some minutes before the due time. */
export interface TaskAlert {
  /** Key in the task's `alerts` (kept when editing). */
  readonly key: string;
  /** When it fires (epoch ms); null if it can't be worked out. */
  readonly at: number | null;
  /** Minutes before the due time (negative: after); null: not tied to it. */
  readonly minutesBefore: number | null;
}

/** A link of a task (the email it came from, a web page). Untrusted. */
export interface TaskLink {
  readonly key: string;
  readonly href: string;
  readonly title: string | null;
}

export interface Task {
  /** The occurrence's id (a repeating task), else the stored task's. */
  readonly id: CalendarEventId;
  /** The stored task (edits the whole series). */
  readonly baseId: CalendarEventId;
  readonly accountId: AccountId;
  readonly calendarId: CalendarId | null;
  /** `calendarKey` of its list (null: none). */
  readonly listKey: string | null;
  readonly uid: string | null;
  readonly title: string;
  readonly notes: string | null;
  readonly due: TaskDue | null;
  /**
   * Epoch ms of the due moment: a day's midnight in the user's zone, else
   * the exact time. For sorting and grouping; null: no due date.
   */
  readonly dueAt: number | null;
  readonly completed: boolean;
  /** COMPLETED of the task, else when it was last updated (null: unknown). */
  readonly completedAt: number | null;
  /** RFC 8984 progress ("needs-action", "in-process", "completed"…). */
  readonly progress: string;
  readonly priority: TaskPriority;
  /** X-APPLE-SORT-ORDER, the manual order other clients keep (lower first). */
  readonly sortOrder: number | null;
  /** UID of the parent task (subtasks); null at the top. */
  readonly parentUid: string | null;
  readonly recurring: boolean;
  /** This occurrence of a repeating task (null: not an occurrence). */
  readonly recurrenceId: string | null;
  readonly alerts: readonly TaskAlert[];
  readonly keywords: readonly string[];
  readonly links: readonly TaskLink[];
  /**
   * RFC 8984 §5.2.3 `estimatedDuration` in whole minutes (a day counts as
   * 24 h); null: none, or not a positive Duration (§1.4.6).
   */
  readonly estimatedMinutes: number | null;
  /** Epoch ms (null: unknown). */
  readonly created: number | null;
  readonly updated: number | null;
}

/** A calendar the tasks can live in (a "list"). */
export interface TaskList {
  readonly key: string;
  readonly accountId: AccountId;
  readonly id: CalendarId;
  readonly name: string;
  readonly color: string;
  /** Who shares it; null for the user's own. */
  readonly owner: string | null;
  readonly mayWrite: boolean;
  readonly isDefault: boolean;
  /** Tasks in it now (not counting completed ones). */
  readonly open: number;
}

const COMPLETED = "completed";

const isoMs = (value: string | null | undefined): number | null => {
  if (value === null || value === undefined) {
    return null;
  }
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
};

const firstCalendar = (
  calendarIds: Readonly<Record<string, boolean>> | null | undefined
): CalendarId | null => {
  const id = Object.entries(calendarIds ?? {}).find(([, on]) => on)?.[0];
  // Ids come from the server's own response, already checked by the client.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return id === undefined ? null : (id as CalendarId);
};

/** The zone of a task's wall times (floating: `display`). */
const zoneOf = (task: RawTask, display: string): string =>
  eventZone({ timeZone: task.timeZone }, display);

/** The due of a task as stored (date or wall time). */
export const dueOf = (task: RawTask): TaskDue | null => {
  if (task.showWithoutTime === true) {
    const date = parseLocalDate(task.due);
    return date === null ? null : { date: date.toString(), kind: "date" };
  }
  const local = parseLocalDateTime(task.due);
  return local === null
    ? null
    : {
        kind: "datetime",
        local: formatLocalDateTime(local),
        timeZone: task.timeZone ?? null,
      };
};

/** Epoch ms of a due: a day's midnight in `display`, else the exact time. */
export const dueInstant = (due: TaskDue, display: string): number | null => {
  if (due.kind === "date") {
    const date = parseLocalDate(due.date);
    return date === null ? null : startOfDayMs(date, display);
  }
  const local = parseLocalDateTime(due.local);
  return local === null
    ? null
    : toZoned(local, eventZone({ timeZone: due.timeZone }, display))
        .toDate()
        .getTime();
};

/** Minutes from a task's start to its due (wall time); null if not both. */
const startToDueMinutes = (task: RawTask): number | null => {
  const start = parseLocalDateTime(task.start);
  const due = parseLocalDateTime(task.due);
  if (start === null || due === null) {
    return null;
  }
  return Math.round(
    (toZoned(due, "UTC").toDate().getTime() -
      toZoned(start, "UTC").toDate().getTime()) /
      60_000
  );
};

/**
 * The due of one occurrence of a repeating task. Stalwart moves `start`
 * (and `utcStart`) but leaves `due` as the series' first one, so the due is
 * the occurrence's start plus the series' gap from start to due. A series
 * with a due but no start (invalid iCalendar, but some clients write it):
 * Stalwart takes the due as the start.
 */
export const occurrenceDue = (
  occurrence: RawTask,
  series: RawTask,
  display: string
): TaskDue | null => {
  const seriesDue = dueOf(series);
  if (seriesDue === null) {
    return null;
  }
  const gap = startToDueMinutes(series) ?? 0;
  if (seriesDue.kind === "date") {
    const day = parseLocalDate(occurrence.start ?? occurrence.recurrenceId);
    return day === null
      ? seriesDue
      : {
          date: day.add({ days: Math.round(gap / 1440) }).toString(),
          kind: "date",
        };
  }
  const utc = taskUtc(occurrence.utcStart);
  if (utc === null) {
    return seriesDue;
  }
  const zone = zoneOf(series, display);
  return {
    kind: "datetime",
    local: formatLocalDateTime(fromAbsolute(utc + gap * 60_000, zone)),
    timeZone: seriesDue.timeZone,
  };
};

const minutesOfOffset = (offset: string | undefined): number | null => {
  const duration = parseDuration(offset);
  return duration === null ? null : durationMinutes(duration);
};

/**
 * Minutes before the due of an alert at `at`: on the wall clock for a day
 * (so "1 day before at 9:00" reads the same across a change of daylight
 * saving time), else exact.
 */
const beforeDue = (
  at: number | null,
  dueAt: number | null,
  due: TaskDue | null,
  display: string
): number | null => {
  if (at === null || dueAt === null) {
    return null;
  }
  if (due?.kind === "date") {
    return dayMinutesBefore(due.date, at, display);
  }
  return Math.round((dueAt - at) / 60_000);
};

/** The reminders of a task (or occurrence), against its due. */
export const alertsOf = (
  task: RawTask,
  dueAt: number | null,
  display: string,
  due: TaskDue | null = null
): TaskAlert[] => {
  const startAt = (() => {
    const start = parseLocalDateTime(task.start);
    return start === null
      ? null
      : toZoned(
          start,
          task.showWithoutTime === true ? display : zoneOf(task, display)
        )
          .toDate()
          .getTime();
  })();
  return Object.entries(task.alerts ?? {}).flatMap(([key, alert]) => {
    const { trigger } = alert;
    if (trigger.when !== undefined) {
      const at = isoMs(trigger.when);
      return [{ at, key, minutesBefore: beforeDue(at, dueAt, due, display) }];
    }
    const offset = minutesOfOffset(trigger.offset);
    if (offset === null) {
      return [{ at: null, key, minutesBefore: null }];
    }
    // RFC 8984: relative to the start, or the due ("end") of a task. A
    // task with a due and no start: Stalwart counts from the due.
    const anchor = trigger.relativeTo === "end" ? dueAt : (startAt ?? dueAt);
    const at = anchor === null ? null : anchor + offset * 60_000;
    return [
      {
        at,
        key,
        minutesBefore:
          at === null || dueAt === null
            ? null
            : Math.round((dueAt - at) / 60_000),
      },
    ];
  });
};

const sortOrderOf = (task: RawTask): number | null => {
  const value = icalValue(task, "x-apple-sort-order");
  if (typeof value !== "number" && typeof value !== "string") {
    return null;
  }
  const number = Number(value);
  return Number.isFinite(number) && String(value).trim() !== ""
    ? Math.trunc(number)
    : null;
};

const linksOf = (task: RawTask): TaskLink[] =>
  Object.entries(task.links ?? {}).flatMap(([key, link]) =>
    typeof link.href === "string" && link.href !== ""
      ? [
          {
            href: link.href,
            key,
            title: typeof link.title === "string" ? link.title : null,
          },
        ]
      : []
  );

/**
 * Minutes of a task's `estimatedDuration` (RFC 8984 §5.2.3: a positive
 * Duration, §1.4.6). Anything else (negative, zero, not a Duration) reads
 * as none; the stored value stays as it is until the user changes it.
 */
export const estimatedMinutesOf = (
  value: string | null | undefined
): number | null => {
  if (typeof value !== "string" || !value.startsWith("P")) {
    return null;
  }
  const duration = parseDuration(value);
  if (duration === null) {
    return null;
  }
  const minutes = Math.round(durationMinutes(duration));
  return minutes > 0 ? minutes : null;
};

const keywordsOf = (task: RawTask): string[] =>
  Object.entries(task.keywords ?? {})
    .filter(([, on]) => on)
    .map(([word]) => word);

export interface TaskContext {
  /** The user's zone. */
  readonly display: string;
  /** The account the tasks come from. */
  readonly accountId: AccountId;
}

/**
 * A task's progress (lower-case). Omitted, RFC 8984 §5.2.5 defines it
 * from the participants' "in order of evaluation": "completed" if the
 * "progress" property value of all participants is "completed"; "failed"
 * if at least one is "failed"; "in-process" if at least one is
 * "in-process"; "needs-action" if none of the other criteria match. A
 * participant without `progress` isn't "completed", and with no
 * participants at all nothing is (not a vacuous "all").
 */
export const taskProgress = (task: {
  readonly progress?: string | null | undefined;
  readonly participants?:
    | Readonly<
        Record<string, { readonly progress?: string | null | undefined }>
      >
    | null
    | undefined;
}): string => {
  if (typeof task.progress === "string" && task.progress !== "") {
    return task.progress.toLowerCase();
  }
  const values = Object.values(task.participants ?? {}).map(
    (participant) => participant.progress?.toLowerCase() ?? null
  );
  if (values.length > 0 && values.every((value) => value === COMPLETED)) {
    return COMPLETED;
  }
  if (values.includes("failed")) {
    return "failed";
  }
  return values.includes("in-process") ? "in-process" : "needs-action";
};

/**
 * A stored task (or one occurrence, with its `series`) for the views; null
 * if it isn't a task.
 */
export const toTask = (
  raw: RawTask,
  series: RawTask | undefined,
  context: TaskContext
): Task | null => {
  const base = series ?? raw;
  // An occurrence's own values win over its series'.
  const task: RawTask = {
    ...base,
    ...Object.fromEntries(
      Object.entries(raw).filter(([, value]) => value !== undefined)
    ),
    id: raw.id,
  };
  if (task["@type"] !== "Task") {
    return null;
  }
  const isOccurrence = series !== undefined && raw.id !== series.id;
  const due = isOccurrence
    ? occurrenceDue(raw, series, context.display)
    : dueOf(raw);
  const dueAt = due === null ? null : dueInstant(due, context.display);
  const progress = taskProgress(task);
  const completed = progress === COMPLETED;
  const calendarId = firstCalendar(task.calendarIds);
  const notes = task.description ?? "";
  return {
    accountId: context.accountId,
    alerts: alertsOf(task, dueAt, context.display, due),
    baseId: raw.baseEventId ?? base.id,
    calendarId,
    completed,
    completedAt: completed ? (completedAt(task) ?? isoMs(task.updated)) : null,
    created: isoMs(base.created),
    due,
    dueAt,
    estimatedMinutes: estimatedMinutesOf(task.estimatedDuration),
    id: raw.id,
    keywords: keywordsOf(base),
    links: linksOf(base),
    listKey:
      calendarId === null ? null : calendarKey(context.accountId, calendarId),
    notes: notes === "" ? null : notes,
    parentUid: parentUids(base)[0] ?? null,
    priority: toPriority(task.priority),
    progress,
    recurrenceId: isOccurrence ? (raw.recurrenceId ?? null) : null,
    recurring:
      base.recurrenceRule !== null && base.recurrenceRule !== undefined,
    sortOrder: sortOrderOf(base),
    title: (task.title ?? "").trim(),
    uid: base.uid ?? null,
    updated: isoMs(task.updated),
  };
};
