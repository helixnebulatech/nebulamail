// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A calendar's time zone and default alerts (draft-ietf-jmap-calendars
// §4.1): reading them, the `Calendar/set` patch of the
// settings dialog, and what a new event starts with. Pure, with tests.
//
// New events get the defaults written out as their own alerts (never
// `useDefaultAlerts`): every client and server fires explicit alerts,
// while `useDefaultAlerts` means different things (the bridge replaces
// `alerts` with the defaults, Stalwart adds them).

import type { CalendarDateTime } from "@internationalized/date";
import {
  toCalendarDateTime,
  toTimeZone,
  toZoned,
} from "@internationalized/date";
import type { Alert } from "@nebulamail/jmap-client";
import { AlertSchema } from "@nebulamail/jmap-client";

import type { EventForm } from "./editing";
import type { CalendarSummary, DefaultAlerts } from "./model";
import {
  durationMinutes,
  formatOffset,
  isValidTimeZone,
  parseDuration,
} from "./time";

/**
 * Reminders of a new event when its calendar says nothing (a server
 * without default alerts): 10 minutes before a timed event, none for an
 * all-day one, as before calendars had their own.
 */
export const FALLBACK_WITH_TIME: readonly number[] = [10];
export const FALLBACK_WITHOUT_TIME: readonly number[] = [];

const isShownAlert = (alert: Alert): boolean =>
  (alert.action === undefined ||
    alert.action === null ||
    alert.action === "display") &&
  (alert.trigger["@type"] === undefined ||
    alert.trigger["@type"] === "OffsetTrigger") &&
  (alert.trigger.relativeTo === undefined ||
    alert.trigger.relativeTo === "start");

/**
 * `defaultAlertsWithTime`/`WithoutTime` as the server sends them; null
 * when it isn't a map (the server doesn't have them). Display alerts
 * relative to the start become minutes; the rest is kept aside, as it is.
 */
export const readDefaultAlerts = (value: unknown): DefaultAlerts | null => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const minutes: number[] = [];
  const others: Record<string, Alert> = {};
  for (const [id, raw] of Object.entries(value)) {
    const alert = AlertSchema.safeParse(raw);
    if (!alert.success) {
      continue;
    }
    const duration = isShownAlert(alert.data)
      ? parseDuration(alert.data.trigger.offset)
      : null;
    if (duration === null) {
      others[id] = alert.data;
    } else {
      minutes.push(-durationMinutes(duration));
    }
  }
  return { minutes: [...new Set(minutes)], others };
};

/**
 * A fresh id for a default alert. Stalwart 1.0 wants them unique across
 * both maps of every calendar of the account (`invalidProperties`
 * otherwise; checked live 2026-10-30), so they are random, not "a1".
 */
