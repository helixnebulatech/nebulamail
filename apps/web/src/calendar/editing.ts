// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The event editor's form ↔ JSCalendar properties. Pure, with tests. What
// the form doesn't show (other properties, participant details, HTML
// descriptions that weren't edited) is kept as it is.

import type { CalendarDate, CalendarDateTime } from "@internationalized/date";
import { fromAbsolute, toZoned } from "@internationalized/date";
import type {
  AccountId,
  Alert,
  CalendarId,
  Location,
  Participant,
} from "@nebulamail/jmap-client";

import { addressOf } from "./itip";
import { SNOOZE_PREFIX } from "./model";
import type { EventDetail, Occurrence, ParticipationStatus } from "./model";
import type { Recurrence } from "./recurrence";
import { NO_RECURRENCE, toRule } from "./recurrence";
import {
  MINUTE_MS,
  durationMinutes,
  formatDuration,
  formatLocalDateTime,
  formatOffset,
  parseDuration,
  parseLocalDate,
  parseLocalDateTime,
  startOfDayMs,
} from "./time";

export interface GuestInput {
  readonly email: string;
  readonly name: string | null;
  readonly optional: boolean;
}

/** What the editor form holds. Dates "YYYY-MM-DD", times "HH:MM". */
export interface EventForm {
  readonly calendarId: CalendarId | null;
  /** The calendar's account (null: the user's own). */
  readonly accountId: AccountId | null;
  readonly title: string;
  readonly allDay: boolean;
  readonly startDate: string;
  readonly startTime: string;
  /** All-day: the last day (inclusive). */
  readonly endDate: string;
  readonly endTime: string;
  /** IANA zone of the times; null keeps a floating event floating. */
  readonly timeZone: string | null;
  readonly location: string;
  readonly description: string;
  /** The stored description is HTML and the user hasn't edited it. */
  readonly keepHtmlDescription: boolean;
  readonly recurrence: Recurrence;
  /** Minutes before the start. */
  readonly alerts: readonly number[];
  readonly guests: readonly GuestInput[];
}

export type FormProblem =
  | "no-calendar"
  | "invalid-start"
  | "invalid-end"
  | "end-before-start"
  | "invalid-guest";

const pad = (value: number): string => String(value).padStart(2, "0");

const dateTimeOf = (date: string, time: string): CalendarDateTime | null =>
  parseLocalDateTime(`${date}T${time.length === 5 ? `${time}:00` : time}`);

const timeOf = (local: CalendarDateTime): string =>
  `${pad(local.hour)}:${pad(local.minute)}`;

/** A blank form for a new event at `start` (minutes long) in `zone`. */
export const newForm = (
  calendarId: CalendarId | null,
  start: CalendarDateTime,
  minutes: number,
  zone: string,
  allDay = false,
  accountId: AccountId | null = null
): EventForm => {
  const end = start.add({ minutes });
  return {
    accountId,
    alerts: allDay ? [] : [10],
    allDay,
    calendarId,
    description: "",
    endDate: (allDay ? start : end).toString().slice(0, 10),
    endTime: timeOf(end),
    guests: [],
    keepHtmlDescription: false,
    location: "",
    recurrence: NO_RECURRENCE,
    startDate: start.toString().slice(0, 10),
    startTime: timeOf(start),
    timeZone: zone,
    title: "",
  };
};

/** Default length of a new event, in minutes. */
export const DEFAULT_MINUTES = 60;

/**
 * Minutes from `start` to a dragged `end` ("2026-10-26T11:30"); the
 * default length without one, or when it isn't after the start.
 */
export const rangeMinutes = (
  start: CalendarDateTime,
  end: string | null,
  allDay: boolean
): number => {
  if (end === null || allDay) {
    return DEFAULT_MINUTES;
  }
  const until = parseLocalDateTime(end.length === 16 ? `${end}:00` : end);
  if (until === null) {
    return DEFAULT_MINUTES;
  }
  // Wall-clock minutes (the grid shows wall times).
  const minutes = Math.round(
    (toZoned(until, "UTC").toDate().getTime() -
      toZoned(start, "UTC").toDate().getTime()) /
      MINUTE_MS
  );
  return minutes > 0 ? minutes : DEFAULT_MINUTES;
};

