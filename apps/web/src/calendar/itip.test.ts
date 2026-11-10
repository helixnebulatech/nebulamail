// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import type { ParsedEvent } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";
import { z } from "zod";

import type { ICalTime } from "./ical-series";
import { rruleOf, timeLine, vtimezoneLines } from "./ical-series";
import type { ItipSeries } from "./itip";
import {
  addressOf,
  analyzeInvitation,
  buildReply,
  buildTaskReply,
  buildTaskRequest,
  seriesWritable,
  cardAction,
  escapeText,
  eventToImport,
  foldLine,
  invitationSender,
  isNewerResponse,
  replyAuthor,
  responseStamp,
  responseStampOf,
  senderBlocked,
  withStoredTiming,
} from "./itip";

/** As Stalwart parses an invitation from bob (checked live). */
const request: ParsedEvent = {
  "@type": "Event",
  alerts: { a: { action: "display", trigger: { offset: "-PT5M" } } },
  duration: "PT30M",
  links: { l: { href: "https://evil.example/track" } },
  method: "request",
  organizerCalendarAddress: "mailto:bob@nebulamail.test",
  participants: {
    a: {
      calendarAddress: "mailto:Alice@NebulaMail.test",
      expectReply: true,
      name: "Alice",
      participationStatus: "needs-action",
    },
    o: {
      calendarAddress: "mailto:bob@nebulamail.test",
      name: "Bob",
      participationStatus: "accepted",
      roles: { attendee: true, owner: true },
    },
  },
  sequence: 2,
  start: "2026-10-30T11:00:00",
  timeZone: "Europe/Madrid",
  title: "Planning",
  uid: "u-1@nebulamail.test",
};

const own = ["alice@nebulamail.test"];

describe(addressOf, () => {
  it("reads mailto URIs and rejects anything else", () => {
    expect(addressOf("mailto:Ana%40x.com")).toBe("ana@x.com");
    expect(addressOf("MAILTO:ana@x.com")).toBe("ana@x.com");
    expect(addressOf("https://x.com")).toBeNull();
    expect(addressOf("mailto:ana@x.com?subject=hi there")).toBeNull();
    expect(addressOf(null)).toBeNull();
  });
});

describe(analyzeInvitation, () => {
  it("finds the organizer, the attendees and the user", () => {
    const invitation = analyzeInvitation([request], own);
    expect(invitation).toMatchObject({
      iAmOrganizer: false,
      me: { email: "alice@nebulamail.test", key: "a", status: "needs-action" },
      method: "request",
      organizer: { email: "bob@nebulamail.test", name: "Bob" },
      sequence: 2,
      uid: "u-1@nebulamail.test",
    });
    expect(invitation?.attendees).toHaveLength(2);
  });

  it("prefers the series over an override and needs a UID", () => {
    const override = {
      ...request,
      recurrenceId: "2026-11-06T11:00:00",
      title: "Moved",
    };
    expect(analyzeInvitation([override, request], own)?.event.title).toBe(
      "Planning"
    );
    expect(analyzeInvitation([{ ...request, uid: "" }], own)).toBeNull();
    expect(analyzeInvitation([], own)).toBeNull();
  });
});

const OverridesSchema = z.record(z.string(), z.record(z.string(), z.unknown()));

const bob = addressOf(request.organizerCalendarAddress) ?? "";
const alice = addressOf(request.participants?.a?.calendarAddress) ?? "";
const fromBob = { email: bob, verdict: null };
const fromAlice = { email: alice, verdict: null };

/** Bob's event as stored in the user's (Alice's) calendar. */
const aliceCopy = {
  cancelled: false,
  organizer: bob,
  organizerIsMe: false,
  participants: null,
  sequence: 2,
};

