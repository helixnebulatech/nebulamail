// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  DEFAULT_REMINDER_MINUTES,
  convertReminder,
  dayMinutesBefore,
  dayReminderInstant,
  dayReminderOf,
  minutesBeforeOf,
  reminderMinutes,
} from "./day-reminders";

const zone = "Europe/Madrid";

describe(dayReminderOf, () => {
  it("reads days before and the time of day", () => {
    // On the day at 9:00: 9 hours after the due midnight (PT9H).
    expect(dayReminderOf(-540)).toStrictEqual({ daysBefore: 0, minutes: 540 });
    // 1 day before at 9:00: 15 hours before (-PT15H).
    expect(dayReminderOf(900)).toStrictEqual({ daysBefore: 1, minutes: 540 });
    // 2 days before at 9:00 (-P1DT15H) and a week before at 18:30.
    expect(dayReminderOf(2340)).toStrictEqual({ daysBefore: 2, minutes: 540 });
    expect(dayReminderOf(7 * 1440 - 1110)).toStrictEqual({
      daysBefore: 7,
      minutes: 1110,
    });
  });

  it("reads other apps' whole days and days after", () => {
    // The midnight itself, and a whole day before (what other apps write).
    expect(dayReminderOf(0)).toStrictEqual({ daysBefore: 0, minutes: 0 });
    expect(dayReminderOf(1440)).toStrictEqual({ daysBefore: 1, minutes: 0 });
    // The day after at 9:00.
    expect(dayReminderOf(-1980)).toStrictEqual({
      daysBefore: -1,
      minutes: 540,
    });
  });

  it("goes back and forth", () => {
    for (const minutes of [-1980, -540, 0, 1, 900, 1440, 2340, 9000]) {
      expect(minutesBeforeOf(dayReminderOf(minutes))).toBe(minutes);
    }
  });
});

describe(dayReminderInstant, () => {
  it("rings on the wall clock of the zone", () => {
    expect(dayReminderInstant("2026-11-06", 900, zone)).toBe(
      Date.UTC(2026, 10, 5, 8)
    );
    expect(dayReminderInstant("2026-11-06", -540, zone)).toBe(
      Date.UTC(2026, 10, 6, 8)
    );
  });

  it("keeps 9:00 across a change of daylight saving time", () => {
    // Summer time ends on 25 October 2026 in Madrid: a week before the
    // 30th at 9:00 is the 23rd at 9:00 CEST (7:00 UTC), not 8:00.
    const at = dayReminderInstant("2026-10-30", 7 * 1440 - 540, zone);
    expect(at).toBe(Date.UTC(2026, 9, 23, 7));
    expect(dayMinutesBefore("2026-10-30", at ?? 0, zone)).toBe(7 * 1440 - 540);
  });

  it("refuses a bad date", () => {
    expect(dayReminderInstant("nope", 0, zone)).toBeNull();
    expect(dayMinutesBefore("nope", 0, zone)).toBeNull();
  });
});

describe(convertReminder, () => {
  it("turns timed reminders into day ones at the default time", () => {
    expect(convertReminder(10, "date", 540)).toBe(-540);
    expect(convertReminder(1440, "date", 540)).toBe(900);
    expect(convertReminder(2880, "date", 600)).toBe(2280);
    expect(convertReminder(-30, "date", 540)).toBe(-540);
  });

  it("keeps the days when a day gets a time", () => {
    expect(convertReminder(-540, "datetime", 540)).toBe(0);
    expect(convertReminder(900, "datetime", 540)).toBe(1440);
    expect(convertReminder(-1980, "datetime", 540)).toBe(0);
  });
});

describe(reminderMinutes, () => {
  it("takes a stored time or 9:00", () => {
    expect(reminderMinutes(600)).toBe(600);
    expect(reminderMinutes(undefined)).toBe(DEFAULT_REMINDER_MINUTES);
    expect(reminderMinutes(1440)).toBe(DEFAULT_REMINDER_MINUTES);
    expect(reminderMinutes(1.5)).toBe(DEFAULT_REMINDER_MINUTES);
  });
});
