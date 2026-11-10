// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Notes with an answer, proposals (COUNTER) and their answers
// (DECLINECOUNTER).

import type { ParsedEvent } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  addressOf,
  MAX_COMMENT,
  analyzeInvitation,
  buildCounter,
  buildDeclineCounter,
  buildOccurrenceRequest,
  buildReply,
  cardAction,
  commentOf,
} from "./itip";

const people = {
  attendee: { email: "alice@nebulamail.test", name: "Alice" },
  organizer: { email: "bob@nebulamail.test", name: "Bob" },
};

describe(buildReply, () => {
  it("adds the note as COMMENT, escaped and folded", () => {
    const ics = buildReply({
      ...people,
      comment: "  Late; sorry,\nsee you  ",
      now: 0,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 1,
      status: "accepted",
      title: "Plan",
      uid: "u",
    });
    expect(ics).toContain("\r\nCOMMENT:Late\\; sorry\\,\\nsee you\r\n");
    expect(ics.indexOf("COMMENT")).toBeGreaterThan(ics.indexOf("SUMMARY"));
  });

  it("leaves out an empty note and cuts a long one", () => {
    const base = {
      ...people,
      now: 0,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 0,
      status: "declined" as const,
      title: "x",
      uid: "u",
    };
    expect(buildReply({ ...base, comment: "   " })).not.toContain("COMMENT");
    const long = buildReply({ ...base, comment: "a".repeat(MAX_COMMENT + 50) });
    expect(long.replaceAll("\r\n ", "")).toContain(
      `COMMENT:${"a".repeat(MAX_COMMENT)}\r\n`
    );
  });

  it("never lets a zone name break out of its line", () => {
    const ics = buildReply({
      ...people,
      now: 0,
      recurrenceId: "2026-11-06T11:00:00",
      recurrenceIdTimeZone:
        "Europe/Madrid\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:evil@x.com",
      sequence: 0,
      status: "declined",
      title: "x",
      uid: "u",
    });
    expect(ics).toContain("\r\nRECURRENCE-ID:20261106T110000\r\n");
    expect(ics).not.toContain("evil@x.com");
  });
});

describe(buildCounter, () => {
  it("proposes UTC times for one occurrence, keeping the answer", () => {
    const ics = buildCounter({
      ...people,
      comment: "Afternoon?",
      end: Date.parse("2026-10-30T16:00:00Z"),
      now: Date.parse("2026-10-26T08:00:00Z"),
      recurrenceId: "2026-10-30T10:00:00",
      recurrenceIdTimeZone: "Etc/UTC",
      sequence: 3,
      start: Date.parse("2026-10-30T15:00:00Z"),
      status: "tentative",
      title: "Planning",
      uid: "u-9@nebulamail.test",
    });
    expect(ics).toBe(
      [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Nebula Mail//EN",
        "METHOD:COUNTER",
        "BEGIN:VEVENT",
        "UID:u-9@nebulamail.test",
        "DTSTAMP:20261026T080000Z",
        "SEQUENCE:3",
        "RECURRENCE-ID:20261030T100000Z",
        'ORGANIZER;CN="Bob":mailto:bob@nebulamail.test',
        'ATTENDEE;PARTSTAT=TENTATIVE;CN="Alice":mailto:alice@nebulamail.test',
        "SUMMARY:Planning",
        "DTSTART:20261030T150000Z",
        "DTEND:20261030T160000Z",
        "COMMENT:Afternoon?",
        "END:VEVENT",
        "END:VCALENDAR",
        "",
      ].join("\r\n")
    );
  });

  it("never ends before it starts", () => {
    const ics = buildCounter({
      ...people,
      end: 0,
      now: 0,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 0,
      start: Date.parse("2026-10-30T15:00:00Z"),
      status: "needs-action",
      title: "x",
      uid: "u",
    });
    expect(ics).toContain("DTEND:20261030T150000Z");
    expect(ics).toContain("PARTSTAT=NEEDS-ACTION");
  });
});