describe(cardAction, () => {
  const invitation = analyzeInvitation([request], own);
  if (invitation === null) {
    throw new Error("fixture");
  }

  it("asks the invited user to answer, never adding by itself", () => {
    expect(cardAction(invitation, null, fromBob)).toBe("rsvp");
    expect(cardAction({ ...invitation, me: null }, null, fromBob)).toBe("add");
  });

  it("offers to remove a cancelled event only if it's in the calendar", () => {
    const cancel = {
      ...invitation,
      cancelled: true,
      method: "cancel" as const,
    };
    expect(cardAction(cancel, null, fromBob)).toBe("none");
    expect(cardAction(cancel, { ...aliceCopy, cancelled: true }, fromBob)).toBe(
      "remove"
    );
  });

  it("acts on an event in the calendar only for its stored organizer", () => {
    const cancel = {
      ...invitation,
      cancelled: true,
      method: "cancel" as const,
    };
    const mallory = { email: "mallory@evil.example", verdict: null };
    expect(cardAction(cancel, aliceCopy, mallory)).toBe("none");
    expect(senderBlocked(cancel, aliceCopy, mallory)).toBeTruthy();
    // An update of the stored event, too.
    expect(cardAction(invitation, aliceCopy, mallory)).toBe("none");
    expect(cardAction(invitation, aliceCopy, fromBob)).toBe("rsvp");
  });

  it("never trusts SENT-BY, and compares addresses, not spellings", () => {
    const cancel = {
      ...invitation,
      cancelled: true,
      method: "cancel" as const,
    };
    const mallory = { email: "mallory@evil.example", verdict: null };
    // ORGANIZER;SENT-BY in the .ics is the sender's word: not enough.
    const sentBy = analyzeInvitation(
      [
        {
          ...request,
          method: "cancel",
          participants: {
            ...request.participants,
            o: {
              ...request.participants?.o,
              sentBy: "mailto:mallory@evil.example",
            },
          },
        },
      ],
      own
    );
    if (sentBy === null) {
      throw new Error("fixture");
    }
    expect(cardAction(sentBy, aliceCopy, mallory)).toBe("none");
    // The stored address may be written MAILTO: and %40: compared as addresses.
    expect(
      cardAction(cancel, aliceCopy, {
        email: addressOf(`MAILTO:${bob.replace("@", "%40").toUpperCase()}`),
        verdict: null,
      })
    ).toBe("remove");
  });

  it("does nothing for a sender the server refused or several senders", () => {
    expect(cardAction(invitation, null, { email: bob, verdict: "fail" })).toBe(
      "none"
    );
    expect(
      cardAction(
        invitation,
        null,
        invitationSender([{ email: bob }, { email: alice }], null)
      )
    ).toBe("none");
    expect(
      invitationSender([{ email: "Bob@Example.COM" }], "pass")
    ).toStrictEqual({ email: "bob@example.com", verdict: "pass" });
  });

  it("never URI-decodes the From (it would dodge DMARC)", () => {
    // `boss%40corp.com` is the attacker's own address, not the boss's.
    for (const email of [
      "boss%40corp.com",
      ["boss", "corp%2ecom"].join("@"),
      ["boss%2B1", "corp.com"].join("@"),
      // The Kelvin sign lower-cases into a plain "k".
      ["boss", "\u212Aorp.com"].join("@"),
    ]) {
      expect(invitationSender([{ email }], null).email).toBeNull();
    }
  });

  it("changes nothing from an email older than the calendar", () => {
    const cancel = {
      ...invitation,
      cancelled: true,
      method: "cancel" as const,
    };
    expect(cardAction(cancel, { ...aliceCopy, sequence: 3 }, fromBob)).toBe(
      "none"
    );
  });

  it("takes who organizes from the stored copy, not the .ics", () => {
    // The .ics names the user as organizer, the stored event says Bob.
    const forged = analyzeInvitation(
      [
        {
          ...request,
          method: "reply",
          organizerCalendarAddress: `mailto:${alice}`,
        },
      ],
      own
    );
    if (forged === null) {
      throw new Error("fixture");
    }
    expect(forged.iAmOrganizer).toBeTruthy();
    expect(cardAction(forged, aliceCopy, fromBob)).toBe("none");
  });

  describe("replies to the user's event", () => {
    // Bob organizes; Alice and Carol are his guests.
    const carol = "carol@example.com";
    const reply = analyzeInvitation(
      [
        {
          ...request,
          method: "reply",
          participants: {
            a: {
              calendarAddress: `mailto:${alice}`,
              participationStatus: "accepted",
            },
            c: {
              calendarAddress: `mailto:${carol}`,
              participationStatus: "declined",
            },
          },
        },
      ],
      [bob]
    );
    if (reply === null) {
      throw new Error("fixture");
    }
    const bobsEvent = {
      cancelled: false,
      organizer: bob,
      organizerIsMe: true,
      participants: {
        x: {
          calendarAddress: `mailto:${alice}`,
          participationStatus: "needs-action",
        },
        y: {
          calendarAddress: `mailto:${carol}`,
          participationStatus: "accepted",
        },
      },
      sequence: 2,
    };

    it("applies only the sender's answer", () => {
      expect(cardAction(reply, bobsEvent, fromAlice)).toBe("apply-reply");
      expect(replyAuthor(reply, bobsEvent, fromAlice)?.email).toBe(alice);
      expect(
        cardAction(
          reply,
          {
            ...bobsEvent,
            participants: {
              ...bobsEvent.participants,
              x: {
                ...bobsEvent.participants.x,
                participationStatus: "accepted",
              },
            },
          },
          fromAlice
        )
      ).toBe("none");
    });

    it("ignores a reply from someone who isn't among its attendees", () => {
      const mallory = { email: "mallory@evil.example", verdict: null };
      expect(cardAction(reply, bobsEvent, mallory)).toBe("none");
      expect(replyAuthor(reply, bobsEvent, mallory)).toBeNull();
      expect(
        cardAction(reply, bobsEvent, { ...fromAlice, verdict: "fail" })
      ).toBe("none");
    });

    it("lets no one propose a time for a guest they copied", () => {
      const counter = { ...reply, method: "counter" as const };
      expect(cardAction(counter, bobsEvent, fromAlice)).toBe("counter");
      expect(
        cardAction(counter, bobsEvent, {
          email: "mallory@evil.example",
          verdict: null,
        })
      ).toBe("none");
    });

    // RFC 5546 §2.1.5: a higher SEQUENCE wins, then the later DTSTAMP;
    // the organizer keeps both of the last response it applied.
    const stamped = (sequence: number, updated: string | null) =>
      ({ ...reply, event: { ...reply.event, sequence, updated } }) as const;
    const withLast = (sequence: number, updated: string) => ({
      ...bobsEvent,
      participants: {
        ...bobsEvent.participants,
        x: {
          ...bobsEvent.participants.x,
          participationStatus: "declined",
          scheduleSequence: sequence,
          scheduleUpdated: updated,
        },
      },
    });

    // [sequence, DTSTAMP] of the reply → what the card offers, after one
    // at SEQUENCE 2, 10:00 was applied.
    it.each([
      // Same SEQUENCE: the later DTSTAMP wins.
      [2, "2026-11-01T09:00:00Z", "none"],
      [2, "2026-11-01T10:00:00Z", "none"],
      [2, "2026-11-01T11:00:00Z", "apply-reply"],
      // A lower SEQUENCE never does, a higher one always.
      [1, "2026-11-02T00:00:00Z", "none"],
      [3, "2026-10-01T00:00:00Z", "apply-reply"],
      // No DTSTAMP can't be shown to be newer at the same SEQUENCE.
      [2, null, "none"],
    ] as const)(
      "never applies a reply older than the last one applied (%j, %j)",
      (sequence, updated, action) => {
        const last = withLast(2, "2026-11-01T10:00:00Z");
        expect(cardAction(stamped(sequence, updated), last, fromAlice)).toBe(
          action
        );
      }
    );

    it("applies any reply when none was applied yet", () => {
      expect(cardAction(stamped(0, null), bobsEvent, fromAlice)).toBe(
        "apply-reply"
      );
    });

    it("stores the stamp of the reply it applies", () => {
      expect(
        responseStampOf(
          responseStamp({ sequence: 3, updated: "2026-11-01T10:11:12Z" })
        )
      ).toStrictEqual({
        scheduleSequence: 3,
        scheduleUpdated: "2026-11-01T10:11:12Z",
      });
      // An unreadable DTSTAMP isn't stored.
      expect(
        responseStampOf(responseStamp({ sequence: 1, updated: "yesterday" }))
      ).toStrictEqual({ scheduleSequence: 1 });
      expect(
        isNewerResponse(null, { sequence: 0, updated: null })
      ).toBeTruthy();
    });
  });
});

