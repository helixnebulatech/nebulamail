// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDateTime } from "@internationalized/date";
import {
  AccountId,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { EventForm } from "./editing";
import {
  canChangeOneOccurrence,
  eventProperties,
  movedOccurrence,
  movedStart,
  newForm,
  participantsOf,
  shiftLocalStart,
  timingOf,
  validateForm,
  withoutNulls,
} from "./editing";
import type { Occurrence } from "./model";

const calendarId = CalendarId.parse("b");
const me = { email: "alice@nebulamail.test", name: "Alice" };

const base = (): EventForm =>
  newForm(calendarId, parseDateTime("2026-10-26T10:00"), 60, "Europe/Madrid");

describe(timingOf, () => {
  it("measures timed events in exact minutes in their zone", () => {
    expect(timingOf(base())).toStrictEqual({
      duration: "PT1H",
      start: "2026-10-26T10:00:00",
    });
    // Across the end of summer time in Madrid (25 Oct 2026, 03:00 → 02:00).
    expect(
      timingOf({
        ...base(),
        endDate: "2026-10-25",
        endTime: "04:00",
        startDate: "2026-10-25",
        startTime: "01:00",
      })
    ).toMatchObject({ duration: "PT4H" });
  });

  it("stores all-day events as whole days with an inclusive last day", () => {
    expect(
      timingOf({
        ...base(),
        allDay: true,
        endDate: "2026-10-28",
        startDate: "2026-10-26",
      })
    ).toStrictEqual({ duration: "P3D", start: "2026-10-26T00:00:00" });
  });

  it("explains what is wrong", () => {
    expect(timingOf({ ...base(), endTime: "09:00" })).toBe("end-before-start");
    expect(timingOf({ ...base(), startDate: "x" })).toBe("invalid-start");
    expect(validateForm({ ...base(), calendarId: null })).toBe("no-calendar");
    expect(
      validateForm({
        ...base(),
        guests: [{ email: "nope", name: null, optional: false }],
      })
    ).toBe("invalid-guest");
  });
});

describe(participantsOf, () => {
  it("makes the user the organizer and keeps known guests' answers", () => {
    const participants = participantsOf(
      [
        { email: "Bob@NebulaMail.test", name: "Bob", optional: false },
        { email: "carol@x.com", name: null, optional: true },
        { email: "alice@nebulamail.test", name: "Alice", optional: false },
      ],
      {
        kept: {
          calendarAddress: "mailto:bob@nebulamail.test",
          participationStatus: "accepted",
        },
        gone: { calendarAddress: "mailto:dan@x.com" },
      },
      me
    );
    const entries = Object.entries(participants ?? {});
    expect(entries).toHaveLength(3);
    expect(participants?.kept).toMatchObject({
      calendarAddress: "mailto:bob@nebulamail.test",
      participationStatus: "accepted",
      roles: { attendee: true },
    });
    const organizer = entries.find(
      ([, item]) => item.roles?.owner === true
    )?.[1];
    expect(organizer).toMatchObject({
      calendarAddress: "mailto:alice@nebulamail.test",
      participationStatus: "accepted",
    });
    const carol = entries.find(
      ([, item]) => item.calendarAddress === "mailto:carol@x.com"
    )?.[1];
    expect(carol).toMatchObject({
      expectReply: true,
      participationStatus: "needs-action",
      roles: { optional: true },
    });
  });

  it("has no participants without guests", () => {
    expect(participantsOf([], null, me)).toBeNull();
  });
});

describe(eventProperties, () => {
  it("builds a JSCalendar event the server accepts", () => {
    const properties = withoutNulls(
      eventProperties(
        {
          ...base(),
          alerts: [10, 10, 60],
          description: "  Bring slides  ",
          location: "Room 1",
          title: " Review ",
        },
        { me }
      )
    );
    expect(properties).toStrictEqual({
      alerts: {
        a1: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-PT10M",
            relativeTo: "start",
          },
        },
        a2: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-PT1H",
            relativeTo: "start",
          },
        },
      },
      calendarIds: { b: true },
      description: "Bring slides",
      duration: "PT1H",
      locations: { l1: { "@type": "Location", name: "Room 1" } },
      showWithoutTime: false,
      start: "2026-10-26T10:00:00",
      timeZone: "Europe/Madrid",
      title: "Review",
    });
  });

  it("leaves an HTML description alone and skips series fields on occurrences", () => {
    const properties = eventProperties(
      { ...base(), keepHtmlDescription: true },
      { me, occurrence: true }
    );
    expect(properties).not.toHaveProperty("description");
    expect(properties).not.toHaveProperty("calendarIds");
    expect(properties).not.toHaveProperty("recurrenceRule");
    expect(properties).not.toHaveProperty("timeZone");
  });
});