export const newAlertId = (): string => {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return `a${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
};

/**
 * The whole map to store: the kept alerts plus one per minutes value,
 * each under a new id (`newAlertId`; the kept ones too, in case an id
 * another client chose, like "a1", clashes with another calendar's).
 */
export const defaultAlertsMap = (
  minutes: readonly number[],
  kept: Readonly<Record<string, Alert>> = {},
  newId: () => string = newAlertId
): Record<string, Alert> => {
  const map: Record<string, Alert> = {};
  for (const alert of Object.values(kept)) {
    map[newId()] = alert;
  }
  for (const before of new Set(minutes)) {
    map[newId()] = {
      "@type": "Alert",
      action: "display",
      trigger: {
        "@type": "OffsetTrigger",
        offset: formatOffset(-before),
        relativeTo: "start",
      },
    };
  }
  return map;
};

/** What the settings dialog edits. */
export interface CalendarSettings {
  /** IANA zone; null: none (each device uses its own). */
  readonly timeZone: string | null;
  readonly withTime: readonly number[];
  readonly withoutTime: readonly number[];
}

/**
 * The dialog's starting values: the calendar's, or for a new calendar
 * no zone and the app's usual reminders.
 */
export const settingsOf = (
  calendar: CalendarSummary | null
): CalendarSettings =>
  calendar === null
    ? {
        timeZone: null,
        withTime: FALLBACK_WITH_TIME,
        withoutTime: FALLBACK_WITHOUT_TIME,
      }
    : {
        timeZone: calendar.timeZone ?? null,
        withTime: calendar.defaultAlertsWithTime?.minutes ?? [],
        withoutTime: calendar.defaultAlertsWithoutTime?.minutes ?? [],
      };

const sameList = (a: readonly number[], b: readonly number[]): boolean =>
  a.length === b.length && a.every((value, index) => value === b[index]);

export interface SettingsPatch {
  timeZone?: string | null;
  defaultAlertsWithTime?: Record<string, Alert>;
  defaultAlertsWithoutTime?: Record<string, Alert>;
}

/**
 * The settings of a calendar we create. Stalwart 1.0 rings the alerts of
 * all-day events and tasks from midnight in the calendar's zone (UTC if it
 * has none), so one is always given, unless the user chose none (null).
 */
export const newCalendarSettings = (
  settings: SettingsPatch | undefined,
  zone: string
): SettingsPatch => {
  const { timeZone = zone, ...rest } = settings ?? {};
  return timeZone === null ? rest : { ...rest, timeZone };
};

/**
 * The `Calendar/set` properties that changed (for a new calendar, what
 * isn't empty). Maps go whole, keeping the alerts the app can't show
 * (`{}` clears). When either map of a stored calendar changes, both go,
 * with new ids (`defaultAlertsMap`): Stalwart checks every id of the calendar
 * against the other calendars, so an old clashing id in the untouched map
 * would refuse the save. A zone that isn't valid here isn't sent.
 */
export const settingsPatch = (
  calendar: CalendarSummary | null,
  before: CalendarSettings,
  after: CalendarSettings,
  newId: () => string = newAlertId
): SettingsPatch => {
  const patch: SettingsPatch = {};
  if (
    after.timeZone !== before.timeZone &&
    (after.timeZone === null || isValidTimeZone(after.timeZone))
  ) {
    patch.timeZone = after.timeZone;
  }
  if (calendar === null) {
    // A new calendar starts without any: only non-empty lists are sent.
    if (after.withTime.length > 0) {
      patch.defaultAlertsWithTime = defaultAlertsMap(after.withTime, {}, newId);
    }
    if (after.withoutTime.length > 0) {
      patch.defaultAlertsWithoutTime = defaultAlertsMap(
        after.withoutTime,
        {},
        newId
      );
    }
    return patch;
  }
  if (
    !sameList(before.withTime, after.withTime) ||
    !sameList(before.withoutTime, after.withoutTime)
  ) {
    patch.defaultAlertsWithTime = defaultAlertsMap(
      after.withTime,
      calendar.defaultAlertsWithTime?.others,
      newId
    );
    patch.defaultAlertsWithoutTime = defaultAlertsMap(
      after.withoutTime,
      calendar.defaultAlertsWithoutTime?.others,
      newId
    );
  }
  return patch;
};

/**
 * Reminders of a new event in `calendar`: its defaults (with or without
 * time), or the fallback when it has none to say.
 */
export const prefillAlerts = (
  calendar: CalendarSummary | null | undefined,
  allDay: boolean
): number[] => {
  const defaults = allDay
    ? calendar?.defaultAlertsWithoutTime
    : calendar?.defaultAlertsWithTime;
  if (defaults === null || defaults === undefined) {
    return [...(allDay ? FALLBACK_WITHOUT_TIME : FALLBACK_WITH_TIME)];
  }
  return [...defaults.minutes];
};

/** The calendar a form points at (own ones when `accountId` is null). */
export const calendarOfForm = (
  calendars: readonly CalendarSummary[],
  form: Pick<EventForm, "accountId" | "calendarId">
): CalendarSummary | undefined =>
  calendars.find(
    (calendar) =>
      calendar.id === form.calendarId &&
      (form.accountId === null
        ? calendar.owner === null
        : calendar.accountId === form.accountId)
  );

/**
 * A change to a new event's form. While the user hasn't touched the
 * reminders, choosing another calendar or (un)ticking "all day" brings
 * that calendar's defaults.
 */
export const changedForm = (
  current: EventForm,
  change: Partial<EventForm>,
  calendars: readonly CalendarSummary[],
  prefill: boolean
): EventForm => {
  const next = { ...current, ...change };
  const moved =
    next.calendarId !== current.calendarId ||
    next.accountId !== current.accountId ||
    next.allDay !== current.allDay;
  if (!prefill || !moved || change.alerts !== undefined) {
    return next;
  }
  return {
    ...next,
    alerts: prefillAlerts(calendarOfForm(calendars, next), next.allDay),
  };
};

/**
 * Where a new timed event starts and in which zone: in the calendar's
 * zone when it has one, at the same instant the user picked in their own
 * zone (the event lands where they clicked). All-day events, and
 * calendars without a (valid) zone, keep the user's zone.
 */
export const newEventStart = (
  at: CalendarDateTime,
  userZone: string,
  calendarZone: string | null | undefined,
  allDay: boolean
): { readonly at: CalendarDateTime; readonly zone: string } => {
  if (
    allDay ||
    calendarZone === null ||
    calendarZone === undefined ||
    calendarZone === userZone ||
    !isValidTimeZone(calendarZone)
  ) {
    return { at, zone: userZone };
  }
  return {
    at: toCalendarDateTime(toTimeZone(toZoned(at, userZone), calendarZone)),
    zone: calendarZone,
  };
};
