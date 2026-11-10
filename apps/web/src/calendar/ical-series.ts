// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// iCalendar values of a repeating series (RFC 5545), for the iTIP messages
// the app writes itself (a delegated task)
// DTSTART / DUE / EXDATE / RECURRENCE-ID as a day,
// a wall time in a zone (with its VTIMEZONE), in UTC or floating, and the
// RRULE of a JSCalendar rule (RFC 8984 §4.3.3 ↔ RFC 5545 §3.3.10, RSCALE
// and SKIP from RFC 7529). Pure, with tests (itip.test.ts).
//
// Everything is checked before it is written: a rule or zone that can't be
// written exactly gives null, and the caller doesn't send the series.

import { fromAbsolute, toZoned } from "@internationalized/date";
import type { RecurrenceRule } from "@nebulamail/jmap-client";

import { parseLocalDate, parseLocalDateTime } from "./time";

/** "2026-10-24T08:00:00.000Z" → "20261024T080000Z". */
export const utcStamp = (ms: number): string =>
  new Date(ms)
    .toISOString()
    .replaceAll(/[-:]/gu, "")
    .replace(/\.\d{3}/u, "");

/** iCalendar DATE ("20261130") of a date or date-time; null if invalid. */
export const dateStamp = (value: string): string | null =>
  parseLocalDate(value)?.toString().replaceAll("-", "") ?? null;

/** A local date-time in iCalendar form ("20261030T110000"). */
export const localStamp = (local: string): string | null => {
  const parsed = parseLocalDateTime(local);
  if (parsed === null) {
    return null;
  }
  return parsed.toString().slice(0, 19).replaceAll(/[-:]/gu, "");
};

/** A zone name as a TZID value: IANA names only, nothing that breaks the line. */
export const tzid = (zone: string): string | null =>
  /^[\w+./-]{1,64}$/u.test(zone) ? zone : null;

/** Zones written as UTC ("…Z"), without a VTIMEZONE. */
export const isUtcZone = (zone: string | null): boolean =>
  zone === "Etc/UTC" || zone === "UTC";

/** A DTSTART, DUE, EXDATE or RECURRENCE-ID of a series. */
export type ICalTime =
  | { readonly kind: "date"; readonly date: string }
  | {
      readonly kind: "local";
      /** Wall time, "2026-11-06T09:00:00". */
      readonly local: string;
      /** IANA zone; null: floating (the same wall time everywhere). */
      readonly timeZone: string | null;
    };

/** Whether the zone is one `Intl` knows (a VTIMEZONE can be written). */
const knownZone = (zone: string): boolean => {
  try {
    fromAbsolute(0, zone);
    return true;
  } catch {
    return false;
  }
};

/**
 * A property with a series value: `DTSTART;TZID=Europe/Madrid:20261106T090000`,
 * `DUE;VALUE=DATE:20261106`, `EXDATE:20261113T080000Z`. Null if the value
 * or the zone can't be written.
 */
export const timeLine = (name: string, time: ICalTime): string | null => {
  if (time.kind === "date") {
    const day = dateStamp(time.date);
    return day === null ? null : `${name};VALUE=DATE:${day}`;
  }
  const stamp = localStamp(time.local);
  if (stamp === null) {
    return null;
  }
  if (time.timeZone === null) {
    return `${name}:${stamp}`;
  }
  if (isUtcZone(time.timeZone)) {
    return `${name}:${stamp}Z`;
  }
  const id = tzid(time.timeZone);
  return id === null || !knownZone(id) ? null : `${name};TZID=${id}:${stamp}`;
};

/** Epoch ms of a time with a zone (a floating one or a day: null). */
export const instantOf = (time: ICalTime): number | null => {
  if (time.kind === "date" || time.timeZone === null) {
    return null;
  }
  const local = parseLocalDateTime(time.local);
  if (local === null || !knownZone(time.timeZone)) {
    return null;
  }
  return toZoned(local, time.timeZone).toDate().getTime();
};

// ---------------------------------------------------------------- RRULE

const FREQUENCIES: ReadonlySet<string> = new Set([
  "yearly",
  "monthly",
  "weekly",
  "daily",
  "hourly",
  "minutely",
  "secondly",
]);
const DAYS: ReadonlySet<string> = new Set([
  "mo",
  "tu",
  "we",
  "th",
  "fr",
  "sa",
  "su",
]);
const SKIPS: ReadonlySet<string> = new Set(["omit", "backward", "forward"]);

