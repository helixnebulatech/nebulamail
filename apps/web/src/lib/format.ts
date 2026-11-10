// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Dates and sizes for the mail UI, with Intl in the user's language. Pure:
// `now` and `locale` are parameters so the output is testable.

const DAY_MS = 86_400_000;

const startOfDay = (date: Date): number =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

/** Whole calendar days between two dates in local time (0 = same day). */
export const calendarDaysBetween = (earlier: Date, later: Date): number =>
  Math.round((startOfDay(later) - startOfDay(earlier)) / DAY_MS);

const capitalize = (text: string, locale: string): string =>
  text.charAt(0).toLocaleUpperCase(locale) + text.slice(1);

// Building an Intl formatter is far slower than using one, and these run
// for every list row: one per locale and fixed set of options, kept here.
// Few locales are ever used; the bound only guards against a runaway.
const MAX_FORMATTERS = 64;
const formatters = new Map<string, Intl.DateTimeFormat>();

const DATE_OPTIONS = {
  clock: { hour: "numeric", minute: "2-digit" },
  full: { dateStyle: "medium", timeStyle: "short" },
  fullThisYear: {
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    month: "short",
  },
  monthDay: { day: "numeric", month: "short" },
  numeric: { day: "numeric", month: "numeric", year: "numeric" },
  weekday: { weekday: "short" },
} as const satisfies Record<string, Intl.DateTimeFormatOptions>;

const dateFormat = (
  locale: string,
  kind: keyof typeof DATE_OPTIONS
): Intl.DateTimeFormat => {
  const key = `${kind}\u0000${locale}`;
  let formatter = formatters.get(key);
  if (formatter === undefined) {
    if (formatters.size >= MAX_FORMATTERS) {
      formatters.clear();
    }
    formatter = new Intl.DateTimeFormat(locale, DATE_OPTIONS[kind]);
    formatters.set(key, formatter);
  }
  return formatter;
};

const yesterdays = new Map<string, string>();

/** "Yesterday" in the locale, capitalized. */
const yesterday = (locale: string): string => {
  let text = yesterdays.get(locale);
  if (text === undefined) {
    if (yesterdays.size >= MAX_FORMATTERS) {
      yesterdays.clear();
    }
    text = capitalize(
      new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(
        -1,
        "day"
      ),
      locale
    );
    yesterdays.set(locale, text);
  }
  return text;
};

/**
 * The short time in a list row, like Gmail and Superhuman: "9:41" today,
 * "Yesterday", the weekday within the last week, "21 Oct" this year,
 * "21/10/2025" before that. Future dates (clock skew) show the time.
 */
export const formatListTime = (
  iso: string,
  now: Date,
  locale: string
): string => {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const days = calendarDaysBetween(date, now);
  if (days <= 0) {
    return dateFormat(locale, "clock").format(date);
  }
  if (days === 1) {
    return yesterday(locale);
  }
  if (days < 7) {
    return capitalize(dateFormat(locale, "weekday").format(date), locale);
  }
  if (date.getFullYear() === now.getFullYear()) {
    return dateFormat(locale, "monthDay").format(date);
  }
  return dateFormat(locale, "numeric").format(date);
};

/**
 * The time to show after "Yesterday" where the list has room ("Yesterday
 * 5:50 PM"): only for yesterday's mail, so same-day order stays visible.
 */
export const formatListClock = (
  iso: string,
  now: Date,
  locale: string
): string | null => {
  const date = new Date(iso);
  return !Number.isNaN(date.getTime()) && calendarDaysBetween(date, now) === 1
    ? dateFormat(locale, "clock").format(date)
    : null;
};

/** Full date and time for the reading pane header and tooltips. */
export const formatFullDate = (iso: string, locale: string): string => {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? ""
    : dateFormat(locale, "full").format(date);
};

/**
 * Date and time for the thread (message headers, folded cards): without the
 * year when it is this year's ("Oct 27, 9:37 PM"), in full otherwise.
 */
export const formatThreadDate = (
  iso: string,
  now: number,
  locale: string
): string => {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return dateFormat(
    locale,
    date.getFullYear() === new Date(now).getFullYear() ? "fullThisYear" : "full"
  ).format(date);
};

/** "9:41", for "Saved at 9:41". */
export const formatClock = (date: Date, locale: string): string =>
  dateFormat(locale, "clock").format(date);

const UNITS = ["byte", "kilobyte", "megabyte", "gigabyte"] as const;

/** "284 KB", "1.2 MB" (decimal units, like most mail clients). */
export const formatBytes = (bytes: number, locale: string): string => {
  if (bytes < 1000) {
    // Intl's short "byte" reads oddly ("13 byte"); B is universal.
    return `${new Intl.NumberFormat(locale).format(Math.max(0, bytes))} B`;
  }
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: value < 10 && unit > 0 ? 1 : 0,
    style: "unit",
    unit: UNITS[unit],
    unitDisplay: "short",
  }).format(value);
};
