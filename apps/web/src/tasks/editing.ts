// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Changes to tasks as JMAP patches, and their undo.
// Pure, with tests. Every change patches by path, so what the app doesn't
// edit stays as it was (`iCalendar.properties` like X-APPLE-SORT-ORDER,
// CLASS → `privacy`, alarms' extras, other relations).
//
// What Stalwart needs (checked live):
//
// - A path into a map that isn't there fails ("recurrenceOverrides/…",
//   "relatedTo/…", "iCalendar/properties" of a task without `iCalendar`):
//   then the whole map is written (`mapPatch`). Get leaves empty maps out.
// - It doesn't unescape "~1" in a path: keys with "/" or "~" (UIDs may
//   have them) rewrite the whole map.
// - Completed = `progress: "completed"` + COMPLETED in `iCalendar`
//   (iCalendar form, written as is). Completing one occurrence of a
//   repeating task: `recurrenceOverrides/<recurrenceId>/progress`, all-day
//   series too.
// - Alerts: relative to the due ("end") only works when the task has a
//   start too (without one Stalwart counts to the end of the due day), and
//   an RRULE needs a DTSTART. So a repeating task always gets `start` =
//   its due, and its alerts are offsets from the due; a single task's
//   alerts are absolute times (every client fires those).

import type {
  AccountId,
  CalendarEventId,
  CalendarId,
  RecurrenceRule,
} from "@nebulamail/jmap-client";
import { completedProperty, withICalProperty } from "@nebulamail/jmap-client";

import { utcDateTime } from "../calendar/alerts";
import {
  formatDuration,
  formatOffset,
  parseLocalDateTime,
} from "../calendar/time";
import {
  DEFAULT_REMINDER_MINUTES,
  convertReminder,
  dayReminderInstant,
} from "./day-reminders";
import type { RawTask, Task, TaskDue, TaskPriority } from "./model";
import { PRIORITY_VALUES, dueInstant, dueOf } from "./model";

export type Patch = Readonly<Record<string, unknown>>;

/** One `CalendarEvent/set` in one account: what a change does. */
export interface TaskChange {
  readonly accountId: AccountId;
  readonly create?: Readonly<Record<string, Record<string, unknown>>>;
  readonly update?: Readonly<Partial<Record<CalendarEventId, Patch>>>;
  readonly destroy?: readonly CalendarEventId[];
  /**
   * `sendSchedulingMessages`: the task is assigned to others and the user
   * assigned it (tasks/assignment.ts, `scheduleChange`).
   */
  readonly notify?: boolean;
}

// ---------------------------------------------------------------- paths

const needsEscape = (key: string): boolean =>
  key.includes("/") || key.includes("~");

/** RFC 6901 escaping of one path segment. */
export const escapePointer = (key: string): string =>
  key.replaceAll("~", "~0").replaceAll("/", "~1");

const unescapePointer = (segment: string): string =>
  segment.replaceAll("~1", "/").replaceAll("~0", "~");

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Sets or removes (null) keys of a map property. Paths when the map is
 * there and the keys need no escaping; else the whole map, merged.
 */
/**
 * An override always carries its start, so the instance a server writes to
 * CalDAV always has DTSTART (strict clients like DAVx5 refuse a calendar
 * without it; support.stalw.art/t/2007). The start of an occurrence that
 * hasn't moved is its recurrence id.
 */
export const withStart = (
  recurrenceId: string,
  override: Readonly<Record<string, unknown>>
): Record<string, unknown> =>
  override.start === undefined
    ? { ...override, start: recurrenceId }
    : { ...override };

export const mapPatch = (
  property: string,
  existing: unknown,
  changes: Readonly<Record<string, unknown>>
): Record<string, unknown> => {
  const keys = Object.keys(changes);
  if (keys.length === 0) {
    return {};
  }
  if (isRecord(existing) && !keys.some(needsEscape)) {
    return Object.fromEntries(
      keys.map((key) => [`${property}/${key}`, changes[key]])
    );
  }
  const merged: Record<string, unknown> = isRecord(existing)
    ? { ...existing }
    : {};
  for (const key of keys) {
    if (changes[key] === null) {
      Reflect.deleteProperty(merged, key);
    } else {
      merged[key] = changes[key];
    }
  }
  return { [property]: Object.keys(merged).length === 0 ? null : merged };
};

