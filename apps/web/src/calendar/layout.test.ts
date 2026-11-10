// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDate } from "@internationalized/date";
import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  agendaDays,
  daysOf,
  occurrencesByWeek,
  occursOn,
  placeBanners,
  placeDay,
  shiftAnchor,
  viewRange,
  weeksOf,
} from "./layout";
import type { Occurrence } from "./model";
import { startOfDayMs } from "./time";

const ZONE = "Europe/Madrid";

const timed = (
  id: string,
  start: string,
  end: string,
  title = id
): Occurrence => ({
  accountId: AccountId.parse("c"),
  allDay: false,
  baseId: CalendarEventId.parse(id),
  calendarId: null,
  calendarKey: null,
  color: "#C0145E",
  end: Date.parse(end),
  endDay: null,
  firstDay: null,
  hasParticipants: false,
  id: CalendarEventId.parse(id),
  isOrganizer: true,
  location: null,
  myStatus: null,
  recurring: false,
  start: Date.parse(start),
  status: "confirmed",
  title,
});

const allDay = (id: string, firstDay: string, endDay: string): Occurrence => ({
  ...timed(id, "2000-01-01T00:00:00Z", "2000-01-01T00:00:00Z"),
  allDay: true,
  end: startOfDayMs(parseDate(endDay), ZONE),
  endDay,
  firstDay,
  start: startOfDayMs(parseDate(firstDay), ZONE),
});

describe(viewRange, () => {
  it("starts weeks on Monday in Spanish and on Sunday in US English", () => {
    const anchor = parseDate("2026-10-28");
    expect(viewRange("week", anchor, "es-ES").start.toString()).toBe(
      "2026-10-26"
    );
    expect(viewRange("week", anchor, "en-US").start.toString()).toBe(
      "2026-10-25"
    );
    expect(viewRange("week", anchor, "en-US", "mon").start.toString()).toBe(
      "2026-10-26"
    );
  });

  it("always shows six full weeks in the month grid", () => {
    const range = viewRange("month", parseDate("2026-02-10"), "es-ES");
    expect(range.start.toString()).toBe("2026-01-26");
    expect(weeksOf(range)).toHaveLength(6);
    expect(daysOf(range)).toHaveLength(42);
  });

  it("moves by the view's step", () => {
    const anchor = parseDate("2026-01-31");
    expect(shiftAnchor("month", anchor, 1).toString()).toBe("2026-02-28");
    expect(shiftAnchor("week", anchor, -1).toString()).toBe("2026-01-24");
    expect(shiftAnchor("day", anchor, 1).toString()).toBe("2026-02-01");
  });
});

describe(placeDay, () => {
  it("puts overlapping events side by side and the rest full width", () => {
    const day = parseDate("2026-10-26");
    const placed = placeDay(
      [
        timed("a", "2026-10-26T08:00:00Z", "2026-10-26T09:00:00Z"),
        timed("b", "2026-10-26T08:30:00Z", "2026-10-26T09:30:00Z"),
        timed("c", "2026-10-26T09:00:00Z", "2026-10-26T10:00:00Z"),
        timed("d", "2026-10-26T12:00:00Z", "2026-10-26T13:00:00Z"),
      ],
      day,
      ZONE
    );
    const byId = Object.fromEntries(
      placed.map((item) => [item.occurrence.id, item])
    );
    expect(byId.a).toMatchObject({
      column: 0,
      columns: 2,
      top: 540,
      bottom: 600,
    });
    expect(byId.b).toMatchObject({ column: 1, columns: 2 });
    // c starts when a ends: it reuses a's column.
    expect(byId.c).toMatchObject({ column: 0, columns: 2 });
    expect(byId.d).toMatchObject({ column: 0, columns: 1 });
  });

  it("clips events that cross midnight and keeps short ones visible", () => {
    const night = timed("n", "2026-10-26T22:00:00Z", "2026-10-27T01:00:00Z");
    const [first] = placeDay([night], parseDate("2026-10-26"), ZONE);
    const [second] = placeDay([night], parseDate("2026-10-27"), ZONE);
    expect(first).toMatchObject({
      bottom: 1440,
      continuesAfter: true,
      top: 1380,
    });
    expect(second).toMatchObject({
      continuesBefore: true,
      top: 0,
      bottom: 120,
    });
    const [tiny] = placeDay(
      [timed("t", "2026-10-26T08:00:00Z", "2026-10-26T08:05:00Z")],
      parseDate("2026-10-26"),
      ZONE
    );
    expect((tiny?.bottom ?? 0) - (tiny?.top ?? 0)).toBe(20);
  });

  it("handles the 25-hour day of a change of time", () => {
    // 25 October 2026: Madrid goes from UTC+2 to UTC+1.
    const late = timed("l", "2026-10-25T21:00:00Z", "2026-10-25T22:00:00Z");
    expect(placeDay([late], parseDate("2026-10-25"), ZONE)[0]).toMatchObject({
      bottom: 1380,
      top: 1320,
    });
  });
});

describe(placeBanners, () => {
  it("packs all-day events into rows across a week", () => {
    const week = daysOf(viewRange("week", parseDate("2026-10-28"), "es-ES"));
    const banners = placeBanners(
      [
        allDay("trip", "2026-10-25", "2026-10-28"),
        allDay("conf", "2026-10-27", "2026-10-30"),
        allDay("party", "2026-10-30", "2026-10-31"),
      ],
      week,
      ZONE
    );
    const byId = Object.fromEntries(
      banners.map((item) => [item.occurrence.id, item])
    );
    expect(byId.trip).toMatchObject({
      continuesBefore: true,
      from: 0,
      row: 0,
      to: 1,
    });
    expect(byId.conf).toMatchObject({ from: 1, row: 1, to: 3 });
    expect(byId.party).toMatchObject({ from: 4, row: 0, to: 4 });
  });
});