const occurrenceReply = (recurrenceIdTimeZone: string | null): string =>
  buildReply({
    attendee: { email: "a@example.org", name: null },
    now: 0,
    organizer: { email: "o@example.org", name: null },
    recurrenceId: "2026-11-06T11:00:00",
    recurrenceIdTimeZone,
    sequence: 0,
    status: "accepted",
    title: "x",
    uid: "u",
  });

describe(buildReply, () => {
  it("writes an RFC 5546 REPLY with only the user as attendee", () => {
    const ics = buildReply({
      attendee: { email: "alice@nebulamail.test", name: 'Alice, "A"' },
      now: Date.parse("2026-10-24T08:00:00Z"),
      organizer: { email: "bob@nebulamail.test", name: "Bob" },
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 2,
      status: "tentative",
      title: "Plan; review, 2\nrounds",
      uid: "u-1@nebulamail.test",
    });
    expect(ics).toBe(
      [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Nebula Mail//EN",
        "METHOD:REPLY",
        "BEGIN:VEVENT",
        "UID:u-1@nebulamail.test",
        "DTSTAMP:20261024T080000Z",
        "SEQUENCE:2",
        'ORGANIZER;CN="Bob":mailto:bob@nebulamail.test',
        'ATTENDEE;PARTSTAT=TENTATIVE;CN="Alice, A":mailto:alice@nebulamail.test',
        "SUMMARY:Plan\\; review\\, 2\\nrounds",
        "END:VEVENT",
        "END:VCALENDAR",
        "",
      ].join("\r\n")
    );
  });

  it("names one occurrence with RECURRENCE-ID in its zone", () => {
    const ics = buildReply({
      attendee: { email: "a@x.com", name: null },
      now: 0,
      organizer: { email: "b@x.com", name: null },
      recurrenceId: "2026-11-06T11:00:00",
      recurrenceIdTimeZone: "Europe/Madrid",
      sequence: 0,
      status: "declined",
      title: "x",
      uid: "u",
    });
    expect(ics).toContain(
      "\r\nRECURRENCE-ID;TZID=Europe/Madrid:20261106T110000\r\n"
    );
    // RFC 5545 §3.2.19: the TZID's VTIMEZONE, before the VEVENT.
    expect(ics).toContain(
      "METHOD:REPLY\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\n"
    );
    expect(ics.indexOf("END:VTIMEZONE")).toBeLessThan(
      ics.indexOf("BEGIN:VEVENT")
    );
    expect(ics).toContain("ATTENDEE;PARTSTAT=DECLINED:mailto:a@x.com");
  });

  it("needs no VTIMEZONE for an occurrence in UTC or floating", () => {
    expect(occurrenceReply("Etc/UTC")).toContain(
      "\r\nRECURRENCE-ID:20261106T110000Z\r\n"
    );
    expect(occurrenceReply(null)).toContain(
      "\r\nRECURRENCE-ID:20261106T110000\r\n"
    );
    expect(occurrenceReply("Etc/UTC")).not.toContain("VTIMEZONE");
    expect(occurrenceReply(null)).not.toContain("VTIMEZONE");
  });

  it("never writes a TZID without its VTIMEZONE", () => {
    expect(() => occurrenceReply("Mars/Olympus_Mons")).toThrow(/time zone/u);
    const task = buildTaskReply({
      attendee: { email: "a@example.org", name: null },
      now: 0,
      organizer: { email: "o@example.org", name: null },
      partstat: "COMPLETED",
      recurrenceId: "2026-11-06T11:00:00",
      recurrenceIdTimeZone: "America/New_York",
      sequence: 0,
      title: "x",
      uid: "u",
    });
    expect(task).toContain(
      "RECURRENCE-ID;TZID=America/New_York:20261106T110000"
    );
    expect(task).toContain("BEGIN:VTIMEZONE\r\nTZID:America/New_York\r\n");
  });

  it("folds long lines at 75 octets without splitting characters", () => {
    const line = `SUMMARY:${"ñ".repeat(60)}`;
    const folded = foldLine(line);
    const parts = folded.split("\r\n ");
    expect(parts.length).toBeGreaterThan(1);
    for (const part of parts) {
      expect(new TextEncoder().encode(part).length).toBeLessThanOrEqual(75);
    }
    expect(parts.join("")).toBe(line);
    expect(escapeText("a\\b")).toBe("a\\\\b");
  });
});