/** The value at a patch path of `raw` (undefined: not there). */
export const valueAt = (raw: unknown, path: string): unknown => {
  let value: unknown = raw;
  for (const segment of path.split("/").map(unescapePointer)) {
    if (!isRecord(value)) {
      return undefined;
    }
    value = value[segment];
  }
  return value;
};

const isProgressUpdated = (path: string): boolean =>
  path === "progressUpdated" || path.endsWith("/progressUpdated");

/**
 * The patch that undoes `patch` on `raw` (what each path had before).
 * A `progressUpdated` (RFC 8984 §5.2.6) the app didn't see is left as the
 * change wrote it rather than erased: Stalwart 1.0.1 stores the task's
 * own one but leaves it out of `CalendarEvent/get` when `properties` are
 * listed (checked live 2026-11-08), so "not there" may mean "not shown".
 */
export const inversePatch = (raw: RawTask, patch: Patch): Patch =>
  Object.fromEntries(
    Object.keys(patch).flatMap((path) => {
      const before = valueAt(raw, path);
      return before === undefined && isProgressUpdated(path)
        ? []
        : [[path, before ?? null]];
    })
  );

/** Server-set properties left out when a deleted task is created again. */
const SERVER_PROPERTIES = new Set([
  "baseEventId",
  "id",
  "isDraft",
  "isOrigin",
  "utcEnd",
  "utcStart",
]);

/**
 * Undo of a change, from the stored tasks before it and the ids the
 * server gave to what it created: patches back, created tasks deleted,
 * deleted tasks created again (same UID).
 */
export const undoOf = (
  change: TaskChange,
  before: ReadonlyMap<CalendarEventId, RawTask>,
  created: Readonly<Record<string, CalendarEventId>>
): TaskChange => {
  const update: Partial<Record<CalendarEventId, Patch>> = {};
  for (const [id, patch] of Object.entries(change.update ?? {})) {
    // Keys come from the change's own update map.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    const key = id as CalendarEventId;
    const raw = before.get(key);
    if (raw !== undefined && patch !== undefined) {
      update[key] = inversePatch(raw, patch);
    }
  }
  const create: Record<string, Record<string, unknown>> = {};
  for (const id of change.destroy ?? []) {
    const raw = before.get(id);
    if (raw !== undefined) {
      create[`undo-${id}`] = Object.fromEntries(
        Object.entries(raw).filter(
          ([property, value]) =>
            !SERVER_PROPERTIES.has(property) && value !== undefined
        )
      );
    }
  }
  return {
    accountId: change.accountId,
    create,
    destroy: Object.values(created),
    update,
  };
};

// ---------------------------------------------------------------- iCalendar

/** Writes `iCalendar.properties` (the whole component if there is none). */
const icalPatch = (
  series: RawTask,
  properties: readonly unknown[]
): Record<string, unknown> => {
  if (isRecord(series.iCalendar)) {
    return {
      "iCalendar/properties": properties.length === 0 ? null : properties,
    };
  }
  return properties.length === 0
    ? {}
    : { iCalendar: { name: "vtodo", properties } };
};

/** The manual order (X-APPLE-SORT-ORDER, what Apple and others keep). */
export const sortOrderPatch = (
  series: RawTask,
  order: number | null
): Record<string, unknown> =>
  icalPatch(
    series,
    withICalProperty(
      series,
      "x-apple-sort-order",
      order === null
        ? null
        : ["x-apple-sort-order", {}, "integer", Math.trunc(order)]
    )
  );

// ---------------------------------------------------------------- creating