describe("days and agenda", () => {
  it("knows which days an event touches", () => {
    const event = allDay("x", "2026-10-26", "2026-10-28");
    expect(occursOn(event, parseDate("2026-10-27"), ZONE)).toBeTruthy();
    expect(occursOn(event, parseDate("2026-10-28"), ZONE)).toBeFalsy();
  });

  it("lists only days with events, in order", () => {
    const days = agendaDays(
      [
        timed("late", "2026-10-27T16:00:00Z", "2026-10-27T17:00:00Z"),
        timed("early", "2026-10-27T07:00:00Z", "2026-10-27T08:00:00Z"),
        allDay("holiday", "2026-10-27", "2026-10-28"),
      ],
      { end: parseDate("2026-11-01"), start: parseDate("2026-10-26") },
      ZONE
    );
    expect(days).toHaveLength(1);
    expect(days[0]?.events.map((event) => event.id)).toStrictEqual([
      "holiday",
      "early",
      "late",
    ]);
  });
});

/** `occursOn` as it was, straight from the definition (the reference). */
const naiveOccursOn = (
  occurrence: Occurrence,
  day: ReturnType<typeof parseDate>,
  zone: string
): boolean => {
  if (occurrence.allDay) {
    return (
      occurrence.firstDay !== null &&
      occurrence.endDay !== null &&
      parseDate(occurrence.firstDay).compare(day) <= 0 &&
      parseDate(occurrence.endDay).compare(day) > 0
    );
  }
  const dayStart = startOfDayMs(day, zone);
  const dayEnd = startOfDayMs(day.add({ days: 1 }), zone);
  return (
    occurrence.start < dayEnd &&
    (occurrence.end > dayStart || occurrence.start === dayStart)
  );
};

describe("layout around changes of time and across weeks", () => {
  // Madrid: 29 March 2026 has 23 hours, 25 October 2026 has 25.
  const tricky = [
    timed("spring", "2026-03-29T00:30:00Z", "2026-03-29T01:30:00Z"),
    timed("autumn", "2026-10-25T00:30:00Z", "2026-10-25T01:30:00Z"),
    // Midnight to midnight across the change: a 25-hour banner.
    timed("long", "2026-10-24T22:00:00Z", "2026-10-25T23:00:00Z"),
    // Zero length, exactly at midnight (Madrid).
    timed("zero", "2026-10-26T23:00:00Z", "2026-10-26T23:00:00Z"),
    timed("weeks", "2026-03-27T10:00:00Z", "2026-04-08T10:00:00Z"),
    allDay("holiday", "2026-03-28", "2026-03-31"),
    allDay("month", "2026-10-01", "2026-11-03"),
    { ...allDay("broken", "2026-10-01", "2026-10-02"), endDay: null },
  ];

  it("places every event on the same days as the plain definition", () => {
    for (const anchor of ["2026-03-15", "2026-10-15", "2026-11-01"]) {
      const range = viewRange("month", parseDate(anchor), "es-ES");
      for (const day of daysOf(range)) {
        for (const event of tricky) {
          expect(
            occursOn(event, day, ZONE),
            `${event.id} on ${day.toString()}`
          ).toBe(naiveOccursOn(event, day, ZONE));
        }
      }
    }
  });

  it("gives each week of the month the events that touch it", () => {
    for (const anchor of ["2026-03-15", "2026-10-15"]) {
      const weeks = weeksOf(viewRange("month", parseDate(anchor), "es-ES"));
      const buckets = occurrencesByWeek(tricky, weeks, ZONE);
      for (const [index, week] of weeks.entries()) {
        const expected = tricky.filter((event) =>
          week.some((day) => naiveOccursOn(event, day, ZONE))
        );
        expect(buckets[index]?.map((event) => event.id)).toStrictEqual(
          expected.map((event) => event.id)
        );
        // And the rows come out the same as with every event.
        expect(
          placeBanners(buckets[index] ?? [], week, ZONE, () => true)
        ).toStrictEqual(placeBanners(tricky, week, ZONE, () => true));
      }
    }
  });

  it("marks banners that go on before and after the week", () => {
    const week = daysOf(viewRange("week", parseDate("2026-04-01"), "es-ES"));
    const weeks = placeBanners(tricky, week, ZONE).find(
      (banner) => banner.occurrence.id === "weeks"
    );
    expect(weeks).toMatchObject({
      continuesAfter: true,
      continuesBefore: true,
      from: 0,
      to: 6,
    });
  });

  it("clips a 25-hour event to each day it touches", () => {
    const day = parseDate("2026-10-25");
    // Not a timed column event (24 h or more: a banner).
    expect(
      placeDay(tricky, day, ZONE).map((p) => p.occurrence.id)
    ).toStrictEqual(["autumn"]);
    const [spring] = placeDay(tricky, parseDate("2026-03-29"), ZONE);
    // 01:30 Madrid (UTC+1) to 03:30 (UTC+2): as tall as it lasts (1 h).
    expect(spring).toMatchObject({ bottom: 150, top: 90 });
  });
});