/** Whole numbers in [min, max], and not zero if `signed` (-max..-1, 1..max). */
const numbers = (
  values: readonly number[] | undefined,
  min: number,
  max: number,
  signed: boolean
): string | null | undefined => {
  if (values === undefined || values.length === 0) {
    return undefined;
  }
  const ok = values.every(
    (value) =>
      Number.isInteger(value) &&
      (signed
        ? value !== 0 && Math.abs(value) <= max
        : value >= min && value <= max)
  );
  return ok ? values.join(",") : null;
};

/** BYDAY: "MO", "-1FR", "2TU". */
const byDay = (days: RecurrenceRule["byDay"]): string | null | undefined => {
  if (days === undefined || days.length === 0) {
    return undefined;
  }
  const out: string[] = [];
  for (const { day, nthOfPeriod } of days) {
    const name = day.toLowerCase();
    if (
      !DAYS.has(name) ||
      (nthOfPeriod !== undefined &&
        (!Number.isInteger(nthOfPeriod) ||
          nthOfPeriod === 0 ||
          Math.abs(nthOfPeriod) > 53))
    ) {
      return null;
    }
    out.push(
      `${nthOfPeriod === undefined ? "" : String(nthOfPeriod)}${name.toUpperCase()}`
    );
  }
  return out.join(",");
};

/**
 * BYMONTH: "3"; "5L" (a leap month) and 13 only with a non-Gregorian
 * RSCALE (RFC 7529 §4.1: the "L" suffix is RSCALE's; RFC 5545 §3.3.10:
 * months are 1 to 12, and the Gregorian calendar has no leap months).
 */
const byMonth = (
  months: readonly string[] | undefined,
  rscale: string | undefined
): string | null | undefined => {
  if (months === undefined || months.length === 0) {
    return undefined;
  }
  const scaled = rscale !== undefined && rscale.toLowerCase() !== "gregorian";
  const month = scaled ? /^(?:1[0-3]|[1-9])L?$/u : /^(?:1[0-2]|[1-9])$/u;
  return months.every((value) => month.test(value)) ? months.join(",") : null;
};

/**
 * UNTIL in the form RFC 5545 §3.3.10 asks for: a DATE for a series of days,
 * floating for a floating start, else UTC. JSCalendar's `until` is a wall
 * time in the series' zone (inclusive).
 */
const untilValue = (until: string, start: ICalTime): string | null => {
  if (start.kind === "date") {
    return dateStamp(until);
  }
  if (parseLocalDateTime(until) === null) {
    return null;
  }
  if (start.timeZone === null) {
    return localStamp(until);
  }
  const at = instantOf({
    kind: "local",
    local: until,
    timeZone: start.timeZone,
  });
  return at === null ? null : utcStamp(at);
};

/** A part that is fine (its text, or nothing to write) or can't be written. */
type Part = string | null | undefined;

/** RSCALE and SKIP (RFC 7529; SKIP needs RSCALE, "omit" is the default). */
const scaleParts = (rule: RecurrenceRule): string[] | null => {
  const rscale = rule.rscale?.toLowerCase();
  const skip = rule.skip?.toLowerCase();
  if (
    (rscale !== undefined && !/^[a-z0-9-]{1,32}$/u.test(rscale)) ||
    (skip !== undefined && !SKIPS.has(skip))
  ) {
    return null;
  }
  const skips = skip !== undefined && skip !== "omit";
  if ((rscale === undefined || rscale === "gregorian") && !skips) {
    return [];
  }
  return [
    `RSCALE=${(rscale ?? "gregorian").toUpperCase()}`,
    ...(skips ? [`SKIP=${skip.toUpperCase()}`] : []),
  ];
};

const intervalPart = (rule: RecurrenceRule): Part => {
  const interval = rule.interval ?? 1;
  if (!Number.isInteger(interval) || interval < 1) {
    return null;
  }
  return interval > 1 ? `INTERVAL=${String(interval)}` : undefined;
};