/** The form for editing an event. */
export const formOf = (
  detail: EventDetail,
  recurrence: Recurrence
): EventForm => {
  const start = parseLocalDateTime(detail.localStart);
  const startDate = detail.localStart.slice(0, 10);
  let end = start;
  if (start !== null) {
    end = detail.allDay
      ? start.add({ days: Math.max(1, detail.durationDays) - 1 })
      : start.add({ minutes: detail.durationMinutes });
  }
  return {
    accountId: detail.accountId,
    alerts: detail.alerts.map((alert) => alert.minutesBefore),
    allDay: detail.allDay,
    calendarId: detail.calendarId,
    description: detail.descriptionIsHtml ? "" : (detail.description ?? ""),
    endDate: end === null ? startDate : end.toString().slice(0, 10),
    endTime: end === null ? "10:00" : timeOf(end),
    guests: detail.participants
      .filter((participant) => !participant.isOrganizer)
      .map((participant) => ({
        email: participant.email,
        name: participant.name,
        optional: participant.optional,
      })),
    keepHtmlDescription: detail.descriptionIsHtml,
    location: detail.location ?? "",
    recurrence,
    startDate,
    startTime: start === null ? "09:00" : timeOf(start),
    timeZone: detail.timeZone,
    title: detail.title,
  };
};

export interface Timing {
  readonly start: string;
  readonly duration: string;
}

/** Start and duration of the form, or why they are wrong. */
export const timingOf = (form: EventForm): Timing | FormProblem => {
  if (form.allDay) {
    const first = parseLocalDate(form.startDate);
    const last = parseLocalDate(form.endDate);
    if (first === null) {
      return "invalid-start";
    }
    if (last === null) {
      return "invalid-end";
    }
    const days = last.compare(first) + 1;
    if (days < 1) {
      return "end-before-start";
    }
    return {
      duration: formatDuration(days, 0),
      start: `${first.toString()}T00:00:00`,
    };
  }
  const start = dateTimeOf(form.startDate, form.startTime);
  const end = dateTimeOf(form.endDate, form.endTime);
  if (start === null) {
    return "invalid-start";
  }
  if (end === null) {
    return "invalid-end";
  }
  // Exact minutes between the two wall times in the event's zone (a change
  // of time in between is counted, like every calendar does).
  const zone = form.timeZone ?? "UTC";
  const minutes = Math.round(
    (toZoned(end, zone).toDate().getTime() -
      toZoned(start, zone).toDate().getTime()) /
      MINUTE_MS
  );
  if (minutes < 0) {
    return "end-before-start";
  }
  return {
    duration: formatDuration(0, minutes),
    start: formatLocalDateTime(start),
  };
};

export const validateForm = (form: EventForm): FormProblem | null => {
  if (form.calendarId === null) {
    return "no-calendar";
  }
  const timing = timingOf(form);
  if (typeof timing === "string") {
    return timing;
  }
  return form.guests.some((guest) => addressOf(guest.email) === null)
    ? "invalid-guest"
    : null;
};

export interface Me {
  readonly email: string;
  readonly name: string | null;
}

