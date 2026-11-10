// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { RecurrenceWords } from "./recurrence";
import {
  describeRecurrence,
  fromPreset,
  fromRule,
  NO_RECURRENCE,
  presetOf,
  toRule,
  weekdayOf,
} from "./recurrence";

const words: RecurrenceWords = {
  custom: () => "Custom",
  repeat: (frequency, interval) =>
    interval === 1
      ? `Every ${frequency}`
      : `Every ${String(interval)} × ${frequency}`,
  onDays: (days) => `on ${days}`,
  times: (count) => `${String(count)} times`,
  until: (date) => `until ${date}`,
};

describe(fromRule, () => {
  it("reads the simple rules the editor offers", () => {
    expect(fromRule({ count: 3, frequency: "weekly" }, false)).toStrictEqual({
      byDay: [],
      end: { count: 3, kind: "count" },
      frequency: "weekly",
      interval: 1,
      kind: "simple",
    });
    expect(
      fromRule(
        {
          byDay: [{ day: "mo" }, { day: "we" }],
          frequency: "weekly",
          interval: 2,
          until: "2026-12-31T23:59:59",
        },
        false
      )
    ).toMatchObject({
      byDay: ["mo", "we"],
      end: { date: "2026-12-31", kind: "until" },
      interval: 2,
    });
  });

  it("keeps anything more complex as a custom rule", () => {
    const rule = {
      byDay: [{ day: "mo", nthOfPeriod: 1 }],
      frequency: "monthly",
    };
    expect(fromRule(rule, false)).toStrictEqual({ kind: "custom", rule });
    expect(
      fromRule({ bySetPosition: [-1], frequency: "monthly" }, false).kind
    ).toBe("custom");
    expect(fromRule({ frequency: "hourly" }, false).kind).toBe("custom");
    expect(fromRule(null, false)).toBe(NO_RECURRENCE);
  });

  it("keeps a rule saving would change as custom", () => {
    // WKST picks other days for WEEKLY;INTERVAL=2;BYDAY (RFC 5545
    // §3.3.10): toRule doesn't write it, so the rule stays untouched.
    const sunday = {
      byDay: [{ day: "mo" }, { day: "su" }],
      firstDayOfWeek: "su",
      frequency: "weekly",
      interval: 2,
    };
    expect(fromRule(sunday, false)).toStrictEqual({
      kind: "custom",
      rule: sunday,
    });
    expect(toRule(fromRule(sunday, false), false)).toStrictEqual(sunday);
    // Monday is the default: nothing is lost.
    expect(fromRule({ ...sunday, firstDayOfWeek: "mo" }, false).kind).toBe(
      "simple"
    );
  });

  it("keeps an until at another time than toRule writes as custom", () => {
    const noon = { frequency: "daily", until: "2026-12-31T12:00:00" };
    expect(fromRule(noon, false)).toStrictEqual({ kind: "custom", rule: noon });
    expect(
      fromRule({ frequency: "daily", until: "2026-12-31T23:59:59" }, true).kind
    ).toBe("custom");
    expect(
      fromRule({ frequency: "daily", until: "2026-12-31T00:00:00" }, true).kind
    ).toBe("simple");
  });
});

describe(toRule, () => {
  it("writes the editor's choice back as JSCalendar", () => {
    expect(
      toRule(
        {
          byDay: ["fr", "mo"],
          end: { date: "2027-01-31", kind: "until" },
          frequency: "weekly",
          interval: 1,
          kind: "simple",
        },
        false
      )
    ).toStrictEqual({
      "@type": "RecurrenceRule",
      byDay: [
        { "@type": "NDay", day: "mo" },
        { "@type": "NDay", day: "fr" },
      ],
      frequency: "weekly",
      until: "2027-01-31T23:59:59",
    });
    expect(
      toRule(
        {
          byDay: [],
          end: { count: 5, kind: "count" },
          frequency: "yearly",
          interval: 2,
          kind: "simple",
        },
        true
      )
    ).toStrictEqual({
      "@type": "RecurrenceRule",
      count: 5,
      frequency: "yearly",
      interval: 2,
    });
  });

  it("returns custom rules untouched (round trip)", () => {
    const rule = {
      bySetPosition: [-1],
      byDay: [{ day: "fr" }],
      frequency: "monthly",
    };
    expect(toRule(fromRule(rule, false), false)).toBe(rule);
    expect(toRule(NO_RECURRENCE, false)).toBeNull();
  });
});

describe("presets", () => {
  it("maps quick choices both ways", () => {
    const weekdays = fromPreset("weekdays", NO_RECURRENCE);
    expect(presetOf(weekdays)).toBe("weekdays");
    expect(presetOf(fromPreset("monthly", weekdays))).toBe("monthly");
    expect(presetOf(fromRule({ count: 2, frequency: "daily" }, false))).toBe(
      "custom"
    );
    expect(presetOf(NO_RECURRENCE)).toBe("none");
  });

  it("knows the weekday of a date", () => {
    expect(weekdayOf("2026-10-26")).toBe("mo");
    expect(weekdayOf("2026-11-01")).toBe("su");
    expect(weekdayOf("nope")).toBeNull();
  });
});

describe(describeRecurrence, () => {
  it("describes rules in the user's language", () => {
    expect(
      describeRecurrence(
        fromRule(
          {
            byDay: [{ day: "mo" }, { day: "we" }],
            count: 10,
            frequency: "weekly",
          },
          false
        ),
        "en",
        words
      )
    ).toBe("Every weekly, on Monday and Wednesday, 10 times");
    expect(
      describeRecurrence(
        // A series of days: its until is that day (T00:00:00).
        fromRule({ frequency: "monthly", until: "2027-03-01T00:00:00" }, true),
        "es",
        words
      )
    ).toBe("Every monthly, until 1 mar 2027");
    expect(describeRecurrence(NO_RECURRENCE, "en", words)).toBeNull();
  });
});
