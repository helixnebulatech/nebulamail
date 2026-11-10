// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { PartialDate } from "./card";
import { describeDate, intlCalendar, toGregorian } from "./dates";

const date = (
  calendarScale: string | null,
  year: number | null,
  month: number,
  day: number
): PartialDate => ({ calendarScale, day, month, year });

const iso = (value: Date | null): string | null =>
  value?.toISOString().slice(0, 10) ?? null;

const TODAY = new Date("2026-10-26T10:00:00Z");

describe("calendars", () => {
  it("knows the calendars of Intl, and only those", () => {
    expect(intlCalendar("hebrew")).toBe("hebrew");
    expect(intlCalendar("gregorian")).toBe("gregory");
    expect(intlCalendar("islamic-umalqura")).toBe("islamic-umalqura");
    expect(intlCalendar("klingon")).toBeNull();
  });

  it("finds the Gregorian day of a full date", () => {
    // 20 March 2024 is 10 Adar II 5784, 1 Farvardin 1403, 10 Ramadan 1445
    // and the 11th of the 2nd Chinese month.
    const day = "2024-03-20";
    expect(iso(toGregorian(date("hebrew", 5784, 6, 10), TODAY))).toBe(day);
    expect(iso(toGregorian(date("persian", 1403, 1, 1), TODAY))).toBe(day);
    expect(iso(toGregorian(date("islamic-umalqura", 1445, 9, 10), TODAY))).toBe(
      day
    );
    expect(iso(toGregorian(date("chinese", 2024, 2, 11), TODAY))).toBe(day);
    expect(iso(toGregorian(date(null, 1990, 5, 17), TODAY))).toBe("1990-05-17");
  });

  it("finds the next birthday of a date without year", () => {
    // 1 Farvardin (Nowruz) after 26 October 2026: 21 March 2027.
    expect(iso(toGregorian(date("persian", null, 1, 1), TODAY))).toBe(
      "2027-03-21"
    );
    expect(iso(toGregorian(date(null, null, 12, 24), TODAY))).toBe(
      "2026-12-24"
    );
  });

  it("gives up on impossible dates", () => {
    expect(toGregorian(date("hebrew", 5784, 1, 31), TODAY)).toBeNull();
    expect(toGregorian(date("klingon", 1, 1, 1), TODAY)).toBeNull();
  });
});

describe(describeDate, () => {
  it("writes Gregorian dates, with or without year", () => {
    expect(describeDate(date(null, 1990, 5, 17), "en", TODAY)).toStrictEqual({
      kind: "gregorian",
      text: "May 17, 1990",
    });
    expect(describeDate(date(null, null, 2, 29), "es", TODAY)).toStrictEqual({
      kind: "gregorian",
      text: "29 de febrero",
    });
  });

  it("writes other calendars with the Gregorian day", () => {
    expect(
      describeDate(date("hebrew", 5784, 6, 10), "en", TODAY)
    ).toStrictEqual({
      gregorian: "March 20, 2024",
      kind: "converted",
      next: false,
      text: "10 Adar II 5784",
    });
    const yearless = describeDate(date("persian", null, 1, 1), "es", TODAY);
    expect(yearless).toMatchObject({
      gregorian: "21 de marzo de 2027",
      kind: "converted",
      next: true,
    });
  });

  it("shows the numbers of a calendar it doesn't know", () => {
    expect(describeDate(date("klingon", 12, 3, 4), "en", TODAY)).toStrictEqual({
      calendar: "klingon",
      kind: "raw",
      text: "4/3/12",
    });
  });
});
