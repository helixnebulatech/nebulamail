// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Event reminders (JSCalendar alerts, RFC 8984 §4.5). The
// server decides when a display alert is due and says so with a
// CalendarAlert push object: over the event source while a tab is open,
// over Web Push (lib/web-push.ts, the service worker) when none is.
// Checked with Stalwart 1.0 (2026-10-26): it sends `calendarAlert` events
// on the event source for `types=CalendarAlert`, skips an alert whose
// `acknowledged` is at or after its time, and fires an AbsoluteTrigger
// alert added later (how snoozing works).
//
// Dismiss: `acknowledged` = now on the alert (for a series, on the series:
// earlier occurrences count as seen, later ones still ring). Snooze: the
// same, plus a new alert at now + N minutes related to the first one
// (RFC 8984 §4.5.2, `relatedTo` with `parent`); dismissing a snooze alert
// removes it.

import { toZoned } from "@internationalized/date";
import type {
  AccountId,
  CalendarAlert,
  CalendarEventId,
} from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";

import { getClient } from "../mail/client";
import { dayReminderInstant } from "../tasks/day-reminders";
import { updateEvent } from "./api";
import { SNOOZE_PREFIX } from "./model";
import type { EventTiming } from "./time";
import {
  dateInZone,
  durationMinutes,
  eventZone,
  parseLocalDate,
  parseDuration,
  parseLocalDateTime,
  startOfDayMs,
  timedRange,
} from "./time";

export { SNOOZE_PREFIX } from "./model";
/** The snooze choices, in minutes. */
export const SNOOZE_MINUTES = [5, 10, 30] as const;

/** localStorage flag (useLocalFlag) for the per-device setting. */
export const ALERTS_KEY = "wm-event-reminders";

/** An alert ready to show. Text is untrusted (cleaned). */
export interface DueAlert {
  /** Same alert → same key (notification tag, toast). */
  readonly key: string;
  readonly accountId: AccountId;
  readonly eventId: CalendarEventId;
  readonly alertId: string;
  readonly recurrenceId: string | null;
  readonly title: string;
  /** Epoch ms of the occurrence; null if unknown. */
  readonly start: number | null;
  readonly end: number | null;
  readonly allDay: boolean;
  /** An event's reminder, or a task's (`start` is its due; no `end`). */
  readonly kind: "event" | "task";
  /** The alert exists in the event (not a server default): can be dismissed. */
  readonly stored: boolean;
  /**
   * When the reminder was set to ring (epoch ms; a task's), shown for a
   * task due on a day ("reminder at 9:00"); null if unknown.
   */
  readonly remindAt?: number | null;
}

export const alertKey = (alert: CalendarAlert): string =>
  [
    "wm-alert",
    alert.accountId,
    alert.calendarEventId,
    alert.alertId,
    alert.recurrenceId ?? "",
  ].join("|");

/** "2026-10-26T20:49:00Z" from epoch ms (UTCDateTime, no milliseconds). */
export const utcDateTime = (ms: number): string =>
  new Date(ms).toISOString().replace(/\.\d{3}Z$/u, "Z");

/** Watches CalendarAlert pushes while a tab is open. */
export const watchAlerts = async (
  onAlert: (alert: CalendarAlert) => void,
  signal: AbortSignal
): Promise<void> => {
  const client = await getClient();
  const changes = client.push({
    onCalendarAlert: onAlert,
    signal,
    types: ["CalendarAlert"],
  });
  // Nothing to do with state changes here: they aren't asked for.
  for await (const _change of changes) {
    // Keeps the connection open.
  }
};

const MAX_TITLE = 200;

/** A UTC time or one with an offset ("…Z", "…+02:00"). */
const WITH_OFFSET = /(?:z|[+-]\d{2}:?\d{2})$/iu;

/**
 * When an occurrence starts (epoch ms), from the alert's `recurrenceId`.
 * Stalwart (checked live) and the bridge send it
 * as a LocalDateTime in the event's own time zone ("2026-10-27T17:11:44"
 * in Madrid), not as UTC; floating and all-day events are in the user's
 * zone (`display`). A UTC time with `Z` (or an offset) is still read as
 * such. Null if there is none or it can't be read.
 */
export const occurrenceStart = (
  recurrenceId: string | null,
  event: EventTiming,
  display: string
): number | null => {
  if (recurrenceId === null || recurrenceId === "") {
    return null;
  }
  if (WITH_OFFSET.test(recurrenceId)) {
    const ms = Date.parse(recurrenceId);
    return Number.isFinite(ms) ? ms : null;
  }
  const zone =
    event.showWithoutTime === true ? display : eventZone(event, display);
  const local = parseLocalDateTime(recurrenceId);
  if (local !== null) {
    return toZoned(local, zone).toDate().getTime();
  }
  // A date alone (an all-day occurrence): its midnight.
  const date = recurrenceId.length === 10 ? parseLocalDate(recurrenceId) : null;
  return date === null ? null : startOfDayMs(date, zone);
};