const noPrefixes = (patch: Record<string, unknown>): boolean =>
  Object.keys(patch).every((key) =>
    Object.keys(patch).every((other) => !other.startsWith(`${key}/`))
  );

describe(eventToImport, () => {
  it("keeps the schedule, drops the sender's alerts and links, sets the answer", () => {
    const invitation = analyzeInvitation([request], own);
    const copy = eventToImport(request, invitation?.me ?? null, "accepted");
    expect(copy).not.toHaveProperty("alerts");
    expect(copy).not.toHaveProperty("links");
    expect(copy).not.toHaveProperty("method");
    expect(copy).toMatchObject({
      participants: { a: { participationStatus: "accepted" } },
      sequence: 2,
      uid: "u-1@nebulamail.test",
    });
  });
  describe("recurrence overrides", () => {
    const invitation = analyzeInvitation([request], own);
    const me = invitation?.me ?? null;
    const imported = (overrides: Record<string, Record<string, unknown>>) =>
      OverridesSchema.optional().parse(
        eventToImport(
          { ...request, recurrenceOverrides: overrides },
          me,
          "accepted"
        ).recurrenceOverrides
      );

    it("keeps only scheduling data in an override", () => {
      const overrides = imported({
        "2026-11-10T10:00:00": {
          alerts: { a1: { trigger: { offset: "-PT1M" } } },
          "alerts/a1/trigger": { offset: "-PT1M" },
          links: { l1: { href: "https://evil.example/" } },
          "links/l1/href": "https://evil.example/",
          recurrenceRule: { frequency: "daily" },
          title: "X",
          uid: "other",
        },
        "2026-11-17T10:00:00": { alerts: { a1: {} } },
      });
      expect(overrides).toStrictEqual({
        "2026-11-10T10:00:00": {
          "participants/a/participationStatus": "accepted",
          title: "X",
        },
        // Only alerts: nothing to patch, but the occurrence stays (it may
        // be an added one, RFC 8984 §4.3.5).
        "2026-11-17T10:00:00": {},
      });
      // Not a LocalDateTime: not a recurrence id.
      expect(imported({ x: { "alerts/a1": {} } })).toBeUndefined();
    });

    it("keeps an added occurrence (an empty patch, like an RDATE)", () => {
      expect(
        imported({
          "2026-11-12T10:00:00": {},
          "2026-11-19T10:00:00": { excluded: true },
        })
      ).toStrictEqual({
        "2026-11-12T10:00:00": {},
        "2026-11-19T10:00:00": { excluded: true },
      });
    });

    it("ignores privacy in an override (RFC 8984 §4.3.5)", () => {
      expect(
        imported({
          "2026-11-10T10:00:00": { privacy: "public", title: "X" },
        })?.["2026-11-10T10:00:00"]
      ).not.toHaveProperty("privacy");
    });

    it("writes the user's answer without a path under another", () => {
      const overrides = imported({
        "2026-11-01T10:00:00": {
          participants: {
            a: {
              calendarAddress: `mailto:${alice}`,
              participationStatus: "needs-action",
            },
            o: { calendarAddress: `mailto:${bob}` },
          },
        },
        "2026-11-03T10:00:00": { title: "Y" },
        "2026-11-02T10:00:00": {
          "participants/a": { participationStatus: "needs-action" },
        },
        "2026-11-04T10:00:00": { excluded: true },
      });
      expect(overrides?.["2026-11-01T10:00:00"]).toMatchObject({
        participants: { a: { participationStatus: "accepted" } },
      });
      expect(overrides?.["2026-11-02T10:00:00"]).toStrictEqual({
        "participants/a": { participationStatus: "accepted" },
      });
      expect(overrides?.["2026-11-03T10:00:00"]).toStrictEqual({
        "participants/a/participationStatus": "accepted",
        title: "Y",
      });
      expect(overrides?.["2026-11-04T10:00:00"]).toStrictEqual({
        excluded: true,
      });
      for (const patch of Object.values(overrides ?? {})) {
        expect(noPrefixes(patch)).toBeTruthy();
      }
    });
  });
});

