// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JSCalendar tasks (RFC 8984 §5.2, "bis" shape) as they travel today:
// inside calendars, through `CalendarEvent/*` with `@type: "Task"` (VTODO
// in CalDAV). There is no JMAP Tasks standard (draft-ietf-jmap-tasks-06
// expired) and no server offers one; `JMAP_TASKS_CAPABILITY` is reserved
// for an adapter if one appears.
//
// Checked against Stalwart (live test in
// apps/web/src/tasks/stalwart.live.test.ts):
//
// - The completion date has no JSCalendar property: it is the iCalendar
//   COMPLETED property in `iCalendar.properties`. Stalwart writes the value
//   to CalDAV as it is, so it must be in iCalendar form ("20261027T100000Z",
//   `formatICalUtc`); it reads it back with the type "unknown".
// - `utcStart`/`utcEnd` of a task without dates is a sentinel far in the
//   future (year 32548): `taskUtc` reads it as null.
// - `iCalendar` only comes back when asked for by name.

import { z } from "zod";

import {
  AlertSchema,
  ICalComponentSchema,
  ICalPropertySchema,
  LinkSchema,
  LocationSchema,
  ParticipantSchema,
  RecurrenceRuleSchema,
  RelationSchema,
  taskOnlyShape,
} from "./calendars";
import type { ICalComponent, ICalProperty, Relation } from "./calendars";
import { CAPABILITIES } from "./capabilities";
import { CalendarEventId } from "./ids";

/**
 * Reserved for a future JMAP Tasks server (draft-ietf-jmap-tasks: `Task/*`
 * and `TaskList/*`). Nothing uses it yet: tasks go through calendars.
 */
export const JMAP_TASKS_CAPABILITY = "urn:ietf:params:jmap:tasks";

/** What task methods need today (calendars) and later (tasks). */
export const TASK_CAPABILITIES = {
  calendars: CAPABILITIES.calendars,
  tasks: JMAP_TASKS_CAPABILITY,
} as const;

/** The `@type` of a JSCalendar task. */
export const TASK_TYPE = "Task";

/** RFC 8984 §5.2.5 `progress` values (VTODO STATUS). */
export const TASK_PROGRESS = [
  "needs-action",
  "in-process",
  "completed",
  "failed",
  "cancelled",
] as const;
export type TaskProgressValue = (typeof TASK_PROGRESS)[number];

const flags = z.record(z.string(), z.boolean());

/** The JSCalendar Task properties (RFC 8984 §5.2 and the common ones). */
const taskShape = {
  ...taskOnlyShape,
  "@type": z.string().nullish(),
  alerts: z.record(z.string(), AlertSchema).nullish(),
  categories: flags.nullish(),
  color: z.string().nullish(),
  created: z.string().nullish(),
  description: z.string().nullish(),
  descriptionContentType: z.string().nullish(),
  excludedRecurrenceRules: z.array(RecurrenceRuleSchema).nullish(),
  iCalendar: ICalComponentSchema.nullish().catch(null),
  keywords: flags.nullish(),
  links: z.record(z.string(), LinkSchema).nullish(),
  locale: z.string().nullish(),
  locations: z.record(z.string(), LocationSchema).nullish(),
  /** `mailto:` of who assigned the task (with `participants`). */
  organizerCalendarAddress: z.string().nullish(),
  participants: z.record(z.string(), ParticipantSchema).nullish(),
  privacy: z.string().nullish(),
  recurrenceId: z.string().nullish(),
  recurrenceIdTimeZone: z.string().nullish(),
  recurrenceOverrides: z
    .record(z.string(), z.record(z.string(), z.unknown()))
    .nullish(),
  recurrenceRule: RecurrenceRuleSchema.nullish(),
  relatedTo: z.record(z.string(), RelationSchema).nullish().catch(null),
  sequence: z.number().nullish(),
  showWithoutTime: z.boolean().nullish(),
  /** LocalDateTime in `timeZone`. */
  start: z.string().nullish(),
  timeZone: z.string().nullish(),
  title: z.string().nullish(),
  uid: z.string().nullish(),
  updated: z.string().nullish(),
  useDefaultAlerts: z.boolean().nullish(),
};

/** A JSCalendar Task (also a parsed one: no JMAP-level properties). */
export const TaskSchema = z.looseObject(taskShape);
export type Task = z.infer<typeof TaskSchema>;

/** A task stored in a calendar, as `CalendarEvent/get` returns it. */
export const StoredTaskSchema = z.looseObject({
  ...taskShape,
  baseEventId: CalendarEventId.nullish(),
  calendarIds: flags.nullish(),
  id: CalendarEventId,
  isDraft: z.boolean().nullish(),
  isOrigin: z.boolean().nullish(),
  utcEnd: z.string().nullish(),
  utcStart: z.string().nullish(),
});
export type StoredTask = z.infer<typeof StoredTaskSchema>;

/** Whether a calendar object is a task. */
export const isTask = (object: {
  readonly "@type"?: string | null | undefined;
}): boolean => object["@type"] === TASK_TYPE;

const COMMON_TASK_PROPERTIES = [
  "@type",
  "alerts",
  "baseEventId",
  "calendarIds",
  "created",
  "description",
  "descriptionContentType",
  "due",
  "estimatedDuration",
  "iCalendar",
  "keywords",
  "links",
  "organizerCalendarAddress",
  "participants",
  "percentComplete",
  "priority",
  "progress",
  // Stalwart 1.0.1 leaves it out when asked by name (editing.ts,
  // `inversePatch`); the bridge returns it.
  "progressUpdated",
  "recurrenceId",
  "recurrenceRule",
  "relatedTo",
  "sequence",
  "showWithoutTime",
  "start",
  "timeZone",
  "title",
  "uid",
  "updated",
] as const;