const weekStartPart = (rule: RecurrenceRule): Part => {
  const day = rule.firstDayOfWeek?.toLowerCase();
  if (day === undefined || day === "mo") {
    return undefined;
  }
  return DAYS.has(day) ? `WKST=${day.toUpperCase()}` : null;
};

const named = (name: string, value: Part): Part =>
  typeof value === "string" ? `${name}=${value}` : value;

/** COUNT or UNTIL (not both, RFC 5545 §3.3.10). */
const endPart = (rule: RecurrenceRule, start: ICalTime): Part => {
  if (rule.count !== undefined && rule.until !== undefined) {
    return null;
  }
  if (rule.count !== undefined) {
    return Number.isInteger(rule.count) && rule.count >= 1
      ? `COUNT=${String(rule.count)}`
      : null;
  }
  return rule.until === undefined
    ? undefined
    : named("UNTIL", untilValue(rule.until, start));
};

const some = (values: readonly unknown[] | undefined): boolean =>
  values !== undefined && values.length > 0;

/**
 * RFC 5545 §3.3.10's MUSTs on which parts go with which FREQ: BYWEEKNO
 * only with YEARLY; BYYEARDAY never with DAILY, WEEKLY or MONTHLY;
 * BYMONTHDAY never with WEEKLY; a BYDAY with a number only with MONTHLY
 * or YEARLY, and never with YEARLY and BYWEEKNO; BYSETPOS only with
 * another BYxxx part.
 */
const combinationOk = (rule: RecurrenceRule, frequency: string): boolean => {
  const others = [
    rule.byMonth,
    rule.byWeekNo,
    rule.byYearDay,
    rule.byMonthDay,
    rule.byDay,
    rule.byHour,
    rule.byMinute,
    rule.bySecond,
  ].some(some);
  const numberedDay = (rule.byDay ?? []).some(
    (day) => day.nthOfPeriod !== undefined
  );
  return !(
    (some(rule.byWeekNo) && frequency !== "yearly") ||
    (some(rule.byYearDay) &&
      ["daily", "weekly", "monthly"].includes(frequency)) ||
    (some(rule.byMonthDay) && frequency === "weekly") ||
    (numberedDay && frequency !== "monthly" && frequency !== "yearly") ||
    (numberedDay && frequency === "yearly" && some(rule.byWeekNo)) ||
    (some(rule.bySetPosition) && !others)
  );
};

/**
 * The RRULE value of a JSCalendar rule for a series starting at `start`
 * ("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T225959Z"). Null if
 * any part can't be written exactly, or the parts break RFC 5545's rules
 * on what goes with what (`combinationOk`).
 */
export const rruleOf = (
  rule: RecurrenceRule,
  start: ICalTime
): string | null => {
  const frequency = rule.frequency.toLowerCase();
  const scale = scaleParts(rule);
  if (
    !FREQUENCIES.has(frequency) ||
    scale === null ||
    !combinationOk(rule, frequency)
  ) {
    return null;
  }
  const parts: readonly Part[] = [
    `FREQ=${frequency.toUpperCase()}`,
    ...scale,
    intervalPart(rule),
    weekStartPart(rule),
    named("BYMONTH", byMonth(rule.byMonth, rule.rscale)),
    named("BYWEEKNO", numbers(rule.byWeekNo, 1, 53, true)),
    named("BYYEARDAY", numbers(rule.byYearDay, 1, 366, true)),
    named("BYMONTHDAY", numbers(rule.byMonthDay, 1, 31, true)),
    named("BYDAY", byDay(rule.byDay)),
    named("BYHOUR", numbers(rule.byHour, 0, 23, false)),
    named("BYMINUTE", numbers(rule.byMinute, 0, 59, false)),
    named("BYSECOND", numbers(rule.bySecond, 0, 60, false)),
    named("BYSETPOS", numbers(rule.bySetPosition, 1, 366, true)),
    endPart(rule, start),
  ];
  if (parts.includes(null)) {
    return null;
  }
  return parts.filter((part) => typeof part === "string").join(";");
};

// ---------------------------------------------------------------- VTIMEZONE

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;
/** Years of the zone's changes written after the series starts. */
export const VTIMEZONE_YEARS = 25;

interface Transition {
  /** Epoch ms of the change. */
  readonly at: number;
  /** Offsets before and after, ms. */
  readonly from: number;
  readonly to: number;
}

