// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDate } from "@internationalized/date";
import { describe, expect, it } from "vitest";

import {
  dateInZone,
  dateTimeFormat,
  dayRange,
  formatDate,
  formatTime,
  formatDuration,
  formatOffset,
  isValidTimeZone,
  minutesInDay,
  parseDuration,
  durationMinutes,
  timedRange,
  timeZoneLabel,
} from "./time";

const iso = (ms: number): string => new Date(ms).toISOString();

describe(timedRange, () => {
  it("reads the wall time in the event's own zone", () => {
    const range = timedRange(
      {
        duration: "PT1H",
        start: "2026-10-26T10:00:00",
        timeZone: "Europe/Madrid",
      },
      "America/New_York"
    );
    // Madrid is UTC+1 in winter (after 25 October 2026).
    expect(range && iso(range.start)).toBe("2026-10-26T09:00:00.000Z");
    expect(range && iso(range.end)).toBe("2026-10-26T10:00:00.000Z");
  });

  it("puts floating events in the viewer's zone", () => {
    const madrid = timedRange(
      { duration: "PT30M", start: "2026-07-01T09:00:00" },
      "Europe/Madrid"
    );
    const tokyo = timedRange(
      { duration: "PT30M", start: "2026-07-01T09:00:00" },
      "Asia/Tokyo"
    );
    expect(madrid && iso(madrid.start)).toBe("2026-07-01T07:00:00.000Z");
    expect(tokyo && iso(tokyo.start)).toBe("2026-07-01T00:00:00.000Z");
  });

  it("keeps the wall time for day durations across a change of time", () => {
    // Europe/Madrid leaves summer time on 25 October 2026.
    const range = timedRange(
      {
        duration: "P1D",
        start: "2026-10-24T10:00:00",
        timeZone: "Europe/Madrid",
      },
      "UTC"
    );
    expect(range && iso(range.start)).toBe("2026-10-24T08:00:00.000Z");
    expect(range && iso(range.end)).toBe("2026-10-25T09:00:00.000Z");
  });

  it("counts hours exactly across a change of time", () => {
    const range = timedRange(
      {
        duration: "PT24H",
        start: "2026-10-24T10:00:00",
        timeZone: "Europe/Madrid",
      },
      "UTC"
    );
    expect(range && iso(range.end)).toBe("2026-10-25T08:00:00.000Z");
  });

  it("treats an unknown zone as floating and rejects bad starts", () => {
    const range = timedRange(
      {
        duration: "PT1H",
        start: "2026-07-01T09:00:00",
        timeZone: "Mars/Olympus",
      },
      "UTC"
    );
    expect(range && iso(range.start)).toBe("2026-07-01T09:00:00.000Z");
    expect(timedRange({ start: "tomorrow" }, "UTC")).toBeNull();
  });
});

describe(dayRange, () => {
  it("gives an exclusive end, one day by default", () => {
    expect(
      dayRange({ duration: "P3D", start: "2026-12-30T00:00:00" })
    ).toMatchObject({
      end: parseDate("2027-01-02"),
      first: parseDate("2026-12-30"),
    });
    expect(dayRange({ start: "2026-12-30" })?.end.toString()).toBe(
      "2026-12-31"
    );
  });
});

describe("durations", () => {
  it.each([
    [0, 90, "PT1H30M"],
    [2, 0, "P2D"],
    [1, 15, "P1DT15M"],
    [0, 0, "PT0S"],
  ])("writes %i days %i minutes as %s", (days, minutes, text) => {
    expect(formatDuration(days, minutes)).toBe(text);
  });

  it("writes alert offsets", () => {
    expect(formatOffset(-15)).toBe("-PT15M");
    expect(formatOffset(-1440)).toBe("-P1D");
  });

  it("reads signed durations", () => {
    const before = parseDuration("-PT15M");
    expect(before && durationMinutes(before)).toBe(-15);
    const week = parseDuration("P1W");
    expect(week && durationMinutes(week)).toBe(7 * 1440);
    expect(parseDuration("soon")).toBeNull();
  });
});

describe("zones", () => {
  it("knows real IANA zones only", () => {
    expect(isValidTimeZone("Europe/Madrid")).toBeTruthy();
    expect(isValidTimeZone("Not/AZone")).toBeFalsy();
    expect(isValidTimeZone("")).toBeFalsy();
  });

  it("finds the local day and minute of an instant", () => {
    const ms = Date.parse("2026-10-26T23:30:00Z");
    expect(dateInZone(ms, "Europe/Madrid").toString()).toBe("2026-10-27");
    expect(minutesInDay(ms, "Europe/Madrid")).toBe(30);
    expect(dateInZone(ms, "America/Los_Angeles").toString()).toBe("2026-10-26");
  });

  it("labels a zone with its city and offset", () => {
    expect(
      timeZoneLabel(
        "America/New_York",
        "en",
        Date.parse("2026-07-01T12:00:00Z")
      )
    ).toBe("New York (GMT-4)");
  });
});

describe(dateTimeFormat, () => {
  it("reuses one formatter per locale and options", () => {
    const options = { hour: "numeric", timeZone: "UTC" } as const;
    expect(dateTimeFormat("es", options)).toBe(dateTimeFormat("es", options));
    expect(dateTimeFormat("es", options)).not.toBe(
      dateTimeFormat("en", options)
    );
  });

  it("formats exactly like a new formatter", () => {
    const at = Date.UTC(2026, 9, 26, 9, 30);
    expect(formatTime(at, "Europe/Madrid", "en-US")).toBe(
      new Intl.DateTimeFormat("en-US", {
        hour: "numeric",
        minute: "2-digit",
        timeZone: "Europe/Madrid",
      }).format(at)
    );
    expect(
      formatDate(parseDate("2026-10-26"), "es-ES", { dateStyle: "full" })
    ).toBe(
      new Intl.DateTimeFormat("es-ES", {
        dateStyle: "full",
        timeZone: "UTC",
      }).format(Date.UTC(2026, 9, 26))
    );
  });

  it("still refuses an unknown zone, every time", () => {
    expect(isValidTimeZone("Mars/Base")).toBeFalsy();
    expect(isValidTimeZone("Mars/Base")).toBeFalsy();
    expect(isValidTimeZone("Europe/Madrid")).toBeTruthy();
  });
});