describe(canChangeOneOccurrence, () => {
  it("timed occurrences and days of all-day series alike", () => {
    const timed = { allDay: false, isOccurrence: true };
    const day = { allDay: true, isOccurrence: true };
    expect(canChangeOneOccurrence(timed, false)).toBeTruthy();
    expect(canChangeOneOccurrence(day, true)).toBeTruthy();
    expect(
      canChangeOneOccurrence({ ...timed, isOccurrence: false }, false)
    ).toBeFalsy();
  });

  it("never from all-day to timed or back on its own", () => {
    expect(
      canChangeOneOccurrence({ allDay: false, isOccurrence: true }, true)
    ).toBeFalsy();
    expect(
      canChangeOneOccurrence({ allDay: true, isOccurrence: true }, false)
    ).toBeFalsy();
  });
});

describe("moving", () => {
  it("shifts a series by the move of one occurrence", () => {
    expect(
      shiftLocalStart(
        "2026-10-05T10:00:00",
        "2026-10-26T10:00:00",
        "2026-10-27T11:30:00"
      )
    ).toBe("2026-10-06T11:30:00");
    expect(movedStart("2026-10-26T23:45:00", 0, 30)).toBe(
      "2026-10-27T00:15:00"
    );
  });
});

const offset = (value: string, relativeTo = "start") => ({
  "@type": "Alert",
  action: "display",
  trigger: { "@type": "OffsetTrigger", offset: value, relativeTo },
});