/**
 * The properties worth asking `CalendarEvent/get` for, for stored tasks
 * (one-off tasks and repeating series). Stalwart 1.0 refuses
 * `recurrenceOverrides` together with `utcStart`/`utcEnd`
 * (`invalidArguments`), so the series list asks for the overrides and the
 * occurrence list for the UTC times, never both.
 */
export const TASK_PROPERTIES = [
  ...COMMON_TASK_PROPERTIES,
  "recurrenceOverrides",
] as const;

/**
 * The properties for occurrences of a repeating task (the ids that
 * `CalendarEvent/query` with `expandRecurrences` gives): their `utcStart`
 * places the occurrence, and the overrides live on the series.
 */
export const TASK_OCCURRENCE_PROPERTIES = [
  ...COMMON_TASK_PROPERTIES,
  "utcEnd",
  "utcStart",
] as const;

// ---------------------------------------------------------------- iCalendar

interface WithICal {
  readonly iCalendar?: ICalComponent | null | undefined;
}

/** The well-formed extra iCalendar properties of a task (empty if none). */
export const icalProperties = (task: WithICal): ICalProperty[] =>
  (task.iCalendar?.properties ?? []).flatMap((entry) => {
    const property = ICalPropertySchema.safeParse(entry);
    return property.success ? [property.data] : [];
  });

/** The first value of an iCalendar property (name in any case), or null. */
export const icalValue = (task: WithICal, name: string): unknown => {
  const lower = name.toLowerCase();
  const found = icalProperties(task).find(
    (property) => property[0].toLowerCase() === lower
  );
  return found === undefined ? null : found[3];
};

const nameOf = (entry: unknown): string | null =>
  Array.isArray(entry) && typeof entry[0] === "string"
    ? entry[0].toLowerCase()
    : null;

/**
 * The task's `iCalendar.properties` with every `name` property replaced by
 * `property` (or removed when null). Everything else stays as it was, in
 * order, odd entries included.
 */
export const withICalProperty = (
  task: WithICal,
  name: string,
  property: ICalProperty | null
): unknown[] => {
  const lower = name.toLowerCase();
  const others = (task.iCalendar?.properties ?? []).filter(
    (entry) => nameOf(entry) !== lower
  );
  return property === null ? others : [...others, property];
};

const pad = (value: number, size = 2): string =>
  String(value).padStart(size, "0");

/** "20261027T100000Z": an iCalendar UTC DATE-TIME (RFC 5545 §3.3.5). */
export const formatICalUtc = (ms: number): string => {
  const date = new Date(ms);
  return [
    pad(date.getUTCFullYear(), 4),
    pad(date.getUTCMonth() + 1),
    pad(date.getUTCDate()),
    "T",
    pad(date.getUTCHours()),
    pad(date.getUTCMinutes()),
    pad(date.getUTCSeconds()),
    "Z",
  ].join("");
};

const ICAL_UTC =
  /^(?<y>\d{4})-?(?<mo>\d{2})-?(?<d>\d{2})(?:T(?<h>\d{2}):?(?<mi>\d{2}):?(?<s>\d{2})(?:\.\d+)?Z?)?$/u;

/**
 * A UTC date-time in iCalendar ("20261027T100000Z") or JSON form
 * ("2026-10-27T10:00:00Z"), or a date ("20261027": its midnight UTC), to
 * epoch ms; null if it isn't one.
 */
export const parseICalUtc = (value: unknown): number | null => {
  if (typeof value !== "string") {
    return null;
  }
  const groups = ICAL_UTC.exec(value.trim())?.groups;
  if (groups === undefined) {
    return null;
  }
  const month = Number(groups.mo);
  const day = Number(groups.d);
  if (month < 1 || month > 12 || day < 1 || day > 31) {
    return null;
  }
  const ms = Date.UTC(
    Number(groups.y),
    month - 1,
    day,
    Number(groups.h ?? "0"),
    Number(groups.mi ?? "0"),
    Number(groups.s ?? "0")
  );
  return Number.isFinite(ms) ? ms : null;
};

/** The COMPLETED property of a task, as epoch ms (null: none or invalid). */
export const completedAt = (task: WithICal): number | null =>
  parseICalUtc(icalValue(task, "completed"));

/** The COMPLETED property for `ms`, in the form Stalwart writes as is. */
export const completedProperty = (ms: number): ICalProperty => [
  "completed",
  {},
  "date-time",
  formatICalUtc(ms),
];

// ---------------------------------------------------------------- the rest

/** UIDs of the task's parents (`relatedTo` with the `parent` relation). */
export const parentUids = (task: {
  readonly relatedTo?: Readonly<Record<string, Relation>> | null | undefined;
}): string[] =>
  Object.entries(task.relatedTo ?? {})
    .filter(([, relation]) => relation.relation?.parent === true)
    .map(([uid]) => uid);

/** The latest year a real UTCDateTime can have (RFC 8984 §1.4.4). */
const MAX_YEAR = 9999;

/**
 * `utcStart`/`utcEnd` as epoch ms, or null for none, an invalid value, or
 * Stalwart's sentinel for a task without dates (a year past 9999).
 */
export const taskUtc = (value: string | null | undefined): number | null => {
  if (value === null || value === undefined) {
    return null;
  }
  const year = Number(/^(?<year>\d+)-/u.exec(value)?.groups?.year);
  if (!Number.isFinite(year) || year > MAX_YEAR) {
    return null;
  }
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
};