describe(buildOccurrenceRequest, () => {
  it("announces one changed occurrence to every guest", () => {
    const ics = buildOccurrenceRequest({
      end: Date.parse("2026-11-30T11:00:00Z"),
      guests: [
        {
          email: "alice@nebulamail.test",
          name: "Alice",
          optional: false,
          status: "accepted",
        },
        {
          email: "eve@x.com",
          name: null,
          optional: true,
          status: "needs-action",
        },
      ],
      location: "Room; 2",
      now: 0,
      organizer: { email: "bob@nebulamail.test", name: "Bob" },
      recurrenceId: "2026-11-30T10:00:00",
      recurrenceIdTimeZone: "Europe/Madrid",
      sequence: 2,
      start: Date.parse("2026-11-30T10:00:00Z"),
      title: "Weekly",
      uid: "u",
    });
    expect(ics).toContain("METHOD:REQUEST");
    // The TZID with its VTIMEZONE (RFC 5545 §3.2.19), before the VEVENT.
    expect(ics).toMatch(
      /BEGIN:VTIMEZONE\r\nTZID:Europe\/Madrid\r\n[\s\S]*END:VTIMEZONE\r\nBEGIN:VEVENT[\s\S]*\r\nRECURRENCE-ID;TZID=Europe\/Madrid:20261130T100000\r\n/u
    );
    expect(ics).toContain("DTSTART:20261130T100000Z\r\nDTEND:20261130T110000Z");
    expect(ics).toContain("LOCATION:Room\\; 2");
    expect(ics.replaceAll("\r\n ", "")).toContain(
      "ATTENDEE;PARTSTAT=NEEDS-ACTION;ROLE=OPT-PARTICIPANT;RSVP=TRUE:mailto:eve@x.com"
    );
  });

  const allDay = {
    guests: [
      {
        email: "alice@nebulamail.test",
        name: "Alice",
        optional: false,
        status: "accepted" as const,
      },
    ],
    location: null,
    now: 0,
    organizer: { email: "bob@nebulamail.test", name: "Bob" },
    sequence: 1,
    title: "Offsite",
    uid: "u",
  };

  it("announces an all-day occurrence with dates, no zone", () => {
    const ics = buildOccurrenceRequest({
      ...allDay,
      allDay: true,
      endDate: "2026-12-03",
      // Stalwart gives the original day as a local date-time.
      recurrenceId: "2026-11-30T00:00:00",
      // The display zone must not leak into DATE values.
      recurrenceIdTimeZone: "Europe/Madrid",
      startDate: "2026-12-01",
    });
    expect(ics).toContain(
      "RECURRENCE-ID;VALUE=DATE:20261130\r\nDTSTART;VALUE=DATE:20261201\r\nDTEND;VALUE=DATE:20261203\r\n"
    );
    expect(ics).not.toContain("TZID");
    const times = ics
      .split("\r\n")
      .filter((line) => /^(?:DTSTART|DTEND|RECURRENCE-ID)/u.test(line));
    expect(times).toHaveLength(3);
    for (const line of times) {
      expect(line).toMatch(/;VALUE=DATE:\d{8}$/u);
    }
  });

  it("an all-day occurrence lasts at least one day", () => {
    const ics = buildOccurrenceRequest({
      ...allDay,
      allDay: true,
      endDate: "2026-12-01",
      recurrenceId: "2026-11-30",
      recurrenceIdTimeZone: null,
      startDate: "2026-12-01",
    });
    expect(ics).toContain("DTSTART;VALUE=DATE:20261201");
    expect(ics).toContain("DTEND;VALUE=DATE:20261202");
  });

  it("answers an all-day occurrence with a DATE RECURRENCE-ID", () => {
    const ics = buildReply({
      ...people,
      allDay: true,
      now: 0,
      recurrenceId: "2026-11-30T00:00:00",
      recurrenceIdTimeZone: "Europe/Madrid",
      sequence: 0,
      status: "accepted",
      title: "Offsite",
      uid: "u",
    });
    expect(ics).toContain("RECURRENCE-ID;VALUE=DATE:20261130\r\n");
    expect(ics).not.toContain("TZID");
  });

  it("refuses an unreadable all-day date", () => {
    expect(() =>
      buildOccurrenceRequest({
        ...allDay,
        allDay: true,
        endDate: "2026-12-02",
        recurrenceId: "2026-11-30\r\nATTENDEE:mailto:x@evil.test",
        recurrenceIdTimeZone: null,
        startDate: "someday",
      })
    ).toThrow("Invalid all-day occurrence");
  });
});