describe("saving keeps what the form doesn't show", () => {
  const stored = {
    a1: { ...offset("-PT10M"), acknowledged: "2026-10-26T08:50:00Z" },
    e1: offset("-PT5M", "end"),
    snz1: {
      "@type": "Alert",
      action: "display",
      relatedTo: { a1: { "@type": "Relation", relation: { parent: true } } },
      trigger: { "@type": "AbsoluteTrigger", when: "2026-10-26T09:05:00Z" },
    },
  };
  const where = { start: "2026-10-26T10:00:00", timeZone: "Europe/Madrid" };
  const save = (form: Partial<EventForm>, previousStart = where) =>
    eventProperties(
      { ...base(), ...form },
      { me, previousAlerts: stored, previousStart }
    ).alerts;

  it("keeps snoozes, end alerts and acknowledged when only the title changes", () => {
    expect(save({ alerts: [10], title: "Renamed" })).toStrictEqual(stored);
  });

  it("adds a new alert under a free key and keeps the rest", () => {
    const alerts = save({ alerts: [10, 30] });
    expect(alerts).toMatchObject({
      a1: stored.a1,
      a2: offset("-PT30M"),
      e1: stored.e1,
      snz1: stored.snz1,
    });
  });

  it("drops a snooze whose alert is removed", () => {
    expect(save({ alerts: [] })).toStrictEqual({ e1: stored.e1 });
  });

  it("drops the snoozes of an event that moves", () => {
    expect(save({ alerts: [10], startTime: "11:00" })).toStrictEqual({
      a1: stored.a1,
      e1: stored.e1,
    });
    expect(
      save({ alerts: [10] }, { ...where, timeZone: "Europe/London" })
    ).not.toHaveProperty("snz1");
  });

  it("changes only the location the form shows", () => {
    const previousLocations = {
      l1: { "@type": "Location", description: "2nd floor", name: "Room 1" },
      l2: { "@type": "Location", name: "Lobby" },
    };
    const locations = (location: string) =>
      eventProperties({ ...base(), location }, { me, previousLocations })
        .locations;
    expect(locations("Room 1")).toStrictEqual(previousLocations);
    expect(locations("Room 2")).toStrictEqual({
      l1: { ...previousLocations.l1, name: "Room 2" },
      l2: previousLocations.l2,
    });
    expect(locations("")).toStrictEqual({ l2: previousLocations.l2 });
  });

  it("keeps participants without an address", () => {
    const room = { kind: "location", name: "Room 1" };
    expect(participantsOf([], { r: room }, me)).toStrictEqual({ r: room });
    const properties = eventProperties(base(), {
      me,
      previousParticipants: { r: room },
    });
    expect(properties.participants).toStrictEqual({ r: room });
    // A room alone doesn't make the user an organizer.
    expect(properties.organizerCalendarAddress).toBeNull();
    const withGuest = participantsOf(
      [{ email: "guest@example.com", name: null, optional: false }],
      { r: room },
      me
    );
    expect(withGuest?.r).toStrictEqual(room);
    expect(Object.keys(withGuest ?? {})).toHaveLength(3);
  });
});

describe(movedOccurrence, () => {
  const zone = "Europe/Madrid";
  const occurrence: Occurrence = {
    accountId: AccountId.parse("c"),
    allDay: false,
    baseId: CalendarEventId.parse("e"),
    calendarId: null,
    calendarKey: null,
    color: "#C0145E",
    end: Date.parse("2026-10-24T09:00:00Z"),
    endDay: null,
    firstDay: null,
    hasParticipants: false,
    id: CalendarEventId.parse("e"),
    isOrganizer: true,
    location: null,
    myStatus: null,
    recurring: false,
    start: Date.parse("2026-10-24T08:00:00Z"),
    status: "confirmed",
    title: "e",
  };

  it("keeps the wall time across a change of time", () => {
    // 10:00 Madrid on Saturday (UTC+2) → 10:15 on Monday (UTC+1).
    const moved = movedOccurrence(
      occurrence,
      { days: 2, minutes: 15, resize: 0 },
      zone
    );
    expect(moved?.start).toBe(Date.parse("2026-10-26T09:15:00Z"));
    expect(moved?.end).toBe(Date.parse("2026-10-26T10:15:00Z"));
  });

  it("resizes like the patch, never under 15 minutes", () => {
    expect(
      movedOccurrence(occurrence, { days: 0, minutes: 0, resize: 30 }, zone)
        ?.end
    ).toBe(Date.parse("2026-10-24T09:30:00Z"));
    expect(
      movedOccurrence(occurrence, { days: 0, minutes: 0, resize: -60 }, zone)
        ?.end
    ).toBe(Date.parse("2026-10-24T08:15:00Z"));
  });

  it("moves all-day events by days and leaves the rest to the server", () => {
    const day = {
      ...occurrence,
      allDay: true,
      endDay: "2026-10-26",
      firstDay: "2026-10-24",
    };
    expect(
      movedOccurrence(day, { days: 1, minutes: 0, resize: 0 }, zone)
    ).toMatchObject({
      end: Date.parse("2026-10-26T23:00:00Z"),
      endDay: "2026-10-27",
      firstDay: "2026-10-25",
      start: Date.parse("2026-10-24T22:00:00Z"),
    });
    expect(
      movedOccurrence(day, { days: 0, minutes: 30, resize: 0 }, zone)
    ).toBeNull();
  });
});