/** What `alertOf` reads of the event or task an alert belongs to. */
export const ALERT_PROPERTIES = [
  "@type",
  "title",
  "start",
  "duration",
  "due",
  "progress",
  "timeZone",
  "showWithoutTime",
  "alerts",
  "recurrenceOverrides",
] as const;

/** The stored object, as far as `alertOf` needs it. */
export interface AlertSubject {
  readonly "@type"?: string | null | undefined;
  readonly title?: string | null | undefined;
  readonly start?: string | null | undefined;
  readonly duration?: string | null | undefined;
  readonly due?: string | null | undefined;
  readonly progress?: string | null | undefined;
  readonly timeZone?: string | null | undefined;
  readonly showWithoutTime?: boolean | null | undefined;
  readonly alerts?: Readonly<Record<string, unknown>> | null | undefined;
  readonly recurrenceOverrides?:
    | Readonly<Record<string, Readonly<Record<string, unknown>>>>
    | null
    | undefined;
}

/** Epoch ms of a LocalDateTime of `subject` (its zone; all-day: `display`). */
const localMs = (
  value: string | null | undefined,
  subject: AlertSubject,
  display: string
): number | null => {
  if (subject.showWithoutTime === true) {
    const date = parseLocalDate(value);
    return date === null ? null : startOfDayMs(date, display);
  }
  const local = parseLocalDateTime(value);
  return local === null
    ? null
    : toZoned(local, eventZone({ timeZone: subject.timeZone ?? null }, display))
        .toDate()
        .getTime();
};

/** Minutes of a SignedDuration ("-PT15H" → -900); null if it isn't one. */
const offsetMinutes = (value: unknown): number | null => {
  const duration = typeof value === "string" ? parseDuration(value) : null;
  return duration === null ? null : durationMinutes(duration);
};

/**
 * When a task's stored alert rings: its absolute time, or the offset from
 * the due `dueAt` (a repeating task's start is its due). A day task's
 * offset is on the wall clock of `display` ("PT9H": 9:00 that day).
 */
const reminderTime = (
  stored: unknown,
  dueAt: number | null,
  allDay: boolean,
  display: string
): number | null => {
  const trigger: unknown =
    typeof stored === "object" && stored !== null
      ? Reflect.get(stored, "trigger")
      : null;
  if (typeof trigger !== "object" || trigger === null) {
    return null;
  }
  const when: unknown = Reflect.get(trigger, "when");
  if (typeof when === "string") {
    const ms = Date.parse(when);
    return Number.isFinite(ms) ? ms : null;
  }
  const offset = offsetMinutes(Reflect.get(trigger, "offset"));
  if (offset === null || dueAt === null) {
    return null;
  }
  return allDay
    ? dayReminderInstant(
        dateInZone(dueAt, display).toString(),
        -offset,
        display
      )
    : dueAt + offset * 60_000;
};

/**
 * A task's alert: about its due, not a start. An
 * occurrence's due is its start (`recurrenceId`) plus the series' gap
 * from start to due (a repeating task has start = due). Null when that
 * task or occurrence is already done.
 */
const taskAlertOf = (
  alert: CalendarAlert,
  task: AlertSubject,
  display: string
): DueAlert | null => {
  const recurrenceId = alert.recurrenceId ?? null;
  const override =
    recurrenceId === null
      ? undefined
      : task.recurrenceOverrides?.[recurrenceId];
  const progress: unknown = override?.progress ?? task.progress;
  if (typeof progress === "string" && progress.toLowerCase() === "completed") {
    return null;
  }
  const due = localMs(task.due, task, display);
  const start = localMs(task.start, task, display);
  const occurrence = occurrenceStart(
    recurrenceId,
    {
      showWithoutTime: task.showWithoutTime === true,
      timeZone: task.timeZone ?? null,
    },
    display
  );
  const at =
    occurrence === null
      ? due
      : occurrence + (due === null || start === null ? 0 : due - start);
  return {
    remindAt: reminderTime(
      task.alerts?.[alert.alertId],
      at,
      task.showWithoutTime === true,
      display
    ),
    accountId: alert.accountId,
    alertId: alert.alertId,
    allDay: task.showWithoutTime === true,
    // A task's reminder stays worth showing until it is done.
    end: null,
    eventId: alert.calendarEventId,
    key: alertKey(alert),
    kind: "task",
    recurrenceId,
    start: at,
    stored: Object.hasOwn(task.alerts ?? {}, alert.alertId),
    title: cleanDisplayText(task.title ?? "").slice(0, MAX_TITLE),
  };
};

