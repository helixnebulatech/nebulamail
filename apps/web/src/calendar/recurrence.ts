// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Repeating events. The editor offers the common choices (every day,
// weekday, week, month, year, with an interval and an end by count or
// date); anything else a rule can say is shown read-only and kept as it is.
// The server expands the rule into occurrences (`expandRecurrences`), so
// there is no RRULE engine here.

import type { RecurrenceRule } from "@nebulamail/jmap-client";

import { dateTimeFormat, parseLocalDate } from "./time";

export type Frequency = "daily" | "weekly" | "monthly" | "yearly";

export const WEEKDAYS = ["mo", "tu", "we", "th", "fr", "sa", "su"] as const;
export type Weekday = (typeof WEEKDAYS)[number];

export type RecurrenceEnd =
  | { readonly kind: "never" }
  | { readonly kind: "count"; readonly count: number }
  | { readonly kind: "until"; readonly date: string };

/** What the editor can show and change. */
export type Recurrence =
  | { readonly kind: "none" }
  | {
      readonly kind: "simple";
      readonly frequency: Frequency;
      readonly interval: number;
      /** Weekly only; empty means the start's weekday. */
      readonly byDay: readonly Weekday[];
      readonly end: RecurrenceEnd;
    }
  /** A rule the editor can't express; kept untouched. */
  | { readonly kind: "custom"; readonly rule: RecurrenceRule };

export const NO_RECURRENCE: Recurrence = { kind: "none" };

const FREQUENCIES: ReadonlySet<string> = new Set([
  "daily",
  "weekly",
  "monthly",
  "yearly",
]);

const isFrequency = (value: string): value is Frequency =>
  FREQUENCIES.has(value);

const isWeekday = (value: string): value is Weekday =>
  (WEEKDAYS as readonly string[]).includes(value);

/** Parts of a rule the simple editor doesn't know. */
const EXTRA_PARTS = [
  "byHour",
  "byMinute",
  "byMonth",
  "byMonthDay",
  "bySecond",
  "bySetPosition",
  "byWeekNo",
  "byYearDay",
  "skip",
] as const;

const hasValue = (value: unknown): boolean =>
  value !== undefined &&
  value !== null &&
  !(Array.isArray(value) && value.length === 0);

/** The time `toRule` writes in `until` (inclusive end of that day). */
const untilTime = (allDay: boolean): string =>
  allDay ? "00:00:00" : "23:59:59";

/**
 * Whether `toRule` would write the rule's start of week and end the same:
 * WKST changes which days a WEEKLY rule with INTERVAL > 1 and BYDAY picks
 * (RFC 5545 §3.3.10; "mo" is the default, RFC 8984 §4.3.3), and an
 * `until` at another time of day ends the series elsewhere.
 */
const keptAsWritten = (rule: RecurrenceRule, allDay: boolean): boolean => {
  const weekStart = rule.firstDayOfWeek?.toLowerCase();
  if (weekStart !== undefined && weekStart !== "mo") {
    return false;
  }
  const time = rule.until?.slice(11);
  return time === undefined || time === "" || time === untilTime(allDay);
};

/**
 * Reads a rule; anything beyond the simple choices is "custom", kept as it
 * is when saved. `allDay`: the series is of days (its `until` ends at the
 * start of the day, else at its end).
 */
export const fromRule = (
  rule: RecurrenceRule | null | undefined,
  allDay: boolean
): Recurrence => {
  if (rule === null || rule === undefined) {
    return NO_RECURRENCE;
  }
  const custom: Recurrence = { kind: "custom", rule };
  const frequency = rule.frequency.toLowerCase();
  if (
    !isFrequency(frequency) ||
    EXTRA_PARTS.some((part) => hasValue(rule[part])) ||
    (rule.rscale !== undefined && rule.rscale !== "gregorian") ||
    (rule.count !== undefined && rule.until !== undefined) ||
    !keptAsWritten(rule, allDay)
  ) {
    return custom;
  }
  const days = rule.byDay ?? [];
  if (
    days.some((day) => day.nthOfPeriod !== undefined || !isWeekday(day.day)) ||
    (days.length > 0 && frequency !== "weekly")
  ) {
    return custom;
  }
  const until = parseLocalDate(rule.until);
  let end: RecurrenceEnd = { kind: "never" };
  if (rule.count !== undefined) {
    end = { count: rule.count, kind: "count" };
  } else if (until !== null) {
    end = { date: until.toString(), kind: "until" };
  }
  return {
    byDay: days.map((day) => day.day).filter(isWeekday),
    end,
    frequency,
    interval: Math.max(1, Math.trunc(rule.interval ?? 1)),
    kind: "simple",
  };
};

/**
 * The rule to store. `until` is inclusive: the end of that day for timed
 * events, the day itself for all-day ones (RFC 8984 §4.3.3).
 */