export interface NewTask {
  readonly calendarId: CalendarId;
  readonly title: string;
  readonly notes?: string;
  readonly due?: TaskDue | null;
  readonly priority?: TaskPriority;
  /** UID of the parent (a subtask). */
  readonly parentUid?: string | null;
  /** Minutes before the due, or absolute times (epoch ms). */
  readonly alerts?: readonly AlertInput[];
  /** A JSCalendar RecurrenceRule (needs a due). */
  readonly recurrenceRule?: RecurrenceRule | null;
  /** Links (a `mid:` link to the email it came from, the app's link). */
  readonly links?: readonly {
    readonly href: string;
    readonly title?: string;
  }[];
}

export type AlertInput =
  | { readonly key?: string; readonly minutesBefore: number }
  | { readonly key?: string; readonly at: number };

/** `due` and its companions for a due (null: none). */
const dueProperties = (due: TaskDue | null): Record<string, unknown> => {
  if (due === null) {
    return { due: null };
  }
  return due.kind === "date"
    ? { due: `${due.date}T00:00:00`, showWithoutTime: true, timeZone: null }
    : { due: due.local, showWithoutTime: false, timeZone: due.timeZone };
};

const dueValue = (due: TaskDue): string =>
  due.kind === "date" ? `${due.date}T00:00:00` : due.local;

const triggerOf = (
  alert: AlertInput,
  due: TaskDue | null,
  recurring: boolean,
  display: string
): Record<string, unknown> | null => {
  if ("at" in alert) {
    return { "@type": "AbsoluteTrigger", when: utcDateTime(alert.at) };
  }
  if (due === null) {
    return null;
  }
  if (recurring) {
    return {
      "@type": "OffsetTrigger",
      offset: formatOffset(-alert.minutesBefore),
      relativeTo: "end",
    };
  }
  const at =
    due.kind === "date"
      ? dayReminderInstant(due.date, alert.minutesBefore, display)
      : dueInstant(due, display);
  const when =
    at === null || due.kind === "date" ? at : at - alert.minutesBefore * 60_000;
  return when === null
    ? null
    : {
        "@type": "AbsoluteTrigger",
        when: utcDateTime(when),
      };
};

const newAlertKey = (index: number): string =>
  `a${Date.now().toString(36)}${String(index)}`;

/** JSCalendar alerts for `inputs` (keys kept or made). */
const alertsFor = (
  inputs: readonly AlertInput[],
  due: TaskDue | null,
  recurring: boolean,
  display: string
): Record<string, Record<string, unknown>> =>
  Object.fromEntries(
    inputs.flatMap((alert, index) => {
      const trigger = triggerOf(alert, due, recurring, display);
      return trigger === null
        ? []
        : [
            [
              alert.key ?? newAlertKey(index),
              { "@type": "Alert", action: "display", trigger },
            ],
          ];
    })
  );

const relation = { "@type": "Relation", relation: { parent: true } };

/** Properties of a new task for `CalendarEvent/set` create. */
export const newTaskProperties = (
  input: NewTask,
  context: { readonly uid: string; readonly display: string }
): Record<string, unknown> => {
  const due = input.due ?? null;
  const recurring =
    input.recurrenceRule !== null &&
    input.recurrenceRule !== undefined &&
    due !== null;
  const alerts = alertsFor(input.alerts ?? [], due, recurring, context.display);
  const notes = input.notes?.trim() ?? "";
  const priority = PRIORITY_VALUES[input.priority ?? "none"];
  const links = input.links ?? [];
  const properties: Record<string, unknown> = {
    "@type": "Task",
    calendarIds: { [input.calendarId]: true },
    progress: "needs-action",
    title: input.title.trim(),
    uid: context.uid,
    ...(due === null ? {} : dueProperties(due)),
    ...(recurring
      ? { recurrenceRule: input.recurrenceRule, start: dueValue(due) }
      : {}),
    ...(notes === "" ? {} : { description: notes }),
    ...(priority === 0 ? {} : { priority }),
    ...(Object.keys(alerts).length === 0 ? {} : { alerts }),
    ...(input.parentUid === null || input.parentUid === undefined
      ? {}
      : { relatedTo: { [input.parentUid]: relation } }),
    ...(links.length === 0
      ? {}
      : {
          links: Object.fromEntries(
            links.map((link, index) => [
              `l${String(index + 1)}`,
              {
                "@type": "Link",
                href: link.href,
                rel: "describedby",
                ...(link.title === undefined ? {} : { title: link.title }),
              },
            ])
          ),
        }),
  };
  if (properties.timeZone === null) {
    Reflect.deleteProperty(properties, "timeZone");
  }
  return properties;
};

