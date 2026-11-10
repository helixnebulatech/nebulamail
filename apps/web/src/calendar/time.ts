// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Times of JSCalendar events (@internationalized/date). An
// event has a local `start` ("2026-10-26T10:00:00"), a `duration` and a
// `timeZone` (IANA; none = floating, the same wall time everywhere). All-day
// events (`showWithoutTime`) are dates. Everything is shown in the user's
// time zone. Pure functions: the zone and locale are parameters.

import type {
  CalendarDate,
  CalendarDateTime,
  DateTimeDuration,
  ZonedDateTime,
} from "@internationalized/date";
import {
  fromAbsolute,
  parseDate,
  parseDateTime,
  parseDuration as parseIsoDuration,
  toCalendarDate,
  toCalendarDateTime,
  toZoned,
} from "@internationalized/date";

export const MINUTE_MS = 60_000;
export const DAY_MINUTES = 1440;

/** Formatters by locale and options: building one is slow (ICU data). */
const formats = new Map<string, Intl.DateTimeFormat>();
/**
 * A bound on the cache: the views use a handful of formats, the zone
 * picker one per IANA zone (about 600).
 */
const MAX_FORMATS = 1024;

/**
 * A cached `Intl.DateTimeFormat`, the same as `new Intl.DateTimeFormat(locale,
 * options)` (throws like it for an unknown zone or locale; those aren't
 * cached). Formatters are immutable, so sharing them is safe.
 */
export const dateTimeFormat = (
  locale: string | undefined,
  options: Intl.DateTimeFormatOptions
): Intl.DateTimeFormat => {
  const key = `${locale ?? ""}\u0000${JSON.stringify(options)}`;
  let format = formats.get(key);
  if (format === undefined) {
    format = new Intl.DateTimeFormat(locale, options);
    if (formats.size >= MAX_FORMATS) {
      formats.clear();
    }
    formats.set(key, format);
  }
  return format;
};

/** The browser's time zone (IANA), UTC if it can't tell. */
export const browserTimeZone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

/** Whether the runtime knows this IANA time zone. */
export const isValidTimeZone = (zone: string): boolean => {
  if (zone === "" || zone.length > 64) {
    return false;
  }
  try {
    dateTimeFormat("en", { timeZone: zone }).format(0);
    return true;
  } catch {
    return false;
  }
};

/** Every IANA zone the browser knows (for the editor's zone list). */
export const knownTimeZones = (): readonly string[] => {
  try {
    return Intl.supportedValuesOf("timeZone");
  } catch {
    return ["UTC"];
  }
};

/** "2026-10-26T10:00:00" (seconds optional); null if not a local date-time. */
export const parseLocalDateTime = (
  value: string | null | undefined
): CalendarDateTime | null => {
  if (value === null || value === undefined) {
    return null;
  }
  try {
    return parseDateTime(value);
  } catch {
    return null;
  }
};

/** "2026-10-26" (also takes a date-time and keeps the date). */
export const parseLocalDate = (
  value: string | null | undefined
): CalendarDate | null => {
  if (value === null || value === undefined) {
    return null;
  }
  try {
    return parseDate(value.slice(0, 10));
  } catch {
    return null;
  }
};

/** JSCalendar LocalDateTime: "2026-10-26T10:00:00" (no zone, no offset). */
export const formatLocalDateTime = (
  value: CalendarDateTime | ZonedDateTime
): string => toCalendarDateTime(value).toString().slice(0, 19);

/** ISO 8601 duration (RFC 8984 Duration / SignedDuration); null if invalid. */
export const parseDuration = (
  value: string | null | undefined
): Required<DateTimeDuration> | null => {
  if (value === null || value === undefined || value === "") {
    return null;
  }
  try {
    return parseIsoDuration(value);
  } catch {
    return null;
  }
};

/**
 * A duration in minutes and days, as JSCalendar wants it: "P1D", "PT1H30M",
 * "P1DT2H". Days are nominal (a day keeps the wall time across a change of
 * time); minutes are exact.
 */
export const formatDuration = (days: number, minutes: number): string => {
  const wholeDays = Math.max(0, Math.trunc(days));
  const total = Math.max(0, Math.round(minutes));
  const hours = Math.floor(total / 60);
  const rest = total % 60;
  const time =
    hours === 0 && rest === 0
      ? ""
      : `T${hours === 0 ? "" : `${String(hours)}H`}${rest === 0 ? "" : `${String(rest)}M`}`;
  if (wholeDays === 0 && time === "") {
    return "PT0S";
  }
  return `P${wholeDays === 0 ? "" : `${String(wholeDays)}D`}${time}`;
};

/** Signed offset of an alert, e.g. -15 → "-PT15M" (minutes before). */
export const formatOffset = (minutes: number): string => {
  const sign = minutes < 0 ? "-" : "";
  const absolute = Math.abs(Math.round(minutes));
  const days = Math.floor(absolute / DAY_MINUTES);
  return `${sign}${formatDuration(days, absolute - days * DAY_MINUTES)}`;
};

