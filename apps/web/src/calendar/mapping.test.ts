// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import {
  AccountId,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { mapContext, toDetail, toOccurrence } from "./mapping";
import type { CalendarSummary } from "./model";

const account = AccountId.parse("c");

const calendar: CalendarSummary = {
  accountId: account,
  color: "#2353C9",
  id: CalendarId.parse("b"),
  isDefault: true,
  key: "c/b",
  mayDelete: false,
  mayWrite: true,
  name: "Personal",
  owner: null,
  sortOrder: 0,
  visible: true,
};

const context = mapContext(
  "Europe/Madrid",
  ["alice@nebulamail.test"],
  [calendar],
  account
);

const id = (value: string) => CalendarEventId.parse(value);

describe(toOccurrence, () => {
  it("uses the server's UTC times and the series' all-day flag", () => {
    // As the bridge returns an expanded occurrence: no showWithoutTime,
    // the query's zone instead of none.
    const occurrence = toOccurrence(
      {
        baseEventId: id("i"),
        calendarIds: { b: true },
        duration: "P2D",
        id: id("ihqsqeaaaaai"),
        start: "2031-05-06T00:00:00",
        timeZone: "Europe/Madrid",
        title: "Holiday",
        utcEnd: "2031-05-07T22:00:00Z",
        utcStart: "2031-05-05T22:00:00Z",
      },
      { id: id("i"), showWithoutTime: true, title: "Holiday" },
      context
    );
    expect(occurrence).toMatchObject({
      allDay: true,
      baseId: "i",
      color: "#2353C9",
      endDay: "2031-05-08",
      firstDay: "2031-05-06",
      recurring: false,
    });
  });

  it("takes a one-off event as its own series (Stalwart 1.0)", () => {
    // Stalwart 1.0 expands an event that doesn't repeat under its real id,
    // with `baseEventId: null`: there is no base to read.
    const occurrence = toOccurrence(
      {
        baseEventId: null,
        calendarIds: { b: true },
        duration: "P1D",
        id: id("i"),
        showWithoutTime: true,
        start: "2031-05-06T00:00:00",
        timeZone: null,
        title: "Holiday",
        utcEnd: "2031-05-06T22:00:00Z",
        utcStart: "2031-05-05T22:00:00Z",
      },
      undefined,
      context
    );
    expect(occurrence).toMatchObject({
      allDay: true,
      baseId: "i",
      firstDay: "2031-05-06",
      id: "i",
      recurring: false,
    });
  });

  it("knows the user's answer and who organizes", () => {
    const occurrence = toOccurrence(
      {
        id: id("e"),
        organizerCalendarAddress: "mailto:bob@nebulamail.test",
        participants: {
          a: {
            calendarAddress: "mailto:alice@nebulamail.test",
            participationStatus: "tentative",
          },
          o: {
            calendarAddress: "mailto:bob@nebulamail.test",
            roles: { owner: true },
          },
        },
        start: "2026-10-30T11:00:00",
        timeZone: "Europe/Madrid",
        title: "Planning",
      },
      undefined,
      context
    );
    expect(occurrence).toMatchObject({
      hasParticipants: true,
      isOrganizer: false,
      myStatus: "tentative",
      start: Date.parse("2026-10-30T10:00:00Z"),
    });
  });

  it("skips events whose start can't be read", () => {
    expect(
      toOccurrence({ id: id("x"), start: "?" }, undefined, context)
    ).toBeNull();
  });

  // A task with a due date, expanded as Stalwart returns it:
  // `start` = the due time, a UTC zone, and `@type` kept.
  const task = {
    "@type": "Task",
    baseEventId: id("t"),
    calendarIds: { b: true },
    id: id("h1qclmaaaagr"),
    start: "2026-11-06T16:30:00",
    timeZone: "Etc/UTC",
    title: "Pay the invoice",
    utcEnd: "2026-11-06T22:59:59Z",
    utcStart: "2026-11-06T16:30:00Z",
  };

  it("keeps Events and objects without a type", () => {
    expect(
      toOccurrence({ ...task, "@type": "Event" }, undefined, context)
    ).not.toBeNull();
    expect(
      toOccurrence({ ...task, "@type": undefined }, undefined, context)
    ).not.toBeNull();
  });

  it("leaves out tasks and anything that isn't an Event", () => {
    expect(toOccurrence(task, undefined, context)).toBeNull();
    // The occurrence without its type, the series with it.
    expect(
      toOccurrence(
        { ...task, "@type": undefined },
        { "@type": "Task", id: id("t") },
        context
      )
    ).toBeNull();
    expect(
      toOccurrence({ ...task, "@type": "Group" }, undefined, context)
    ).toBeNull();
    expect(toDetail(task, undefined, context)).toBeNull();
  });
});

describe(toDetail, () => {
  it("gives the days of an all-day event of several days", () => {
    const detail = toDetail(
      {
        calendarIds: { b: true },
        duration: "P5D",
        id: id("d"),
        showWithoutTime: true,
        start: "2026-11-03T00:00:00",
        timeZone: null,
        title: "Conference",
      },
      undefined,
      context
    );
    expect(detail).toMatchObject({
      allDay: true,
      endDay: "2026-11-08",
      firstDay: "2026-11-03",
    });
  });

  it("reads alerts, links and an HTML description", () => {
    const detail = toDetail(
      {
        alerts: {
          a1: { trigger: { offset: "-PT15M", relativeTo: "start" } },
          a2: { trigger: { when: "2026-10-30T09:00:00Z" } },
        },
        calendarIds: { b: true },
        description: "<b>Agenda</b>",
        descriptionContentType: "text/html",
        duration: "PT1H30M",
        id: id("e"),
        recurrenceRule: { frequency: "weekly" },
        start: "2026-10-30T11:00:00",
        timeZone: "Europe/Madrid",
        title: "Planning",
        uid: "u1",
        virtualLocations: {
          v: { name: "Video", uri: "https://meet.example/x" },
        },
      },
      undefined,
      context
    );
    expect(detail).toMatchObject({
      alerts: [{ key: "a1", minutesBefore: 15 }],
      descriptionIsHtml: true,
      durationMinutes: 90,
      isOccurrence: false,
      links: [{ name: "Video", uri: "https://meet.example/x" }],
      recurring: true,
      timeZone: "Europe/Madrid",
      uid: "u1",
    });
  });
});