// ---------------------------------------------------------------- completing

/** Override keys that only say "completed" (safe to drop on undo). */
const COMPLETION_KEYS = new Set([
  "@type",
  "percentComplete",
  "progress",
  "progressUpdated",
  "updated",
]);

export interface CompleteOptions {
  readonly now: number;
}

/**
 * RFC 8984 §5.2.6 `progressUpdated`: when `progress` was last set, a
 * UTCDateTime (§1.4.4: "Z", no zero fraction). Written with every change
 * of `progress` the app makes.
 */
export const progressUpdatedAt = (now: number): string => utcDateTime(now);

const completedPatch = (
  series: RawTask,
  done: boolean,
  now: number
): Record<string, unknown> => ({
  progress: done ? "completed" : "needs-action",
  progressUpdated: progressUpdatedAt(now),
  ...(series.percentComplete === undefined || series.percentComplete === null
    ? {}
    : { percentComplete: done ? 100 : 0 }),
  ...icalPatch(
    series,
    withICalProperty(series, "completed", done ? completedProperty(now) : null)
  ),
});

/**
 * Completes (or reopens) a task, or one occurrence of a repeating task.
 * `series` is the stored task (`task.baseId`). An occurrence's
 * `progressUpdated` goes in its override, next to its `progress`, as RFC
 * 8984 §5.2.6 suggests (the other occurrences keep theirs). Reopening an
 * occurrence whose override only said "completed" drops the override,
 * timestamp and all: the occurrence is back to the series, which keeps
 * the list of overrides short (the concern §5.2.6 raises).
 */
export const completeChange = (
  task: Task,
  series: RawTask,
  done: boolean,
  options: CompleteOptions
): TaskChange => {
  const { accountId } = task;
  const id = task.baseId;
  const { recurrenceId } = task;
  if (recurrenceId === null) {
    return {
      accountId,
      update: { [id]: completedPatch(series, done, options.now) },
    };
  }
  const overrides = series.recurrenceOverrides;
  const current = overrides?.[recurrenceId];
  if (current !== undefined && !needsEscape(recurrenceId)) {
    const onlyCompletion = Object.keys(current).every((key) =>
      COMPLETION_KEYS.has(key)
    );
    return {
      accountId,
      update: {
        [id]:
          !done && onlyCompletion
            ? mapPatch("recurrenceOverrides", overrides, {
                [recurrenceId]: null,
              })
            : {
                [`recurrenceOverrides/${recurrenceId}/progress`]: done
                  ? "completed"
                  : "needs-action",
                [`recurrenceOverrides/${recurrenceId}/progressUpdated`]:
                  progressUpdatedAt(options.now),
                ...(current.start === undefined
                  ? {
                      [`recurrenceOverrides/${recurrenceId}/start`]:
                        recurrenceId,
                    }
                  : {}),
              },
      },
    };
  }
  return {
    accountId,
    update: {
      [id]: mapPatch("recurrenceOverrides", overrides, {
        [recurrenceId]: withStart(
          recurrenceId,
          done
            ? {
                progress: "completed",
                progressUpdated: progressUpdatedAt(options.now),
              }
            : {
                ...current,
                progress: "needs-action",
                progressUpdated: progressUpdatedAt(options.now),
              }
        ),
      }),
    },
  };
};

// ---------------------------------------------------------------- editing

export const titlePatch = (title: string): Patch => ({ title: title.trim() });

export const notesPatch = (notes: string): Patch => {
  const text = notes.trim();
  return text === ""
    ? { description: null, descriptionContentType: null }
    : { description: text, descriptionContentType: null };
};