const offsetAt = (ms: number, zone: string): number =>
  fromAbsolute(ms, zone).offset;

/**
 * The zone's offset changes in [from, to): checked a week apart, then
 * narrowed to the minute (zones change at least weeks apart).
 */
const transitions = (zone: string, from: number, to: number): Transition[] => {
  const out: Transition[] = [];
  let at = from;
  let offset = offsetAt(at, zone);
  while (at < to) {
    const next = Math.min(at + 7 * DAY, to);
    const nextOffset = offsetAt(next, zone);
    if (nextOffset !== offset) {
      let low = Math.floor(at / MINUTE);
      let high = Math.ceil(next / MINUTE);
      while (high - low > 1) {
        const middle = Math.floor((low + high) / 2);
        if (offsetAt(middle * MINUTE, zone) === offset) {
          low = middle;
        } else {
          high = middle;
        }
      }
      out.push({ at: high * MINUTE, from: offset, to: nextOffset });
      offset = nextOffset;
    }
    at = next;
  }
  return out;
};

const pad = (value: number): string => String(value).padStart(2, "0");

/** "+0100", "-0330", "+054500" (RFC 5545 §3.3.14). */
const utcOffset = (ms: number): string => {
  const sign = ms < 0 ? "-" : "+";
  const total = Math.round(Math.abs(ms) / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return `${sign}${pad(hours)}${pad(minutes)}${seconds === 0 ? "" : pad(seconds)}`;
};

/** The wall time of an instant at an offset, as a DTSTART / RDATE value. */
const wallStamp = (at: number, offset: number): string =>
  utcStamp(at + offset).replace(/Z$/u, "");

type Observance = "STANDARD" | "DAYLIGHT";

const observanceOf = (change: Transition): Observance =>
  change.to > change.from ? "DAYLIGHT" : "STANDARD";

/**
 * The VTIMEZONE of `zone` from `from` (epoch ms) for `VTIMEZONE_YEARS`
 * years (or until `to`), from the browser's time zone data: one observance
 * for the offset at the start and one per kind of change, its onsets as
 * RDATEs (RFC 5545 §3.6.5; no rules guessed). After the last year the last
 * offset holds. Null for UTC, floating times or an unknown zone.
 */
export const vtimezoneLines = (
  zone: string | null,
  from: number,
  to?: number
): string[] | null => {
  if (zone === null || isUtcZone(zone)) {
    return null;
  }
  const id = tzid(zone);
  if (id === null || !knownZone(id)) {
    return null;
  }
  const begin = from - DAY;
  const end = Math.min(
    to ?? Number.POSITIVE_INFINITY,
    begin + Math.round(VTIMEZONE_YEARS * 365.25 * DAY)
  );
  const changes = transitions(id, begin, Math.max(begin, end) + DAY);
  const initial = offsetAt(begin, id);
  const [first] = changes;
  const lines = ["BEGIN:VTIMEZONE", `TZID:${id}`];
  const firstKind: Observance =
    first === undefined || observanceOf(first) === "DAYLIGHT"
      ? "STANDARD"
      : "DAYLIGHT";
  lines.push(
    `BEGIN:${firstKind}`,
    `DTSTART:${wallStamp(begin, initial)}`,
    `TZOFFSETFROM:${utcOffset(initial)}`,
    `TZOFFSETTO:${utcOffset(initial)}`,
    `END:${firstKind}`
  );
  const groups = new Map<string, Transition[]>();
  for (const change of changes) {
    const key = `${observanceOf(change)}|${String(change.from)}|${String(change.to)}`;
    groups.set(key, [...(groups.get(key) ?? []), change]);
  }
  for (const group of groups.values()) {
    const [onset, ...more] = group;
    if (onset !== undefined) {
      const kind = observanceOf(onset);
      lines.push(
        `BEGIN:${kind}`,
        `DTSTART:${wallStamp(onset.at, onset.from)}`,
        ...more.map((change) => `RDATE:${wallStamp(change.at, change.from)}`),
        `TZOFFSETFROM:${utcOffset(onset.from)}`,
        `TZOFFSETTO:${utcOffset(onset.to)}`,
        `END:${kind}`
      );
    }
  }
  lines.push("END:VTIMEZONE");
  return lines;
};