describe(withStoredTiming, () => {
  // Our own REPLY, as the bridge parses it: no DTSTART (RFC 5546 allows it).
  const reply: ParsedEvent = {
    "@type": "Event",
    method: "reply",
    organizerCalendarAddress: "mailto:alice@nebulamail.test",
    title: "Planning",
    uid: "u-2",
  };
  const stored = {
    duration: "PT1H",
    showWithoutTime: false,
    start: "2026-10-28T12:00:00",
    timeZone: "Europe/Madrid",
  };

  it("takes the time of the event in our calendar when it has none", () => {
    expect(withStoredTiming(reply, stored)).toMatchObject({
      duration: "PT1H",
      start: "2026-10-28T12:00:00",
      timeZone: "Europe/Madrid",
    });
    // One occurrence's reply: at its RECURRENCE-ID.
    expect(
      withStoredTiming(
        { ...reply, recurrenceId: "2026-10-30T12:00:00" },
        stored
      ).start
    ).toBe("2026-10-30T12:00:00");
  });

  it("keeps its own time, and leaves it alone without a stored copy", () => {
    const timed = { ...reply, start: "2026-11-02T09:00:00" };
    expect(withStoredTiming(timed, stored)).toBe(timed);
    expect(withStoredTiming(reply, undefined)).toBe(reply);
  });
});

// ---- A repeating series in iCalendar (a delegated task, 2026-11-01) -------