describe(buildDeclineCounter, () => {
  it("names the guest and the event, nothing else", () => {
    const ics = buildDeclineCounter({
      ...people,
      now: 0,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 4,
      uid: "u",
    });
    expect(ics).toContain("METHOD:DECLINECOUNTER");
    expect(ics).toContain(
      '\r\nATTENDEE;CN="Alice":mailto:alice@nebulamail.test\r\n'
    );
    expect(ics).toContain("SEQUENCE:4");
    expect(ics).not.toContain("SUMMARY");
    expect(ics).not.toContain("DTSTART");
  });
});

/** A COUNTER from Alice as Stalwart parses it (checked live, 2026-10-26). */
const counter: ParsedEvent = {
  "@type": "Event",
  duration: "PT1H",
  iCalendar: {
    name: "vevent",
    properties: [["comment", {}, "unknown", "Can we do the afternoon?"]],
  },
  method: "counter",
  organizerCalendarAddress: "mailto:bob@nebulamail.test",
  participants: {
    a: {
      calendarAddress: "mailto:alice@nebulamail.test",
      name: "Alice",
      participationStatus: "tentative",
    },
    b: {
      calendarAddress: "mailto:bob@nebulamail.test",
      name: "Bob",
      roles: { owner: true },
    },
  },
  recurrenceId: "2026-10-30T05:00:00",
  recurrenceIdTimeZone: "Europe/Madrid",
  sequence: 0,
  start: "2026-10-30T11:00:00",
  timeZone: "America/New_York",
  title: "Planning",
  uid: "u-9@nebulamail.test",
};

const stored = {
  cancelled: false,
  organizer: addressOf(counter.organizerCalendarAddress),
  organizerIsMe: true,
  sequence: 0,
  participants: {
    x: { calendarAddress: "mailto:alice@nebulamail.test" },
    y: {
      calendarAddress: "mailto:bob@nebulamail.test",
      roles: { owner: true },
    },
  },
};

describe("COUNTER cards", () => {
  it("reads the note and the zone recurrenceId is written in", () => {
    const invitation = analyzeInvitation([counter], ["bob@nebulamail.test"]);
    expect(invitation).toMatchObject({
      comment: "Can we do the afternoon?",
      iAmOrganizer: true,
      method: "counter",
      // Stalwart writes recurrenceId in the event's zone (RFC 8984).
      recurrenceIdTimeZone: "America/New_York",
    });
  });

  it("offers accept/decline only to the organizer, for a guest of a live event", () => {
    const mine = analyzeInvitation([counter], ["bob@nebulamail.test"]);
    const theirs = analyzeInvitation([counter], ["alice@nebulamail.test"]);
    if (mine === null || theirs === null) {
      throw new Error("unreadable");
    }
    const sender = {
      email: addressOf(counter.participants?.a?.calendarAddress),
      verdict: null,
    };
    expect(cardAction(mine, stored, sender)).toBe("counter");
    expect(cardAction(mine, null, sender)).toBe("none");
    expect(cardAction(mine, { ...stored, cancelled: true }, sender)).toBe(
      "none"
    );
    // The guest's own copy: the user doesn't organize it.
    expect(
      cardAction(theirs, { ...stored, organizerIsMe: false }, sender)
    ).toBe("none");
    // Someone who isn't a guest can't move the user's event.
    expect(
      cardAction(
        mine,
        { ...stored, participants: { y: stored.participants.y } },
        sender
      )
    ).toBe("none");
  });

  it("finds no note when there is none, and cuts a long one", () => {
    expect(commentOf({ ...counter, iCalendar: undefined })).toBeNull();
    expect(
      // @ts-expect-error -- a malformed iCalendar from another sender.
      commentOf({ ...counter, iCalendar: { properties: "x" } })
    ).toBeNull();
    expect(
      commentOf({
        ...counter,
        iCalendar: {
          properties: [["COMMENT", {}, "text", "b".repeat(MAX_COMMENT * 2)]],
        },
      })
    ).toHaveLength(MAX_COMMENT);
  });
});