/** Total minutes of a (signed) duration, counting a day as 24 h. */
export const durationMinutes = (
  duration: Required<DateTimeDuration>
): number => {
  const sign =
    duration.weeks < 0 ||
    duration.days < 0 ||
    duration.hours < 0 ||
    duration.minutes < 0
      ? -1
      : 1;
  const minutes =
    Math.abs(duration.weeks) * 7 * DAY_MINUTES +
    Math.abs(duration.days) * DAY_MINUTES +
    Math.abs(duration.hours) * 60 +
    Math.abs(duration.minutes) +
    Math.abs(duration.seconds) / 60;
  return sign * minutes;
};

/** What `toInstant` and friends need from an event. */
export interface EventTiming {
  readonly start?: string | null | undefined;
  readonly duration?: string | null | undefined;
  readonly timeZone?: string | null | undefined;
  readonly showWithoutTime?: boolean | null | undefined;
}

/** The zone an event's wall time is in; floating events use `display`. */
export const eventZone = (event: EventTiming, display: string): string =>
  event.timeZone !== null &&
  event.timeZone !== undefined &&
  isValidTimeZone(event.timeZone)
    ? event.timeZone
    : display;

/** Start of a timed event as a zoned date-time in its own zone. */
export const zonedStart = (
  event: EventTiming,
  display: string
): ZonedDateTime | null => {
  const local = parseLocalDateTime(event.start);
  return local === null ? null : toZoned(local, eventZone(event, display));
};

/** Adds a duration (days nominal, time exact), never going backwards. */
export const addDuration = (
  start: ZonedDateTime,
  duration: Required<DateTimeDuration> | null
): ZonedDateTime => {
  if (duration === null || durationMinutes(duration) <= 0) {
    return start;
  }
  return start.add({ days: duration.days, weeks: duration.weeks }).add({
    hours: duration.hours,
    minutes: duration.minutes,
    seconds: duration.seconds,
  });
};

/** [start, end) in epoch ms of a timed event; null if `start` is invalid. */
export const timedRange = (
  event: EventTiming,
  display: string
): { readonly start: number; readonly end: number } | null => {
  const start = zonedStart(event, display);
  if (start === null) {
    return null;
  }
  const end = addDuration(start, parseDuration(event.duration));
  return { end: end.toDate().getTime(), start: start.toDate().getTime() };
};

/**
 * Days of an all-day event: [first, end) with `end` exclusive. A missing or
 * zero duration is one day.
 */
export const dayRange = (
  event: EventTiming
): { readonly first: CalendarDate; readonly end: CalendarDate } | null => {
  const first = parseLocalDate(event.start);
  if (first === null) {
    return null;
  }
  const duration = parseDuration(event.duration);
  const days =
    duration === null ? 1 : Math.max(1, duration.weeks * 7 + duration.days);
  return { end: first.add({ days }), first };
};

/** Midnight of a date in a zone, in epoch ms. */
export const startOfDayMs = (date: CalendarDate, zone: string): number =>
  toZoned(date, zone).toDate().getTime();

/** The calendar date of an instant in a zone. */
export const dateInZone = (ms: number, zone: string): CalendarDate =>
  toCalendarDate(fromAbsolute(ms, zone));

/** Minutes since midnight of an instant in a zone (0–1439). */
export const minutesInDay = (ms: number, zone: string): number => {
  const zoned = fromAbsolute(ms, zone);
  return zoned.hour * 60 + zoned.minute;
};

/** Short time in the locale, e.g. "9:30" or "9:30 AM". */
export const formatTime = (ms: number, zone: string, locale: string): string =>
  dateTimeFormat(locale, {
    hour: "numeric",
    minute: "2-digit",
    timeZone: zone,
  }).format(ms);

/** Whether a locale writes times with AM/PM. */
export const uses12Hours = (locale: string): boolean =>
  dateTimeFormat(locale, { hour: "numeric" }).resolvedOptions().hourCycle ===
  "h12";

/** A date in the locale; `zone` only matters for the weekday of midnight. */
export const formatDate = (
  date: CalendarDate,
  locale: string,
  options: Intl.DateTimeFormatOptions
): string =>
  dateTimeFormat(locale, { ...options, timeZone: "UTC" }).format(
    toZoned(date, "UTC").toDate()
  );

/** "Europe/Madrid" → "GMT+1" (the zone's short offset at `at`). */
export const timeZoneOffset = (
  zone: string,
  locale: string,
  at: number
): string => {
  try {
    return (
      dateTimeFormat(locale, { timeZone: zone, timeZoneName: "shortOffset" })
        .formatToParts(at)
        .find((part) => part.type === "timeZoneName")?.value ?? ""
    );
  } catch {
    return "";
  }
};

/** "Europe/Madrid" → "Madrid (GMT+1)" style label for pickers. */
export const timeZoneLabel = (
  zone: string,
  locale: string,
  at: number
): string => {
  try {
    const offset =
      dateTimeFormat(locale, {
        timeZone: zone,
        timeZoneName: "shortOffset",
      })
        .formatToParts(at)
        .find((part) => part.type === "timeZoneName")?.value ?? "";
    const city = zone.split("/").at(-1)?.replaceAll("_", " ") ?? zone;
    return offset === "" ? city : `${city} (${offset})`;
  } catch {
    return zone;
  }
};