/** A short random key for new map entries (participants, alerts). */
const newKey = (prefix: string): string =>
  `${prefix}${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;

const guestParticipant = (
  guest: GuestInput,
  previous: Participant | undefined
): Participant => ({
  ...previous,
  "@type": "Participant",
  calendarAddress: `mailto:${guest.email}`,
  expectReply: previous?.expectReply ?? true,
  kind: previous?.kind ?? "individual",
  ...(guest.name === null || guest.name === "" ? {} : { name: guest.name }),
  participationStatus: previous?.participationStatus ?? "needs-action",
  roles: guest.optional ? { optional: true } : { attendee: true },
});

/**
 * Participants to store. With guests, the user is the organizer (owner,
 * accepted). Existing entries keep their key and details (their answer);
 * removed guests are dropped.
 */
export const participantsOf = (
  guests: readonly GuestInput[],
  previous: Readonly<Record<string, Participant>> | null | undefined,
  me: Me
): Record<string, Participant> | null => {
  const cleaned = guests.flatMap((guest) => {
    const email = addressOf(guest.email);
    return email === null || email === me.email.toLowerCase()
      ? []
      : [{ ...guest, email }];
  });
  const byEmail = new Map<string, [string, Participant]>();
  // Entries without an address (a room, a group by name…) aren't in the
  // form, so the user can't remove them: they stay as they are.
  const unlisted: Record<string, Participant> = {};
  for (const [key, participant] of Object.entries(previous ?? {})) {
    const email =
      addressOf(participant.calendarAddress) ?? addressOf(participant.email);
    if (email === null) {
      unlisted[key] = participant;
    } else {
      byEmail.set(email, [key, participant]);
    }
  }
  if (cleaned.length === 0) {
    return Object.keys(unlisted).length === 0 ? null : unlisted;
  }
  const result: Record<string, Participant> = { ...unlisted };
  const myEmail = me.email.toLowerCase();
  const mine = byEmail.get(myEmail);
  result[mine?.[0] ?? newKey("o")] = {
    ...mine?.[1],
    "@type": "Participant",
    calendarAddress: `mailto:${myEmail}`,
    ...(me.name === null || me.name === "" ? {} : { name: me.name }),
    participationStatus: mine?.[1].participationStatus ?? "accepted",
    roles: { ...mine?.[1].roles, attendee: true, owner: true },
  };
  for (const guest of cleaned) {
    const existing = byEmail.get(guest.email);
    result[existing?.[0] ?? newKey("p")] = guestParticipant(
      guest,
      existing?.[1]
    );
  }
  return result;
};

/** The first of `a1`, `a2`… (or `l1`…) not taken in `map`. */
const freeKey = (prefix: string, map: Readonly<Record<string, unknown>>) => {
  let index = 1;
  while (Object.hasOwn(map, `${prefix}${String(index)}`)) {
    index += 1;
  }
  return `${prefix}${String(index)}`;
};

/** Minutes before the start of an alert the form can show; else null. */
const formMinutes = (alert: Alert): number | null => {
  const { offset, relativeTo } = alert.trigger;
  if (
    offset === undefined ||
    (relativeTo !== undefined && relativeTo !== "start")
  ) {
    return null;
  }
  const duration = parseDuration(offset);
  return duration === null ? null : -durationMinutes(duration);
};

/** The alert a snooze hangs from (`snoozePatch`, alerts.ts). */
const snoozeParent = (alert: Alert): string | undefined => {
  const related: unknown = Reflect.get(alert, "relatedTo");
  return typeof related === "object" && related !== null
    ? Object.keys(related)[0]
    : undefined;
};

/**
 * Alerts to store, merged with the stored ones. The form only shows "N
 * minutes before the start":
 * - an alert it shows keeps its key and everything else (`acknowledged`)
 *   while its minutes stay; new minutes get a new key;
 * - alerts it can't show (absolute, relative to the end) stay as they are;
 * - a snooze stays only while its alert stays and the event hasn't moved
 *   (it rings at a fixed time, which would be the old one).
 */
export const alertsOf = (
  minutes: readonly number[],
  previous: Readonly<Record<string, Alert>> | null | undefined,
  moved: boolean
): Record<string, Alert> | null => {
  const wanted = new Set(minutes);
  const result: Record<string, Alert> = {};
  const snoozes: [string, Alert][] = [];
  for (const [key, alert] of Object.entries(previous ?? {})) {
    if (key.startsWith(SNOOZE_PREFIX)) {
      snoozes.push([key, alert]);
      continue;
    }
    const before = formMinutes(alert);
    if (before === null) {
      result[key] = alert;
    } else if (wanted.has(before)) {
      result[key] = alert;
      wanted.delete(before);
    }
  }
  if (!moved) {
    for (const [key, alert] of snoozes) {
      const parent = snoozeParent(alert);
      if (parent !== undefined && Object.hasOwn(result, parent)) {
        result[key] = alert;
      }
    }
  }
  for (const before of wanted) {
    result[freeKey("a", result)] = {
      "@type": "Alert",
      action: "display",
      trigger: {
        "@type": "OffsetTrigger",
        offset: formatOffset(-before),
        relativeTo: "start",
      },
    };
  }
  return Object.keys(result).length === 0 ? null : result;
};

/**
 * Locations to store. The form shows one name, the first stored location
 * with a name (`toDetail`): only that entry changes (or goes, when the
 * name is cleared); the others stay.
 */
export const locationsOf = (
  name: string,
  previous: Readonly<Record<string, Location>> | null | undefined
): Record<string, Location> | null => {
  const entries = Object.entries(previous ?? {});
  const shown = entries.find(([, location]) => {
    const stored = location.name?.trim();
    return stored !== undefined && stored !== "";
  });
  if (shown === undefined) {
    const result: Record<string, Location> = Object.fromEntries(entries);
    if (name !== "") {
      result[freeKey("l", result)] = { "@type": "Location", name };
    }
    return Object.keys(result).length === 0 ? null : result;
  }
  const [shownKey, shownLocation] = shown;
  const result: Record<string, Location> = Object.fromEntries(
    entries.flatMap(([key, location]): [string, Location][] => {
      if (key !== shownKey || shownLocation.name?.trim() === name) {
        return [[key, location]];
      }
      return name === "" ? [] : [[key, { ...location, name }]];
    })
  );
  return Object.keys(result).length === 0 ? null : result;
};

export interface BuildOptions {
  readonly me: Me;
  /** Participants stored now (to keep keys and answers). */
  readonly previousParticipants?: Readonly<Record<string, Participant>> | null;
  /** Alerts and locations stored now (what the form can't show stays). */
  readonly previousAlerts?: Readonly<Record<string, Alert>> | null;
  readonly previousLocations?: Readonly<Record<string, Location>> | null;
  /**
   * Where the event was ("YYYY-MM-DDTHH:MM:SS" and zone): a snooze only
   * stays if it hasn't moved. Without it, snoozes go.
   */
  readonly previousStart?: {
    readonly start: string;
    readonly timeZone: string | null;
  } | null;
  /** An occurrence: no calendar, recurrence or zone changes. */
  readonly occurrence?: boolean;
}

/**
 * JSCalendar properties for `CalendarEvent/set` (create, or a full update
 * of every property the form shows). Throws on an invalid form (check
 * `validateForm` first).
 */
export const eventProperties = (
  form: EventForm,
  {
    me,
    occurrence = false,
    previousAlerts = null,
    previousLocations = null,
    previousParticipants = null,
    previousStart = null,
  }: BuildOptions
): Record<string, unknown> => {
  const timing = timingOf(form);
  if (typeof timing === "string" || form.calendarId === null) {
    throw new Error(
      `Invalid event form: ${typeof timing === "string" ? timing : "no-calendar"}`
    );
  }
  const participants = participantsOf(form.guests, previousParticipants, me);
  const timeZone = form.allDay ? null : form.timeZone;
  const moved =
    previousStart === null ||
    previousStart.start !== timing.start ||
    (!occurrence && previousStart.timeZone !== timeZone);
  const properties: Record<string, unknown> = {
    alerts: alertsOf(form.alerts, previousAlerts, moved),
    duration: timing.duration,
    locations: locationsOf(form.location.trim(), previousLocations),
    participants,
    showWithoutTime: form.allDay,
    start: timing.start,
    title: form.title.trim(),
  };
  if (!occurrence) {
    properties.calendarIds = { [form.calendarId]: true };
    properties.recurrenceRule = toRule(form.recurrence, form.allDay);
    properties.timeZone = timeZone;
  }
  // Organized by the user when there are guests (participants left without
  // an address alone don't make it a meeting).
  const hasGuests = Object.values(participants ?? {}).some(
    (participant) => addressOf(participant.calendarAddress) !== null
  );
  properties.organizerCalendarAddress = hasGuests
    ? `mailto:${me.email.toLowerCase()}`
    : null;
  if (!form.keepHtmlDescription) {
    const description = form.description.trim();
    properties.description = description === "" ? null : description;
    properties.descriptionContentType = null;
  }
  return properties;
};

/** For a new event, null values are simply left out. */
export const withoutNulls = (
  properties: Record<string, unknown>
): Record<string, unknown> =>
  Object.fromEntries(
    Object.entries(properties).filter(([, value]) => value !== null)
  );

/**
 * Moving one occurrence of a series by dragging, applied to the whole
 * series: the same shift of the series' start (wall time, in its zone).
 */
export const shiftLocalStart = (
  seriesStart: string,
  from: string,
  to: string
): string | null => {
  const series = parseLocalDateTime(seriesStart);
  const before = parseLocalDateTime(from);
  const after = parseLocalDateTime(to);
  if (series === null || before === null || after === null) {
    return null;
  }
  const minutes = Math.round(
    (toZoned(after, "UTC").toDate().getTime() -
      toZoned(before, "UTC").toDate().getTime()) /
      MINUTE_MS
  );
  return formatLocalDateTime(series.add({ minutes }));
};

/**
 * Whether a change may go to ONE occurrence ("This event"). Turning one
 * occurrence into an all-day one, or the other way round, never goes
 * alone. One day of an all-day series may.
 */
export const canChangeOneOccurrence = (
  detail: { readonly allDay: boolean; readonly isOccurrence: boolean },
  allDayAfter: boolean
): boolean => detail.isOccurrence && detail.allDay === allDayAfter;

/** The local start after moving by whole days and minutes. */
export const movedStart = (
  localStart: string,
  days: number,
  minutes: number
): string | null => {
  const start = parseLocalDateTime(localStart);
  return start === null
    ? null
    : formatLocalDateTime(start.add({ days }).add({ minutes }));
};

/** Patch for answering an invitation stored in the calendar. */
export const statusPatch = (
  participantKey: string,
  status: ParticipationStatus
): Record<string, unknown> => ({
  [`participants/${participantKey}/participationStatus`]: status,
});

export const dateOnly = (date: CalendarDate): string => date.toString();

/**
 * Where an occurrence shows right after being dragged or moved with the
 * keyboard, before the server answers (the list is fetched again after
 * the change). Moved in the user's zone like the grid shows it; the server
 * moves it in the event's own zone, the same wall time unless the two
 * zones change offset on different days. Null when it can't be told
 * (an all-day event moved by minutes or resized): wait for the server.
 */
export const movedOccurrence = (
  occurrence: Occurrence,
  move: {
    readonly days: number;
    readonly minutes: number;
    readonly resize: number;
  },
  zone: string
): Occurrence | null => {
  if (occurrence.allDay) {
    const first = parseLocalDate(occurrence.firstDay);
    const end = parseLocalDate(occurrence.endDay);
    if (
      move.minutes !== 0 ||
      move.resize !== 0 ||
      first === null ||
      end === null
    ) {
      return null;
    }
    const newFirst = first.add({ days: move.days });
    const newEnd = end.add({ days: move.days });
    return {
      ...occurrence,
      end: startOfDayMs(newEnd, zone),
      endDay: newEnd.toString(),
      firstDay: newFirst.toString(),
      start: startOfDayMs(newFirst, zone),
    };
  }
  const start = fromAbsolute(occurrence.start, zone)
    .add({ days: move.days })
    .add({ minutes: move.minutes })
    .toDate()
    .getTime();
  const minutes = Math.round((occurrence.end - occurrence.start) / MINUTE_MS);
  // As the patch does: never shorter than 15 minutes when resized.
  const length =
    move.resize === 0 ? minutes : Math.max(15, minutes + move.resize);
  return { ...occurrence, end: start + length * MINUTE_MS, start };
};