/** What an alert is about, from its stored event or task. */
export const alertOf = (
  alert: CalendarAlert,
  event: AlertSubject,
  zone: string
): DueAlert | null => {
  if (event["@type"] === "Task") {
    return taskAlertOf(alert, event, zone);
  }
  const timing = {
    duration: event.duration,
    start: event.start,
    showWithoutTime: event.showWithoutTime === true,
    timeZone: event.timeZone ?? null,
  };
  const occurrence = occurrenceStart(alert.recurrenceId ?? null, timing, zone);
  const range = timedRange(timing, zone);
  const length = range === null ? 0 : range.end - range.start;
  const start = occurrence ?? range?.start ?? null;
  return {
    accountId: alert.accountId,
    alertId: alert.alertId,
    allDay: event.showWithoutTime === true,
    end: start === null ? null : start + length,
    eventId: alert.calendarEventId,
    key: alertKey(alert),
    kind: "event",
    recurrenceId: alert.recurrenceId ?? null,
    start,
    stored: Object.hasOwn(event.alerts ?? {}, alert.alertId),
    title: cleanDisplayText(event.title ?? "").slice(0, MAX_TITLE),
  };
};

/** Reads what an alert is about. Null if the event is gone (or done). */
export const loadAlert = async (
  alert: CalendarAlert,
  zone: string
): Promise<DueAlert | null> => {
  const client = await getClient();
  const [result] = await client.request((b) => [
    b.call("CalendarEvent/get", {
      accountId: alert.accountId,
      ids: [alert.calendarEventId],
      properties: [...ALERT_PROPERTIES],
    }),
  ]);
  const [event] = result.list;
  return event === undefined ? null : alertOf(alert, event, zone);
};

/** The patch that dismisses an alert at `now`. */
export const dismissPatch = (
  alertId: string,
  now: number
): Record<string, unknown> =>
  alertId.startsWith(SNOOZE_PREFIX)
    ? { [`alerts/${alertId}`]: null }
    : { [`alerts/${alertId}/acknowledged`]: utcDateTime(now) };

/**
 * The patch that snoozes an alert for `minutes`: the alert is seen, and a
 * new one rings later. Snoozing a snooze replaces it (one at a time).
 */
export const snoozePatch = (
  alertId: string,
  parentId: string,
  minutes: number,
  now: number,
  newKey: string
): Record<string, unknown> => ({
  ...dismissPatch(alertId, now),
  [`alerts/${newKey}`]: {
    "@type": "Alert",
    action: "display",
    relatedTo: {
      [parentId]: { "@type": "Relation", relation: { parent: true } },
    },
    trigger: {
      "@type": "AbsoluteTrigger",
      when: utcDateTime(now + minutes * 60_000),
    },
  },
});

/** The alert a snooze alert hangs from (itself for any other). */
const parentOf = async (
  alert: Pick<DueAlert, "accountId" | "eventId" | "alertId">
): Promise<string> => {
  if (!alert.alertId.startsWith(SNOOZE_PREFIX)) {
    return alert.alertId;
  }
  const client = await getClient();
  const [result] = await client.request((b) => [
    b.call("CalendarEvent/get", {
      accountId: alert.accountId,
      ids: [alert.eventId],
      properties: ["alerts"],
    }),
  ]);
  const related: unknown = Reflect.get(
    result.list[0]?.alerts?.[alert.alertId] ?? {},
    "relatedTo"
  );
  const parent =
    typeof related === "object" && related !== null
      ? Object.keys(related)[0]
      : undefined;
  return parent ?? alert.alertId;
};

/** Marks the alert as seen on the server (every device stops ringing). */
export const dismissAlert = async (alert: DueAlert): Promise<void> => {
  if (!alert.stored) {
    return;
  }
  await updateEvent(
    alert.eventId,
    dismissPatch(alert.alertId, Date.now()),
    false,
    alert.accountId
  );
};

/** Rings again in `minutes` (on every device with reminders on). */
export const snoozeAlert = async (
  alert: DueAlert,
  minutes: number
): Promise<void> => {
  if (!alert.stored) {
    return;
  }
  const parent = await parentOf(alert);
  const key = `${SNOOZE_PREFIX}${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
  await updateEvent(
    alert.eventId,
    snoozePatch(alert.alertId, parent, minutes, Date.now(), key),
    false,
    alert.accountId
  );
};

/**
 * Whether a due alert is still worth showing: not for an event that has
 * already ended (a late push after the laptop wakes up).
 */
export const stillRelevant = (alert: DueAlert, now: number): boolean =>
  alert.end === null || alert.end > now;
