// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { DueAlert } from "./alerts";
import {
  alertKey,
  alertOf,
  dismissPatch,
  occurrenceStart,
  snoozePatch,
  stillRelevant,
  utcDateTime,
} from "./alerts";

const NOW = Date.parse("2026-10-26T20:50:17.123Z");

describe(dismissPatch, () => {
  it("marks an alert as seen now (UTCDateTime, no milliseconds)", () => {
    expect(dismissPatch("a1", NOW)).toStrictEqual({
      "alerts/a1/acknowledged": "2026-10-26T20:50:17Z",
    });
  });

  it("removes a snooze alert instead", () => {
    expect(dismissPatch("snzabc", NOW)).toStrictEqual({
      "alerts/snzabc": null,
    });
  });
});

describe(snoozePatch, () => {
  it("acknowledges the alert and adds one later, related to it", () => {
    expect(snoozePatch("a1", "a1", 10, NOW, "snz1")).toStrictEqual({
      "alerts/a1/acknowledged": "2026-10-26T20:50:17Z",
      "alerts/snz1": {
        "@type": "Alert",
        action: "display",
        relatedTo: {
          a1: { "@type": "Relation", relation: { parent: true } },
        },
        trigger: { "@type": "AbsoluteTrigger", when: "2026-10-26T21:00:17Z" },
      },
    });
  });

  it("replaces a snooze with the next one, still under the first alert", () => {
    const patch = snoozePatch("snz1", "a1", 5, NOW, "snz2");
    expect(patch["alerts/snz1"]).toBeNull();
    expect(patch).toMatchObject({
      "alerts/snz2": { relatedTo: { a1: { relation: { parent: true } } } },
    });
  });
});

describe(alertKey, () => {
  it("is the same for the same alert of the same occurrence only", () => {
    const alert = {
      "@type": "CalendarAlert" as const,
      accountId: AccountId.parse("c"),
      alertId: "a1",
      calendarEventId: CalendarEventId.parse("v"),
      recurrenceId: null,
    };
    expect(alertKey(alert)).toBe(alertKey({ ...alert }));
    expect(alertKey(alert)).not.toBe(
      alertKey({ ...alert, recurrenceId: "2026-10-30T09:00:00Z" })
    );
    expect(alertKey(alert)).not.toBe(
      alertKey({ ...alert, accountId: AccountId.parse("d") })
    );
  });
});

const due = (end: number | null): DueAlert => ({
  accountId: AccountId.parse("c"),
  alertId: "a1",
  allDay: false,
  end,
  eventId: CalendarEventId.parse("v"),
  key: "k",
  kind: "event",
  recurrenceId: null,
  start: end === null ? null : end - 3_600_000,
  stored: true,
  title: "x",
});

describe(stillRelevant, () => {
  it("drops reminders of events that are over", () => {
    expect(stillRelevant(due(NOW + 1), NOW)).toBeTruthy();
    expect(stillRelevant(due(NOW - 1), NOW)).toBeFalsy();
    expect(stillRelevant(due(null), NOW)).toBeTruthy();
  });
});

describe(utcDateTime, () => {
  it("drops milliseconds", () => {
    expect(utcDateTime(Date.UTC(2026, 0, 2, 3, 4, 5, 678))).toBe(
      "2026-01-02T03:04:05Z"
    );
  });
});

