// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Words for dates, reminders and repetition in the user's language.

import { parseDate } from "@internationalized/date";

import type { DateRange, CalendarView } from "../../calendar/layout";
import type { ParticipationStatus } from "../../calendar/model";
import type { RecurrenceWords } from "../../calendar/recurrence";
import { dateInZone, dateTimeFormat, formatDate } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";

/**
 * An event's time. An all-day event must say its days ("YYYY-MM-DD", the
 * first and the day after the last; null if unknown): without them only
 * the first day would show.
 */
export type Timing =
  | {
      readonly allDay: true;
      readonly start: number;
      readonly end: number;
      readonly firstDay: string | null;
      readonly endDay: string | null;
    }
  | { readonly allDay: false; readonly start: number; readonly end: number };

const DAY_MS = 86_400_000;

/**
 * The timing of something known only by its instants (a reminder): an
 * all-day one gets its days from the midnights in `zone` (rounded, so a
 * day of 23 or 25 hours still counts as one).
 */
export const timingOf = (
  { allDay, end, start }: { allDay: boolean; start: number; end: number },
  zone: string
): Timing => {
  if (!allDay) {
    return { allDay, end, start };
  }
  const first = dateInZone(start, zone);
  const days = Math.max(1, Math.round((end - start) / DAY_MS));
  return {
    allDay,
    end,
    endDay: first.add({ days }).toString(),
    firstDay: first.toString(),
    start,
  };
};

/** "Monday, 26 October 2026, 10:00 – 11:00" (or the days of an all-day event). */
export const formatWhen = (
  timing: Timing,
  zone: string,
  locale: string
): string => {
  if (timing.allDay) {
    const first = timing.firstDay === null ? null : parseDate(timing.firstDay);
    const end = timing.endDay === null ? null : parseDate(timing.endDay);
    if (first !== null && end !== null) {
      const last = end.subtract({ days: 1 });
      if (last.compare(first) <= 0) {
        return formatDate(first, locale, { dateStyle: "full" });
      }
      return dateTimeFormat(locale, {
        dateStyle: "medium",
        timeZone: "UTC",
      }).formatRange(
        Date.UTC(first.year, first.month - 1, first.day),
        Date.UTC(last.year, last.month - 1, last.day)
      );
    }
    return dateTimeFormat(locale, {
      dateStyle: "full",
      timeZone: zone,
    }).format(timing.start);
  }
  return dateTimeFormat(locale, {
    dateStyle: "full",
    timeStyle: "short",
    timeZone: zone,
  }).formatRange(timing.start, Math.max(timing.start, timing.end));
};

/** The same times in the event's own zone, if it differs from the user's. */
export const formatInZone = (
  timing: { readonly start: number; readonly end: number },
  eventZone: string | null,
  zone: string,
  locale: string
): string | null => {
  if (eventZone === null || eventZone === zone) {
    return null;
  }
  const offset = (target: string): string =>
    dateTimeFormat("en", {
      timeZone: target,
      timeZoneName: "longOffset",
    })
      .formatToParts(timing.start)
      .find((part) => part.type === "timeZoneName")?.value ?? "";
  try {
    if (offset(eventZone) === offset(zone)) {
      return null;
    }
    // dateStyle and timeStyle can't be combined with timeZoneName.
    return dateTimeFormat(locale, {
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
      month: "short",
      timeZone: eventZone,
      timeZoneName: "short",
      weekday: "short",
    }).formatRange(timing.start, Math.max(timing.start, timing.end));
  } catch {
    return null;
  }
};

/** "10 minutes before", "1 hour before", "At the start". */
export const alertText = (minutesBefore: number): string => {
  if (minutesBefore === 0) {
    return m.cal_alert_at_start();
  }
  if (minutesBefore < 0) {
    return m.cal_alert_after({ count: -minutesBefore });
  }
  if (minutesBefore % 1440 === 0) {
    return m.cal_alert_days({ count: minutesBefore / 1440 });
  }
  if (minutesBefore % 60 === 0) {
    return m.cal_alert_hours({ count: minutesBefore / 60 });
  }
  return m.cal_alert_minutes({ count: minutesBefore });
};

export const recurrenceWords: RecurrenceWords = {
  custom: () => m.cal_repeat_custom_rule(),
  repeat: (frequency, interval) => {
    switch (frequency) {
      case "daily": {
        return m.cal_every_days({ count: interval });
      }
      case "weekly": {
        return m.cal_every_weeks({ count: interval });
      }
      case "monthly": {
        return m.cal_every_months({ count: interval });
      }
      case "yearly": {
        return m.cal_every_years({ count: interval });
      }
      default: {
        return frequency satisfies never;
      }
    }
  },
  onDays: (days) => m.cal_on_days({ days }),
  times: (count) => m.cal_times({ count }),
  until: (date) => m.cal_until({ date }),
};

/** Badge colour of an answer. */
export const statusTone = (
  status: ParticipationStatus
): "ok" | "danger" | "warn" | "neutral" => {
  const tones = {
    accepted: "ok",
    declined: "danger",
    delegated: "neutral",
    "needs-action": "neutral",
    tentative: "warn",
  } as const;
  return tones[status];
};

export const statusText = (status: ParticipationStatus): string => {
  switch (status) {
    case "accepted": {
      return m.cal_status_accepted();
    }
    case "declined": {
      return m.cal_status_declined();
    }
    case "tentative": {
      return m.cal_status_tentative();
    }
    case "delegated": {
      return m.cal_status_delegated();
    }
    case "needs-action": {
      return m.cal_status_needs_action();
    }
    default: {
      return status satisfies never;
    }
  }
};

/** The toolbar title: "October 2026", "26 Oct – 1 Nov 2026", a day. */
export const rangeTitle = (
  view: CalendarView,
  anchor: ReturnType<typeof parseDate>,
  range: DateRange,
  locale: string
): string => {
  switch (view) {
    case "month": {
      return formatDate(anchor, locale, { month: "long", year: "numeric" });
    }
    case "day": {
      return formatDate(anchor, locale, {
        day: "numeric",
        month: "long",
        weekday: "long",
        year: "numeric",
      });
    }
    case "week":
    case "agenda": {
      const last = range.end.subtract({ days: 1 });
      return dateTimeFormat(locale, {
        day: "numeric",
        month: "short",
        timeZone: "UTC",
        year: "numeric",
      }).formatRange(
        Date.UTC(range.start.year, range.start.month - 1, range.start.day),
        Date.UTC(last.year, last.month - 1, last.day)
      );
    }
    default: {
      return view satisfies never;
    }
  }
};

/**
 * The same range, short enough for a phone's top bar ("Sun, Nov 8",
 * "Nov 8 – 14", "Nov 2026"): no weekday for ranges, no year for days.
 */
export const rangeTitleShort = (
  view: CalendarView,
  anchor: ReturnType<typeof parseDate>,
  range: DateRange,
  locale: string
): string => {
  switch (view) {
    case "month": {
      return formatDate(anchor, locale, { month: "short", year: "numeric" });
    }
    case "day": {
      return formatDate(anchor, locale, {
        day: "numeric",
        month: "short",
        weekday: "short",
      });
    }
    case "week":
    case "agenda": {
      const last = range.end.subtract({ days: 1 });
      return dateTimeFormat(locale, {
        day: "numeric",
        month: "short",
        timeZone: "UTC",
      }).formatRange(
        Date.UTC(range.start.year, range.start.month - 1, range.start.day),
        Date.UTC(last.year, last.month - 1, last.day)
      );
    }
    default: {
      return view satisfies never;
    }
  }
};
