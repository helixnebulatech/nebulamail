// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  calendarDaysBetween,
  formatBytes,
  formatFullDate,
  formatListClock,
  formatListTime,
  formatThreadDate,
} from "./format";

// Local-time dates, so the tests don't depend on the machine's time zone.
const now = new Date(2026, 9, 23, 18, 30);
const at = (...args: [number, number, number, number?, number?]) =>
  new Date(
    args[0],
    args[1],
    args[2],
    args[3] ?? 12,
    args[4] ?? 0
  ).toISOString();

describe(calendarDaysBetween, () => {
  it("counts calendar days, not 24-hour periods", () => {
    expect(calendarDaysBetween(new Date(2026, 9, 22, 23, 59), now)).toBe(1);
    expect(calendarDaysBetween(new Date(2026, 9, 23, 0, 1), now)).toBe(0);
  });
});

describe(formatListTime, () => {
  it("shows the time for today", () => {
    expect(formatListTime(at(2026, 9, 23, 9, 41), now, "es")).toBe("9:41");
    expect(formatListTime(at(2026, 9, 23, 9, 41), now, "en")).toBe("9:41 AM");
  });

  it("says yesterday in words, capitalized", () => {
    expect(formatListTime(at(2026, 9, 22), now, "es")).toBe("Ayer");
    expect(formatListTime(at(2026, 9, 22), now, "en")).toBe("Yesterday");
  });

  it("adds the clock to yesterday only", () => {
    expect(formatListClock(at(2026, 9, 22, 17, 50), now, "en")).toBe("5:50 PM");
    expect(formatListClock(at(2026, 9, 22, 17, 50), now, "es")).toBe("17:50");
    expect(formatListClock(at(2026, 9, 23, 9, 41), now, "en")).toBeNull();
    expect(formatListClock(at(2026, 9, 19), now, "en")).toBeNull();
    expect(formatListClock("not a date", now, "en")).toBeNull();
  });

  it("uses the weekday within the last week", () => {
    // 2026-10-19 is a Monday.
    expect(formatListTime(at(2026, 9, 19), now, "en")).toBe("Mon");
    expect(formatListTime(at(2026, 9, 19), now, "es")).toBe("Lun");
  });

  it("uses day and month this year, and the full date before", () => {
    expect(formatListTime(at(2026, 9, 2), now, "en")).toBe("Oct 2");
    expect(formatListTime(at(2026, 9, 2), now, "es")).toBe("2 oct");
    expect(formatListTime(at(2025, 9, 2), now, "es")).toBe("2/10/2025");
  });

  it("treats future dates (clock skew) as today and bad dates as empty", () => {
    expect(formatListTime(at(2026, 9, 24, 8, 5), now, "es")).toBe("8:05");
    expect(formatListTime("not a date", now, "es")).toBe("");
  });

  it("gives the same text as a fresh formatter, whatever the order", () => {
    const dates = [
      at(2026, 9, 24, 8, 5),
      at(2026, 9, 22),
      at(2026, 9, 19),
      at(2026, 9, 2),
      at(2025, 9, 2),
    ];
    for (const locale of ["en", "es", "en", "de", "es"]) {
      for (const date of dates) {
        const text = formatListTime(date, now, locale);
        expect(formatListTime(date, now, locale)).toBe(text);
      }
    }
    expect(formatFullDate(at(2025, 9, 2, 9, 41), "es")).toBe(
      new Intl.DateTimeFormat("es", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(new Date(at(2025, 9, 2, 9, 41)))
    );
    expect(formatFullDate("not a date", "es")).toBe("");
  });
});

describe(formatThreadDate, () => {
  const today = new Date(at(2026, 10, 8, 12, 0)).getTime();

  it("drops the year when it is this year's", () => {
    const text = formatThreadDate(at(2026, 9, 27, 21, 37), today, "en");
    expect(text).not.toContain("2026");
    expect(text).toContain("Oct");
    expect(text).toContain("27");
  });

  it("keeps it for other years", () => {
    expect(formatThreadDate(at(2025, 9, 27, 21, 37), today, "en")).toContain(
      "2025"
    );
  });

  it("is empty for a bad date", () => {
    expect(formatThreadDate("nope", today, "en")).toBe("");
  });
});

describe(formatBytes, () => {
  it("uses short decimal units", () => {
    expect(formatBytes(512, "en")).toBe("512 B");
    expect(formatBytes(13, "es")).toBe("13 B");
    expect(formatBytes(284_000, "en")).toBe("284 kB");
    expect(formatBytes(1_200_000, "en")).toBe("1.2 MB");
    expect(formatBytes(1_200_000, "es")).toBe("1,2 MB");
  });
});