export const priorityPatch = (priority: TaskPriority): Patch => ({
  priority: PRIORITY_VALUES[priority],
});

/** Longest estimate the editor writes: 999 h (a sanity cap, not a rule). */
export const MAX_ESTIMATE_MINUTES = 999 * 60;

/**
 * Sets the estimated time (RFC 8984 §5.2.3 `estimatedDuration`, "the
 * estimated positive duration of time the task takes to complete") as a
 * §1.4.6 Duration in hours and minutes ("PT1H30M"; ESTIMATED-DURATION in
 * iCalendar, RFC 10051 §10.1). Null, zero or not a number removes it.
 */
export const estimatePatch = (minutes: number | null): Patch => {
  const whole =
    minutes === null || !Number.isFinite(minutes)
      ? 0
      : Math.min(Math.round(minutes), MAX_ESTIMATE_MINUTES);
  return { estimatedDuration: whole > 0 ? formatDuration(0, whole) : null };
};

/** Moves a task to another list (a calendar of the same account). */
export const listPatch = (calendarId: CalendarId): Patch => ({
  calendarIds: { [calendarId]: true },
});

/** Makes a task a subtask of `parentUid` (null: back to the top). */
export const parentPatch = (
  series: RawTask,
  parentUid: string | null
): Record<string, unknown> => {
  const changes: Record<string, unknown> = {};
  for (const [uid, value] of Object.entries(series.relatedTo ?? {})) {
    if (value.relation?.parent === true && uid !== parentUid) {
      // Another parent: drop it (one parent at a time), keep other relations.
      const others = Object.entries(value.relation).filter(
        ([kind, on]) => kind !== "parent" && on
      );
      changes[uid] =
        others.length === 0
          ? null
          : {
              ...value,
              relation: Object.fromEntries(others),
            };
    }
  }
  if (
    parentUid !== null &&
    series.relatedTo?.[parentUid]?.relation?.parent !== true
  ) {
    changes[parentUid] = relation;
  }
  return mapPatch("relatedTo", series.relatedTo, changes);
};

/** The current alerts of a series, as inputs (to edit and write back). */
const currentInputs = (task: Task): AlertInput[] =>
  task.alerts.flatMap((alert): AlertInput[] => {
    if (alert.minutesBefore !== null) {
      return [{ key: alert.key, minutesBefore: alert.minutesBefore }];
    }
    return alert.at === null ? [] : [{ at: alert.at, key: alert.key }];
  });

/** A repeating task needs a start (its due) for its RRULE and alerts. */
const anchorPatch = (
  series: RawTask,
  due: TaskDue | null
): Record<string, unknown> => {
  const recurring =
    series.recurrenceRule !== null && series.recurrenceRule !== undefined;
  if (!recurring || due === null) {
    return {};
  }
  const value = dueValue(due);
  return series.start === value ? {} : { start: value };
};

const whenOf = (trigger: Record<string, unknown>): number | null =>
  typeof trigger.when === "string" ? Date.parse(trigger.when) : null;

/** Same moment for the same anchor (`@type` and defaults aside). */
const sameTrigger = (a: Record<string, unknown>, b: unknown): boolean => {
  if (!isRecord(b)) {
    return false;
  }
  return (
    whenOf(a) === whenOf(b) &&
    (a.offset ?? null) === (b.offset ?? null) &&
    (a.relativeTo ?? "start") === (b.relativeTo ?? "start")
  );
};

/**
 * Sets the alerts. Each alert's other properties (acknowledged, the
 * alarm's extras) stay: only `trigger` and `action` are written for keys
 * that are there.
 */
