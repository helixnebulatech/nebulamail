// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reminders of a task due on a day without a time: "on the
// day at 9:00", "1 day before at 9:00", like Apple Reminders and Google
// Tasks. Pure, with tests.
//
// They are stored like any task reminder, as minutes before the due, and a
// day's due is its midnight: "1 day before at 9:00" is 15 hours before
// (-PT15H), "on the day at 9:00" 9 hours after (PT9H). Every client
// understands that. A single task gets the absolute time (worked out on
// the wall clock of the user's zone, so a change of daylight saving time
// in between doesn't move it); a repeating one the offset from its due
// (RELATED=END). Stalwart reads a day's midnight in the calendar's zone
// for those offsets (UTC in a calendar without one)

import { toZoned } from "@internationalized/date";

import {
  DAY_MINUTES,
  dateInZone,
  minutesInDay,
  parseLocalDate,
} from "../calendar/time";

/** 9:00, when the user hasn't chosen another time. */
export const DEFAULT_REMINDER_MINUTES = 9 * 60;

/** How many days before the due a day reminder can be (the choices). */
export const DAY_REMINDER_CHOICES = [0, 1, 2, 7] as const;

/** A reminder of a day task: so many days before, at a time of day. */
export interface DayReminder {
  /** 0: on the day; negative: after it. */
  readonly daysBefore: number;
  /** Minutes since midnight (0–1439). */
  readonly minutes: number;
}

const mod = (value: number, by: number): number => ((value % by) + by) % by;

/** Minutes before the due midnight → days before and time of day. */
export const dayReminderOf = (minutesBefore: number): DayReminder => {
  const before = Math.round(minutesBefore);
  const minutes = mod(-before, DAY_MINUTES);
  return { daysBefore: (before + minutes) / DAY_MINUTES, minutes };
};

/** Days before and time of day → minutes before the due midnight. */
export const minutesBeforeOf = (reminder: DayReminder): number =>
  reminder.daysBefore * DAY_MINUTES - reminder.minutes;

/**
 * When a reminder of a task due on `date` rings (epoch ms), on the wall
 * clock of `zone`: the day `daysBefore` earlier at its time.
 */
export const dayReminderInstant = (
  date: string,
  minutesBefore: number,
  zone: string
): number | null => {
  const due = parseLocalDate(date);
  if (due === null) {
    return null;
  }
  const { daysBefore, minutes } = dayReminderOf(minutesBefore);
  const day = due.subtract({ days: daysBefore });
  return toZoned(day, zone)
    .set({ hour: Math.floor(minutes / 60), minute: minutes % 60 })
    .toDate()
    .getTime();
};

/**
 * Minutes before the due midnight of a reminder at `at` (epoch ms), on the
 * wall clock of `zone` (the inverse of `dayReminderInstant`).
 */
export const dayMinutesBefore = (
  date: string,
  at: number,
  zone: string
): number | null => {
  const due = parseLocalDate(date);
  if (due === null || !Number.isFinite(at)) {
    return null;
  }
  const days = due.compare(dateInZone(at, zone));
  return minutesBeforeOf({ daysBefore: days, minutes: minutesInDay(at, zone) });
};

/** "09:00" → 540; null if it isn't a time. */
export const minutesOfTime = (hour: number, minute: number): number | null =>
  Number.isInteger(hour) &&
  Number.isInteger(minute) &&
  hour >= 0 &&
  hour < 24 &&
  minute >= 0 &&
  minute < 60
    ? hour * 60 + minute
    : null;

/** A stored default time (synced preference) or 9:00. */
export const reminderMinutes = (value: number | null | undefined): number =>
  value !== null &&
  value !== undefined &&
  Number.isInteger(value) &&
  value >= 0 &&
  value < DAY_MINUTES
    ? value
    : DEFAULT_REMINDER_MINUTES;

/**
 * Reminders when a due changes between a day and a time: a timed one
 * ("10 minutes before") becomes "on the day" (or so many days before) at
 * the default time, and a day one keeps its days ("1 day before").
 */
export const convertReminder = (
  minutesBefore: number,
  to: "date" | "datetime",
  defaultMinutes: number
): number => {
  if (to === "datetime") {
    return Math.max(0, dayReminderOf(minutesBefore).daysBefore) * DAY_MINUTES;
  }
  const days = Math.max(0, Math.floor(minutesBefore / DAY_MINUTES));
  return minutesBeforeOf({ daysBefore: days, minutes: defaultMinutes });
};
