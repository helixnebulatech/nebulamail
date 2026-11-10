// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Birthdays in words, also in calendars other than the Gregorian one
// (JSContact `calendarScale`, RFC 9553 §2.8.1: "hebrew", "chinese",
// "islamic-umalqura"…). No date library: the browser's Intl knows these
// calendars. Intl can format a day in any calendar but can't build one
// from its parts, so the matching Gregorian day is found by formatting the
// days around it and comparing day, month and year.

import type { PartialDate } from "./card";

/** RFC 7529 / CLDR names → Intl's (`ca` Unicode extension) names. */
const ALIASES: Readonly<Record<string, string>> = {
  ethioaa: "ethioaa",
  "ethiopic-amete-alem": "ethioaa",
  gregorian: "gregory",
};

/** The Intl name of a calendar, or null if this browser doesn't know it. */
export const intlCalendar = (scale: string): string | null => {
  const wanted = ALIASES[scale] ?? scale;
  try {
    const resolved = new Intl.DateTimeFormat("en", {
      calendar: wanted,
    }).resolvedOptions().calendar;
    // Unknown calendars quietly become "gregory"; "islamic" → "islamic-tbla"…
    return resolved === wanted || resolved.startsWith(`${wanted}-`)
      ? resolved
      : null;
  } catch {
    return null;
  }
};

/**
 * Hebrew months by their English name, numbered like RFC 7529 (Adar is 6;
 * the leap month Adar I has no plain number, so it never matches).
 */
const HEBREW_MONTHS: Readonly<Record<string, number>> = {
  Adar: 6,
  "Adar II": 6,
  Av: 11,
  Elul: 12,
  Heshvan: 2,
  Iyar: 8,
  Kislev: 3,
  Nisan: 7,
  Shevat: 5,
  Sivan: 9,
  Tamuz: 10,
  Tevet: 4,
  Tishri: 1,
};

interface Parts {
  readonly year: number | null;
  readonly month: number | null;
  readonly day: number | null;
}

const partsFormats = new Map<string, Intl.DateTimeFormat>();

const partsFormat = (calendar: string): Intl.DateTimeFormat => {
  let format = partsFormats.get(calendar);
  if (format === undefined) {
    format = new Intl.DateTimeFormat("en", {
      calendar,
      day: "numeric",
      month: "numeric",
      numberingSystem: "latn",
      timeZone: "UTC",
      year: "numeric",
    });
    partsFormats.set(calendar, format);
  }
  return format;
};

/** The leading digits ("6bis", a Chinese leap month, is 6). */
const number = (text: string | undefined): number | null => {
  const digits = /^\d+/u.exec(text ?? "")?.[0];
  return digits === undefined ? null : Number(digits);
};

/** Day, month and year of a (UTC) day in another calendar. */
export const calendarParts = (calendar: string, date: Date): Parts => {
  const parts = partsFormat(calendar).formatToParts(date);
  const value = (type: string): string | undefined =>
    parts.find((part) => part.type === type)?.value;
  const month = value("month");
  return {
    day: number(value("day")),
    // Chinese and Korean years are cycles: Intl gives the related year.
    month:
      calendar === "hebrew"
        ? (HEBREW_MONTHS[month ?? ""] ?? null)
        : number(month),
    year: number(value("relatedYear") ?? value("year")),
  };
};

const DAY = 86_400_000;

const utcDay = (year: number, month: number, day: number): Date => {
  const date = new Date(0);
  date.setUTCFullYear(year, month, day);
  return date;
};

/**
 * The Gregorian day of a date in another calendar: that very day when it
 * has a year; otherwise the next time it comes, from `today` on. Null if
 * the browser lacks the calendar or no day matches.
 */
export const toGregorian = (
  date: PartialDate,
  today: Date = new Date()
): Date | null => {
  const calendar =
    date.calendarScale === null ? "gregory" : intlCalendar(date.calendarScale);
  if (calendar === null) {
    return null;
  }
  const matches = (day: Date, withYear: boolean): boolean => {
    const parts = calendarParts(calendar, day);
    return (
      parts.day === date.day &&
      parts.month === date.month &&
      (!withYear || parts.year === date.year)
    );
  };
  const start = utcDay(
    today.getUTCFullYear(),
    today.getUTCMonth(),
    today.getUTCDate()
  );
  if (date.year === null) {
    // A lunar year can skip a day (the 30th of a short month): two years.
    for (let offset = 0; offset < 800; offset += 1) {
      const day = new Date(start.getTime() + offset * DAY);
      if (matches(day, false)) {
        return day;
      }
    }
    return null;
  }
  // Calendar years don't line up with ours: guess from today, look around.
  const shift =
    (calendarParts(calendar, start).year ?? 0) - start.getUTCFullYear();
  const guess = date.year - shift;
  const from = utcDay(guess - 1, 0, 1).getTime();
  const to = utcDay(guess + 2, 0, 1).getTime();
  for (let time = from; time < to; time += DAY) {
    const day = new Date(time);
    if (matches(day, true)) {
      return day;
    }
  }
  return null;
};

export type DateText =
  /** A Gregorian date ("17 May 1990", "2 March"). */
  | { readonly kind: "gregorian"; readonly text: string }
  /** Another calendar, with the Gregorian day (that one, or the next). */
  | {
      readonly kind: "converted";
      readonly text: string;
      readonly gregorian: string;
      /** Without a year: `gregorian` is the next birthday. */
      readonly next: boolean;
    }
  /** A calendar this browser doesn't know: the numbers and its name. */
  | { readonly kind: "raw"; readonly text: string; readonly calendar: string };

const calendarName = (locale: string, scale: string): string => {
  try {
    return (
      new Intl.DisplayNames(locale, { type: "calendar" }).of(
        intlCalendar(scale) ?? scale
      ) ?? scale
    );
  } catch {
    return scale;
  }
};

/** A birthday in words, in the user's language. */
export const describeDate = (
  date: PartialDate,
  locale: string,
  today: Date = new Date()
): DateText => {
  const long = (day: Date, calendar: string, withYear: boolean): string =>
    new Intl.DateTimeFormat(locale, {
      calendar,
      day: "numeric",
      month: "long",
      timeZone: "UTC",
      ...(withYear ? { year: "numeric" } : {}),
    }).format(day);
  if (date.calendarScale === null) {
    // Without a year, a leap year so that 29 February exists.
    const day = utcDay(date.year ?? 2000, date.month - 1, date.day);
    return {
      kind: "gregorian",
      text: long(day, "gregory", date.year !== null),
    };
  }
  const calendar = intlCalendar(date.calendarScale);
  const day = toGregorian(date, today);
  if (calendar === null || day === null) {
    return {
      calendar: calendarName(locale, date.calendarScale),
      kind: "raw",
      text: [date.day, date.month, date.year]
        .filter((part) => part !== null)
        .join("/"),
    };
  }
  return {
    gregorian: long(day, "gregory", true),
    kind: "converted",
    next: date.year === null,
    text: long(day, calendar, date.year !== null),
  };
};