export const alertsPatch = (
  series: RawTask,
  inputs: readonly AlertInput[],
  due: TaskDue | null,
  display: string
): Record<string, unknown> => {
  const recurring =
    series.recurrenceRule !== null && series.recurrenceRule !== undefined;
  const wanted = alertsFor(inputs, due, recurring, display);
  const existing = series.alerts ?? {};
  const changes: Record<string, unknown> = {};
  for (const key of Object.keys(existing)) {
    if (!(key in wanted)) {
      changes[key] = null;
    }
  }
  for (const [key, alert] of Object.entries(wanted)) {
    const old = existing[key];
    if (old === undefined) {
      changes[key] = alert;
    } else if (!sameTrigger(old.trigger, alert.trigger)) {
      changes[key] = {
        ...old,
        action: old.action ?? "display",
        trigger: alert.trigger,
      };
    }
  }
  return {
    ...mapPatch("alerts", series.alerts, changes),
    ...anchorPatch(series, due),
  };
};

/**
 * The alerts tied to the due, for a new due. From a day to a time or back,
 * "1 day before at 9:00" ↔ "1 day before" (`convertReminder`).
 */
const tiedInputs = (
  task: Task,
  due: TaskDue | null,
  reminderDefault: number
): AlertInput[] => {
  const toKind =
    due !== null && task.due !== null && task.due.kind !== due.kind
      ? due.kind
      : null;
  return currentInputs(task).map((input) =>
    toKind !== null && "minutesBefore" in input
      ? {
          ...input,
          minutesBefore: convertReminder(
            input.minutesBefore,
            toKind,
            reminderDefault
          ),
        }
      : input
  );
};

/**
 * Changes the due (null: none). A repeating task's start moves with it
 * (it is the series' anchor), and so does a start that would be after the
 * due or of the other kind (day / time). Alerts tied to the due follow
 * it. Taking the due from a repeating task also ends the repetition.
 */
export const duePatch = (
  task: Task,
  series: RawTask,
  due: TaskDue | null,
  display: string,
  reminderDefault: number = DEFAULT_REMINDER_MINUTES
): Record<string, unknown> => {
  const recurring =
    series.recurrenceRule !== null && series.recurrenceRule !== undefined;
  const patch: Record<string, unknown> = { ...dueProperties(due) };
  if (due === null) {
    if (recurring) {
      Object.assign(patch, {
        recurrenceOverrides: null,
        recurrenceRule: null,
        start: null,
      });
    }
  } else if (typeof series.start === "string") {
    const before = dueOf(series);
    const start = parseLocalDateTime(series.start);
    const newDue = parseLocalDateTime(dueValue(due));
    const kindChanged = before !== null && before.kind !== due.kind;
    const startAfterDue =
      start !== null && newDue !== null && start.compare(newDue) > 0;
    if (recurring || kindChanged || startAfterDue) {
      patch.start = dueValue(due);
    }
  } else if (recurring) {
    patch.start = dueValue(due);
  }
  const tied = tiedInputs(task, due, reminderDefault);
  if (tied.length > 0) {
    Object.assign(patch, alertsPatch(series, tied, due, display));
  }
  // A day writes `timeZone: null`: leave it out when there is none.
  if (
    due?.kind !== "datetime" &&
    (series.timeZone === undefined || series.timeZone === null)
  ) {
    Reflect.deleteProperty(patch, "timeZone");
  }
  return patch;
};

/**
 * Sets or ends the repetition (`rule` a JSCalendar RecurrenceRule). A
 * repeating task needs a due; its start becomes the due and alerts tied to
 * the due become offsets (absolute ones would fire only once).
 */
export const recurrencePatch = (
  task: Task,
  series: RawTask,
  rule: RecurrenceRule | null,
  display: string
): Record<string, unknown> | null => {
  if (rule === null) {
    const patch: Record<string, unknown> = {
      recurrenceOverrides: null,
      recurrenceRule: null,
    };
    const tied = currentInputs(task);
    if (tied.length > 0) {
      Object.assign(
        patch,
        alertsPatch(
          { ...series, recurrenceRule: null },
          tied,
          task.due,
          display
        )
      );
    }
    return patch;
  }
  const due = dueOf(series);
  if (due === null) {
    return null;
  }
  const repeating = { ...series, recurrenceRule: rule };
  return {
    recurrenceRule: rule,
    ...anchorPatch(repeating, due),
    ...alertsPatch(repeating, currentInputs(task), due, display),
  };
};