const madrid = (local: string): ICalTime => ({
  kind: "local",
  local,
  timeZone: "Europe/Madrid",
});
const day = (date: string): ICalTime => ({ date, kind: "date" });
const at = (local: string, timeZone: string | null): ICalTime => ({
  kind: "local",
  local,
  timeZone,
});
const unfold = (text: string): string => text.replaceAll("\r\n ", "");
/** The parts `text` lacks (one assertion with a clear failure). */
const missing = (text: string, parts: readonly string[]): string[] =>
  parts.filter((part) => !text.includes(part));

describe(timeLine, () => {
  it.each([
    [day("2026-11-06"), "DTSTART;VALUE=DATE:20261106"],
    [
      madrid("2026-11-06T09:00:00"),
      "DTSTART;TZID=Europe/Madrid:20261106T090000",
    ],
    [at("2026-11-06T09:00:00", "Etc/UTC"), "DTSTART:20261106T090000Z"],
    [at("2026-11-06T09:00:00", null), "DTSTART:20261106T090000"],
    // A zone nobody knows, or one that would break the line.
    [at("2026-11-06T09:00:00", "Mars/Olympus"), null],
    [at("2026-11-06T09:00:00", "X:\r\nATTENDEE"), null],
  ])("writes %j as %s", (time, line) => {
    expect(timeLine("DTSTART", time)).toBe(line);
  });
});

describe(rruleOf, () => {
  const start = madrid("2026-11-06T09:00:00");
  const daily = { frequency: "daily", until: "2026-12-31T00:00:00" };

  it.each([
    // UNTIL in UTC for a zoned start (the end of 31 December in Madrid).
    [
      {
        byDay: [{ day: "mo" }, { day: "fr" }],
        frequency: "weekly",
        interval: 2,
        until: "2026-12-31T23:59:59",
      },
      start,
      "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;UNTIL=20261231T225959Z",
    ],
    // A day for days, floating for floating.
    [daily, day("2026-11-06"), "FREQ=DAILY;UNTIL=20261231"],
    [
      daily,
      at("2026-11-06T09:00:00", null),
      "FREQ=DAILY;UNTIL=20261231T000000",
    ],
    [
      {
        byDay: [{ day: "fr", nthOfPeriod: -1 }],
        bySetPosition: [1],
        count: 6,
        firstDayOfWeek: "su",
        frequency: "monthly",
      },
      start,
      "FREQ=MONTHLY;WKST=SU;BYDAY=-1FR;BYSETPOS=1;COUNT=6",
    ],
    // RSCALE and SKIP (RFC 7529).
    [
      {
        byMonth: ["2"],
        byMonthDay: [29],
        frequency: "yearly",
        rscale: "gregorian",
        skip: "forward",
      },
      start,
      "FREQ=YEARLY;RSCALE=GREGORIAN;SKIP=FORWARD;BYMONTH=2;BYMONTHDAY=29",
    ],
    // A leap month only with a calendar that has them (RFC 7529 §4.1).
    [
      { byMonth: ["5L"], frequency: "yearly", rscale: "chinese" },
      start,
      "FREQ=YEARLY;RSCALE=CHINESE;BYMONTH=5L",
    ],
  ])("writes %j", (rule, from, rrule) => {
    expect(rruleOf(rule, from)).toBe(rrule);
  });

  it.each([
    { frequency: "fortnightly" },
    { byDay: [{ day: "xx" }], frequency: "weekly" },
    { byMonthDay: [0], frequency: "monthly" },
    { frequency: "daily", interval: 0 },
    { count: 3, frequency: "daily", until: "2026-12-31T00:00:00" },
    { frequency: "daily", skip: "sideways" },
    // "L" and month 13 need RSCALE (RFC 7529; RFC 5545: 1 to 12).
    { byMonth: ["5L"], frequency: "yearly" },
    { byMonth: ["5L"], frequency: "yearly", rscale: "gregorian" },
    { byMonth: ["13"], frequency: "yearly" },
  ])("refuses %j (can't be written exactly)", (rule) => {
    expect(rruleOf(rule, start)).toBeNull();
  });

  // RFC 5545 §3.3.10: parts that MUST NOT go with that FREQ.
  it.each([
    { byWeekNo: [1], frequency: "monthly" },
    { byYearDay: [100], frequency: "daily" },
    { byYearDay: [100], frequency: "weekly" },
    { byYearDay: [100], frequency: "monthly" },
    { byMonthDay: [1], frequency: "weekly" },
    { byDay: [{ day: "mo", nthOfPeriod: 1 }], frequency: "weekly" },
    { byDay: [{ day: "mo", nthOfPeriod: -1 }], frequency: "daily" },
    { bySetPosition: [1], frequency: "monthly" },
    // "MUST NOT be specified with a numeric value with the FREQ rule part
    // set to YEARLY when the BYWEEKNO rule part is specified".
    {
      byDay: [{ day: "mo", nthOfPeriod: 1 }],
      byWeekNo: [20],
      frequency: "yearly",
    },
  ])("refuses %j (RFC 5545 forbids it)", (rule) => {
    expect(rruleOf(rule, start)).toBeNull();
  });

  it.each([
    [{ byWeekNo: [20], frequency: "yearly" }, "FREQ=YEARLY;BYWEEKNO=20"],
    [{ byYearDay: [-1], frequency: "hourly" }, "FREQ=HOURLY;BYYEARDAY=-1"],
    [
      { byDay: [{ day: "mo", nthOfPeriod: 2 }], frequency: "yearly" },
      "FREQ=YEARLY;BYDAY=2MO",
    ],
    [
      { byDay: [{ day: "mo" }], byWeekNo: [20], frequency: "yearly" },
      "FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO",
    ],
  ])("writes %j (allowed with that FREQ)", (rule, rrule) => {
    expect(rruleOf(rule, start)).toBe(rrule);
  });
});