export const toRule = (
  recurrence: Recurrence,
  allDay: boolean
): RecurrenceRule | null => {
  switch (recurrence.kind) {
    case "none": {
      return null;
    }
    case "custom": {
      return recurrence.rule;
    }
    case "simple": {
      const { end } = recurrence;
      return {
        "@type": "RecurrenceRule",
        frequency: recurrence.frequency,
        ...(recurrence.interval > 1 ? { interval: recurrence.interval } : {}),
        ...(recurrence.frequency === "weekly" && recurrence.byDay.length > 0
          ? {
              byDay: WEEKDAYS.filter((day) =>
                recurrence.byDay.includes(day)
              ).map((day) => ({ "@type": "NDay", day })),
            }
          : {}),
        ...(end.kind === "count"
          ? { count: Math.max(1, Math.trunc(end.count)) }
          : {}),
        ...(end.kind === "until"
          ? { until: `${end.date}T${untilTime(allDay)}` }
          : {}),
      };
    }
    default: {
      return recurrence satisfies never;
    }
  }
};

/** The editor's quick choices. */
export type RecurrencePreset =
  | "none"
  | "daily"
  | "weekdays"
  | "weekly"
  | "monthly"
  | "yearly"
  | "custom";

const WORKWEEK: readonly Weekday[] = ["mo", "tu", "we", "th", "fr"];

const sameDays = (a: readonly Weekday[], b: readonly Weekday[]): boolean =>
  a.length === b.length && a.every((day) => b.includes(day));

/** Which quick choice a recurrence is ("custom" if it has more detail). */
export const presetOf = (recurrence: Recurrence): RecurrencePreset => {
  if (recurrence.kind !== "simple") {
    return recurrence.kind;
  }
  if (recurrence.interval !== 1 || recurrence.end.kind !== "never") {
    return "custom";
  }
  if (recurrence.frequency === "weekly") {
    if (sameDays(recurrence.byDay, WORKWEEK)) {
      return "weekdays";
    }
    return recurrence.byDay.length <= 1 ? "weekly" : "custom";
  }
  return recurrence.frequency;
};

/** A quick choice as a recurrence (keeps the end of `current`). */
export const fromPreset = (
  preset: Exclude<RecurrencePreset, "custom">,
  current: Recurrence
): Recurrence => {
  if (preset === "none") {
    return NO_RECURRENCE;
  }
  const end: RecurrenceEnd =
    current.kind === "simple" ? current.end : { kind: "never" };
  if (preset === "weekdays") {
    return {
      byDay: WORKWEEK,
      end,
      frequency: "weekly",
      interval: 1,
      kind: "simple",
    };
  }
  return { byDay: [], end, frequency: preset, interval: 1, kind: "simple" };
};

/** JSCalendar weekday of a date (Monday = "mo"). */
export const weekdayOf = (isoDate: string): Weekday | null => {
  const date = parseLocalDate(isoDate);
  if (date === null) {
    return null;
  }
  // 2024-01-01 was a Monday.
  const days = date.compare(parseLocalDate("2024-01-01") ?? date);
  return WEEKDAYS[((days % 7) + 7) % 7] ?? null;
};

export interface RecurrenceWords {
  readonly repeat: (frequency: Frequency, interval: number) => string;
  readonly onDays: (days: string) => string;
  readonly times: (count: number) => string;
  readonly until: (date: string) => string;
  readonly custom: () => string;
}

/** Weekday names in a locale ("lunes", "Monday"), Monday first. */
export const weekdayNames = (
  locale: string,
  width: "long" | "short" | "narrow"
): readonly string[] =>
  // 2024-01-01 (UTC) was a Monday.
  WEEKDAYS.map((_, index) =>
    dateTimeFormat(locale, { timeZone: "UTC", weekday: width }).format(
      Date.UTC(2024, 0, 1 + index)
    )
  );

/** "Every 2 weeks on Monday, Wednesday, 10 times" in the user's language. */
export const describeRecurrence = (
  recurrence: Recurrence,
  locale: string,
  words: RecurrenceWords
): string | null => {
  switch (recurrence.kind) {
    case "none": {
      return null;
    }
    case "custom": {
      return words.custom();
    }
    case "simple": {
      const parts = [words.repeat(recurrence.frequency, recurrence.interval)];
      if (recurrence.frequency === "weekly" && recurrence.byDay.length > 0) {
        const names = weekdayNames(locale, "long");
        const list = new Intl.ListFormat(locale, {
          type: "conjunction",
        }).format(
          WEEKDAYS.flatMap((day, index) =>
            recurrence.byDay.includes(day) ? [names[index] ?? day] : []
          )
        );
        parts.push(words.onDays(list));
      }
      const { end } = recurrence;
      if (end.kind === "count") {
        parts.push(words.times(end.count));
      } else if (end.kind === "until") {
        const date = parseLocalDate(end.date);
        parts.push(
          words.until(
            date === null
              ? end.date
              : dateTimeFormat(locale, {
                  dateStyle: "medium",
                  timeZone: "UTC",
                }).format(Date.UTC(date.year, date.month - 1, date.day))
          )
        );
      }
      return parts.join(", ");
    }
    default: {
      return recurrence satisfies never;
    }
  }
};