describe(occurrenceStart, () => {
  const madrid = { timeZone: "Europe/Madrid" };

  it("reads Stalwart's LocalDateTime in the event's zone, not the browser's", () => {
    // 17:11:44 in Madrid on 27 October 2026 (CET, UTC+1).
    const expected = Date.parse("2026-10-27T16:11:44Z");
    expect(
      occurrenceStart("2026-10-27T17:11:44", madrid, "America/New_York")
    ).toBe(expected);
    expect(occurrenceStart("2026-10-27T17:11:44", madrid, "Asia/Tokyo")).toBe(
      expected
    );
  });

  it("follows summer time in the event's zone", () => {
    expect(occurrenceStart("2026-07-01T09:00:00", madrid, "UTC")).toBe(
      Date.parse("2026-07-01T07:00:00Z")
    );
  });

  it("still takes a UTC time or an offset as such", () => {
    expect(occurrenceStart("2026-10-30T09:00:00Z", madrid, "UTC")).toBe(
      Date.parse("2026-10-30T09:00:00Z")
    );
    expect(occurrenceStart("2026-10-30T09:00:00+02:00", madrid, "UTC")).toBe(
      Date.parse("2026-10-30T07:00:00Z")
    );
  });

  it("puts floating and all-day occurrences in the user's zone", () => {
    expect(occurrenceStart("2026-10-27T09:00:00", {}, "Asia/Tokyo")).toBe(
      Date.parse("2026-10-27T00:00:00Z")
    );
    expect(
      occurrenceStart(
        "2026-10-27T00:00:00",
        { showWithoutTime: true, timeZone: "Europe/Madrid" },
        "America/New_York"
      )
    ).toBe(Date.parse("2026-10-27T04:00:00Z"));
    expect(
      occurrenceStart("2026-10-27", { showWithoutTime: true }, "UTC")
    ).toBe(Date.parse("2026-10-27T00:00:00Z"));
  });

  it("is null without a recurrence id or with nonsense", () => {
    expect(occurrenceStart(null, madrid, "UTC")).toBeNull();
    expect(occurrenceStart("", madrid, "UTC")).toBeNull();
    expect(occurrenceStart("not a date", madrid, "UTC")).toBeNull();
  });
});

const pushed = (recurrenceId: string | null = null) => ({
  "@type": "CalendarAlert" as const,
  accountId: AccountId.parse("c"),
  alertId: "a1",
  calendarEventId: CalendarEventId.parse("t1"),
  recurrenceId,
  uid: "t1@x",
});

describe(alertOf, () => {
  it("reads a task's reminder about its due", () => {
    const alert = alertOf(
      pushed(),
      {
        "@type": "Task",
        alerts: { a1: {} },
        due: "2026-10-30T17:30:00",
        timeZone: "Europe/Madrid",
        title: "Pay‮ the invoice",
      },
      "UTC"
    );
    expect(alert).toMatchObject({
      allDay: false,
      end: null,
      kind: "task",
      start: Date.parse("2026-10-30T16:30:00Z"),
      stored: true,
      title: "Pay the invoice",
    });
    expect(
      alert === null || stillRelevant(alert, Date.parse("2027-01-01"))
    ).toBeTruthy();
  });

  it("dates an occurrence by its recurrence id", () => {
    expect(
      alertOf(
        pushed("2026-11-06T09:00:00"),
        {
          "@type": "Task",
          due: "2026-10-30T09:00:00",
          start: "2026-10-30T09:00:00",
          timeZone: "Etc/UTC",
          title: "Weekly report",
        },
        "UTC"
      )?.start
    ).toBe(Date.parse("2026-11-06T09:00:00Z"));
  });

  it("skips tasks (and occurrences) already done", () => {
    expect(
      alertOf(pushed(), { "@type": "Task", progress: "completed" }, "UTC")
    ).toBeNull();
    expect(
      alertOf(
        pushed("2026-11-06T09:00:00"),
        {
          "@type": "Task",
          due: "2026-10-30T09:00:00",
          recurrenceOverrides: {
            "2026-11-06T09:00:00": { progress: "completed" },
          },
          start: "2026-10-30T09:00:00",
        },
        "UTC"
      )
    ).toBeNull();
  });

  it("keeps reading events by their start and end", () => {
    expect(
      alertOf(
        pushed(),
        {
          "@type": "Event",
          duration: "PT1H",
          start: "2026-10-30T10:00:00",
          timeZone: "Etc/UTC",
          title: "Standup",
        },
        "UTC"
      )
    ).toMatchObject({
      end: Date.parse("2026-10-30T11:00:00Z"),
      kind: "event",
      start: Date.parse("2026-10-30T10:00:00Z"),
      stored: false,
    });
  });
});