describe(vtimezoneLines, () => {
  const from = Date.UTC(2026, 10, 6, 8);

  it("writes the zone's changes as RDATEs, from the series' start", () => {
    const lines = vtimezoneLines("Europe/Madrid", from) ?? [];
    expect(lines.slice(0, 2)).toStrictEqual([
      "BEGIN:VTIMEZONE",
      "TZID:Europe/Madrid",
    ]);
    // The offset at the start (winter), then summer from March 2027
    // (02:00 in winter time) and winter from October 2027 (03:00 in
    // summer time), each year.
    expect(
      missing(lines.join("\n"), [
        "BEGIN:STANDARD\nDTSTART:20261105T090000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD",
        "BEGIN:DAYLIGHT\nDTSTART:20270328T020000\nRDATE:20280326T020000",
        "TZOFFSETFROM:+0100\nTZOFFSETTO:+0200\nEND:DAYLIGHT",
        "BEGIN:STANDARD\nDTSTART:20271031T030000\nRDATE:20281029T030000",
      ])
    ).toStrictEqual([]);
    // About 25 years of changes, two a year.
    expect(lines.filter((line) => line.startsWith("RDATE:"))).toHaveLength(48);
    expect(lines.at(-1)).toBe("END:VTIMEZONE");
  });

  it("writes one offset for a zone without changes", () => {
    const tokyo = vtimezoneLines("Asia/Tokyo", from) ?? [];
    expect(tokyo.join("\n")).toContain("TZOFFSETFROM:+0900\nTZOFFSETTO:+0900");
    expect(tokyo.some((line) => line.startsWith("RDATE"))).toBeFalsy();
  });

  it.each(["Etc/UTC", null, "Mars/Olympus"])("writes none for %s", (zone) => {
    expect(vtimezoneLines(zone, from)).toBeNull();
  });
});

describe("buildTaskRequest with a series", () => {
  const base = {
    attendees: [
      {
        delegatedFrom: null,
        delegatedTo: "dana@example.com",
        email: "bob@example.com",
        name: "Bob",
        partstat: "DELEGATED",
        rsvp: false,
      },
      {
        delegatedFrom: "bob@example.com",
        delegatedTo: null,
        email: "dana@example.com",
        name: "Dana",
        partstat: "NEEDS-ACTION",
        rsvp: true,
      },
    ],
    description: null,
    due: null,
    now: Date.UTC(2026, 10, 1, 10),
    organizer: { email: "alice@example.com", name: "Alice" },
    sequence: 3,
    title: "Informe semanal",
    uid: "t1@uid",
  };
  const series: ItipSeries = {
    due: madrid("2026-11-06T09:00:00"),
    excluded: [madrid("2026-11-20T09:00:00")],
    overrides: [
      {
        completed: true,
        due: madrid("2026-11-13T09:00:00"),
        recurrenceId: madrid("2026-11-13T09:00:00"),
        start: madrid("2026-11-13T09:00:00"),
        title: null,
      },
      {
        completed: false,
        due: madrid("2026-11-28T12:00:00"),
        recurrenceId: madrid("2026-11-27T09:00:00"),
        start: madrid("2026-11-28T12:00:00"),
        title: "Informe (sábado)",
      },
    ],
    rrule: "FREQ=WEEKLY;BYDAY=FR",
    start: madrid("2026-11-06T09:00:00"),
  };

  it("sends the series in its zone, with a VTIMEZONE first", () => {
    const ics = unfold(buildTaskRequest({ ...base, series }));
    expect(ics).toMatch(
      /^METHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Europe\/Madrid\r\n/mu
    );
    expect(ics.indexOf("END:VTIMEZONE")).toBeLessThan(
      ics.indexOf("BEGIN:VTODO")
    );
    expect(ics).toContain(
      "DTSTART;TZID=Europe/Madrid:20261106T090000\r\nDUE;TZID=Europe/Madrid:20261106T090000\r\nRRULE:FREQ=WEEKLY;BYDAY=FR\r\nEXDATE;TZID=Europe/Madrid:20261120T090000\r\n"
    );
    expect(ics).not.toContain("BEGIN:VALARM");
  });

  it("sends each occurrence that differs, with the people", () => {
    const ics = unfold(buildTaskRequest({ ...base, series }));
    // The series and two occurrences, each with UID and people (RFC 5546).
    expect({
      organizers: ics.match(/^ORGANIZER;CN="Alice"/gmu)?.length,
      tasks: ics.match(/^BEGIN:VTODO$/gmu)?.length,
      // The series' title in the occurrence that keeps it.
      titles: ics.match(/^SUMMARY:Informe semanal$/gmu)?.length,
      uids: ics.match(/^UID:t1@uid$/gmu)?.length,
    }).toStrictEqual({ organizers: 3, tasks: 3, titles: 2, uids: 3 });
    expect(
      missing(ics, [
        "RECURRENCE-ID;TZID=Europe/Madrid:20261113T090000\r\nDTSTART;TZID=Europe/Madrid:20261113T090000\r\nDUE;TZID=Europe/Madrid:20261113T090000\r\nSTATUS:COMPLETED\r\n",
        "RECURRENCE-ID;TZID=Europe/Madrid:20261127T090000\r\nDTSTART;TZID=Europe/Madrid:20261128T120000\r\nDUE;TZID=Europe/Madrid:20261128T120000\r\nSUMMARY:Informe (sábado)\r\n",
      ])
    ).toStrictEqual([]);
  });

  it("sends a series of days as dates, without a VTIMEZONE", () => {
    const ics = unfold(
      buildTaskRequest({
        ...base,
        series: {
          due: day("2026-11-06"),
          excluded: [day("2026-11-20")],
          overrides: [],
          rrule: "FREQ=WEEKLY",
          start: day("2026-11-06"),
        },
      })
    );
    expect(ics).not.toContain("VTIMEZONE");
    expect(ics).toContain(
      "DTSTART;VALUE=DATE:20261106\r\nDUE;VALUE=DATE:20261106\r\nRRULE:FREQ=WEEKLY\r\nEXDATE;VALUE=DATE:20261120\r\n"
    );
  });

  it("sends a series in UTC without a VTIMEZONE", () => {
    const utc = at("2026-11-06T09:00:00", "Etc/UTC");
    const ics = unfold(
      buildTaskRequest({
        ...base,
        series: {
          due: utc,
          excluded: [],
          overrides: [],
          rrule: "FREQ=DAILY",
          start: utc,
        },
      })
    );
    expect(ics).not.toContain("VTIMEZONE");
    expect(ics).toContain(
      "DTSTART:20261106T090000Z\r\nDUE:20261106T090000Z\r\n"
    );
  });

  it("refuses a series it can't write", () => {
    // EXDATE of another value type than DTSTART (RFC 5545).
    const mixed = { ...series, excluded: [day("2026-11-20")] };
    expect(() => buildTaskRequest({ ...base, series: mixed })).toThrow(
      "The series can't be written"
    );
    expect({
      // A time in another zone than the series'.
      otherZone: seriesWritable({
        ...series,
        excluded: [at("2026-11-20T09:00:00", "Etc/UTC")],
      }),
      // Something that would break the RRULE line.
      rule: seriesWritable({ ...series, rrule: "FREQ=DAILY\r\nATTENDEE:x" }),
      series: seriesWritable(series),
      types: seriesWritable(mixed),
    }).toStrictEqual({
      otherZone: false,
      rule: false,
      series: true,
      types: false,
    });
  });
});
