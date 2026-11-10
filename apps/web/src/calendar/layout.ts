// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where events go on screen: which days a view shows, the month grid, the
// columns of overlapping events in a day, the rows of all-day bars and the
// agenda list. Pure: the time zone, locale and week start are parameters.

import type { CalendarDate, DayOfWeek } from "@internationalized/date";
import { endOfMonth, startOfMonth, startOfWeek } from "@internationalized/date";

import type { Occurrence } from "./model";
import {
  DAY_MINUTES,
  dateInZone,
  minutesInDay,
  MINUTE_MS,
  parseLocalDate,
  startOfDayMs,
} from "./time";

export const VIEWS = ["day", "week", "month", "agenda"] as const;
export type CalendarView = (typeof VIEWS)[number];

export const isCalendarView = (value: unknown): value is CalendarView =>
  typeof value === "string" && (VIEWS as readonly string[]).includes(value);

/** Days the agenda shows from the anchor. */
export const AGENDA_DAYS = 30;

export interface DateRange {
  /** First day shown. */
  readonly start: CalendarDate;
  /** Day after the last day shown. */
  readonly end: CalendarDate;
}

/** The days a view shows around `anchor`. */
export const viewRange = (
  view: CalendarView,
  anchor: CalendarDate,
  locale: string,
  weekStart?: DayOfWeek
): DateRange => {
  switch (view) {
    case "day": {
      return { end: anchor.add({ days: 1 }), start: anchor };
    }
    case "week": {
      const start = startOfWeek(anchor, locale, weekStart);
      return { end: start.add({ days: 7 }), start };
    }
    case "month": {
      const start = startOfWeek(startOfMonth(anchor), locale, weekStart);
      const last = endOfMonth(anchor);
      let end = startOfWeek(last, locale, weekStart).add({ days: 7 });
      // Always 6 rows, so the grid doesn't jump between months.
      while (end.compare(start) < 42) {
        end = end.add({ days: 7 });
      }
      return { end, start };
    }
    case "agenda": {
      return { end: anchor.add({ days: AGENDA_DAYS }), start: anchor };
    }
    default: {
      return view satisfies never;
    }
  }
};

/** Previous or next page of a view. */
export const shiftAnchor = (
  view: CalendarView,
  anchor: CalendarDate,
  direction: -1 | 1
): CalendarDate => {
  switch (view) {
    case "day": {
      return anchor.add({ days: direction });
    }
    case "week": {
      return anchor.add({ weeks: direction });
    }
    case "month": {
      return anchor.add({ months: direction });
    }
    case "agenda": {
      return anchor.add({ days: direction * AGENDA_DAYS });
    }
    default: {
      return view satisfies never;
    }
  }
};

/** Every day in [start, end). */
export const daysOf = (range: DateRange): CalendarDate[] => {
  const days: CalendarDate[] = [];
  for (
    let day = range.start;
    day.compare(range.end) < 0;
    day = day.add({ days: 1 })
  ) {
    days.push(day);
  }
  return days;
};

/** Days in rows of 7 (the month grid). */
export const weeksOf = (range: DateRange): CalendarDate[][] => {
  const days = daysOf(range);
  const weeks: CalendarDate[][] = [];
  for (let index = 0; index < days.length; index += 7) {
    weeks.push(days.slice(index, index + 7));
  }
  return weeks;
};

/** Midnights already worked out, by zone and date (time zone rules don't change). */
const midnights = new Map<string, number>();
const MAX_MIDNIGHTS = 4096;

/**
 * `startOfDayMs`, remembered: the views ask for the same few days for every
 * event, and converting to a zone is slow.
 */
const midnight = (day: CalendarDate, zone: string): number => {
  const key = `${zone} ${day.toString()}`;
  let ms = midnights.get(key);
  if (ms === undefined) {
    ms = startOfDayMs(day, zone);
    if (midnights.size >= MAX_MIDNIGHTS) {
      midnights.clear();
    }
    midnights.set(key, ms);
  }
  return ms;
};

/** [start, end) of a day in a zone, in epoch ms. */
const dayBounds = (
  day: CalendarDate,
  zone: string
): { readonly start: number; readonly end: number } => ({
  end: midnight(day.add({ days: 1 }), zone),
  start: midnight(day, zone),
});

/** What `occursOn` needs of an occurrence, parsed once. */
type Span =
  | {
      readonly allDay: true;
      readonly first: CalendarDate | null;
      readonly end: CalendarDate | null;
    }
  | { readonly allDay: false; readonly start: number; readonly end: number };

const spanOf = (occurrence: Occurrence): Span =>
  occurrence.allDay
    ? {
        allDay: true,
        end: parseLocalDate(occurrence.endDay),
        first: parseLocalDate(occurrence.firstDay),
      }
    : { allDay: false, end: occurrence.end, start: occurrence.start };

const spanTouches = (span: Span, day: CalendarDate, zone: string): boolean => {
  if (span.allDay) {
    return (
      span.first !== null &&
      span.end !== null &&
      span.first.compare(day) <= 0 &&
      span.end.compare(day) > 0
    );
  }
  const bounds = dayBounds(day, zone);
  // A zero-length event still shows at its start.
  return (
    span.start < bounds.end &&
    (span.end > bounds.start || span.start === bounds.start)
  );
};

/** Whether an occurrence touches a day (in `zone`). */
export const occursOn = (
  occurrence: Occurrence,
  day: CalendarDate,
  zone: string
): boolean => spanTouches(spanOf(occurrence), day, zone);

/**
 * The occurrences that touch each week (rows of consecutive days, as
 * `weeksOf` gives), in their order: what the month view lays out per row,
 * without looking at every event for every day.
 */
export const occurrencesByWeek = (
  occurrences: readonly Occurrence[],
  weeks: readonly (readonly CalendarDate[])[],
  zone: string
): Occurrence[][] => {
  const ranges = weeks.map((week) => {
    const [first] = week;
    const last = week.at(-1);
    return first === undefined || last === undefined
      ? null
      : {
          endDay: last.add({ days: 1 }),
          endMs: midnight(last.add({ days: 1 }), zone),
          firstDay: first,
          startMs: midnight(first, zone),
        };
  });
  const buckets: Occurrence[][] = weeks.map(() => []);
  for (const occurrence of occurrences) {
    const span = spanOf(occurrence);
    for (const [index, range] of ranges.entries()) {
      if (range === null) {
        continue;
      }
      // The same test as `occursOn`, over the whole week.
      const touches = span.allDay
        ? span.first !== null &&
          span.end !== null &&
          span.first.compare(range.endDay) < 0 &&
          span.end.compare(range.firstDay) > 0
        : span.start < range.endMs &&
          (span.end > range.startMs || span.start === range.startMs);
      if (touches) {
        buckets[index]?.push(occurrence);
      }
    }
  }
  return buckets;
};

/**
 * Events that go in the all-day strip rather than the hours: all-day ones
 * and timed ones of 24 hours or more (like most calendars).
 */
export const isBanner = (occurrence: Occurrence): boolean =>
  occurrence.allDay || occurrence.end - occurrence.start >= 24 * 60 * MINUTE_MS;

const byStartThenLength = (a: Occurrence, b: Occurrence): number =>
  a.start - b.start ||
  b.end - b.start - (a.end - a.start) ||
  a.title.localeCompare(b.title) ||
  a.id.localeCompare(b.id);

/** Sorted the way every view lists events. */
export const sortOccurrences = (
  occurrences: readonly Occurrence[]
): Occurrence[] =>
  occurrences.toSorted(
    (a, b) => Number(b.allDay) - Number(a.allDay) || byStartThenLength(a, b)
  );

/** A timed event placed in one day column of the week or day view. */
export interface PlacedEvent {
  readonly occurrence: Occurrence;
  /** Minutes since midnight, clipped to the day. */
  readonly top: number;
  readonly bottom: number;
  /** It continues before this day / after it. */
  readonly continuesBefore: boolean;
  readonly continuesAfter: boolean;
  readonly column: number;
  readonly columns: number;
}

/** Minimum height in minutes, so short events stay clickable. */
const MIN_MINUTES = 20;

/**
 * Timed events of one day, side by side where they overlap (each cluster of
 * overlapping events shares its width equally).
 */
export const placeDay = (
  occurrences: readonly Occurrence[],
  day: CalendarDate,
  zone: string
): PlacedEvent[] => {
  const { end: dayEnd, start: dayStart } = dayBounds(day, zone);
  const dayMinutes = Math.round((dayEnd - dayStart) / MINUTE_MS);
  const items = sortOccurrences(
    occurrences.filter(
      (occurrence) => !isBanner(occurrence) && occursOn(occurrence, day, zone)
    )
  ).map((occurrence) => {
    const top =
      occurrence.start <= dayStart ? 0 : minutesInDay(occurrence.start, zone);
    const bottom =
      occurrence.end >= dayEnd
        ? dayMinutes
        : Math.max(
            top,
            top +
              Math.round(
                (occurrence.end - Math.max(occurrence.start, dayStart)) /
                  MINUTE_MS
              )
          );
    return {
      bottom: Math.min(dayMinutes, Math.max(bottom, top + MIN_MINUTES)),
      continuesAfter: occurrence.end > dayEnd,
      continuesBefore: occurrence.start < dayStart,
      occurrence,
      top,
    };
  });

  const placed: PlacedEvent[] = [];
  let cluster: (typeof items)[number][] = [];
  let columnsEnd: number[] = [];
  let clusterEnd = -1;
  const assigned = new Map<string, number>();

  const flush = (): void => {
    const columns = Math.max(1, columnsEnd.length);
    for (const item of cluster) {
      placed.push({
        ...item,
        column: assigned.get(item.occurrence.id) ?? 0,
        columns,
      });
    }
    cluster = [];
    columnsEnd = [];
    clusterEnd = -1;
  };

  for (const item of items) {
    if (cluster.length > 0 && item.top >= clusterEnd) {
      flush();
    }
    let column = columnsEnd.findIndex((end) => end <= item.top);
    if (column === -1) {
      column = columnsEnd.length;
      columnsEnd.push(item.bottom);
    } else {
      columnsEnd[column] = item.bottom;
    }
    assigned.set(item.occurrence.id, column);
    cluster.push(item);
    clusterEnd = Math.max(clusterEnd, item.bottom);
  }
  flush();
  return placed;
};

/** An all-day bar across some days of a week row. */
export interface Banner {
  readonly occurrence: Occurrence;
  /** Index of the first and last day it covers in the row (inclusive). */
  readonly from: number;
  readonly to: number;
  readonly row: number;
  readonly continuesBefore: boolean;
  readonly continuesAfter: boolean;
}

/** Packs banners (and, in the month view, every event) into rows. */
export const placeBanners = (
  occurrences: readonly Occurrence[],
  days: readonly CalendarDate[],
  zone: string,
  include: (occurrence: Occurrence) => boolean = isBanner
): Banner[] => {
  const rows: boolean[][] = [];
  const banners: Banner[] = [];
  for (const occurrence of sortOccurrences(occurrences.filter(include))) {
    const span = spanOf(occurrence);
    const covered = days
      .map((day, index) => (spanTouches(span, day, zone) ? index : -1))
      .filter((index) => index >= 0);
    const [from] = covered;
    const to = covered.at(-1);
    if (from === undefined || to === undefined) {
      continue;
    }
    let row = rows.findIndex((used) =>
      used.slice(from, to + 1).every((taken) => !taken)
    );
    if (row === -1) {
      row = rows.length;
      rows.push(days.map(() => false));
    }
    const used = rows[row];
    for (let index = from; index <= to; index += 1) {
      if (used !== undefined) {
        used[index] = true;
      }
    }
    const firstDay = days[from];
    const lastDay = days[to];
    banners.push({
      continuesAfter:
        lastDay !== undefined &&
        spanTouches(span, lastDay.add({ days: 1 }), zone),
      continuesBefore:
        firstDay !== undefined &&
        spanTouches(span, firstDay.subtract({ days: 1 }), zone),
      from,
      occurrence,
      row,
      to,
    });
  }
  return banners;
};

/** The agenda: days with events, each with its events in order. */
export const agendaDays = (
  occurrences: readonly Occurrence[],
  range: DateRange,
  zone: string
): { readonly day: CalendarDate; readonly events: Occurrence[] }[] => {
  const spans = occurrences.map((occurrence) => ({
    occurrence,
    span: spanOf(occurrence),
  }));
  return daysOf(range)
    .map((day) => ({
      day,
      events: sortOccurrences(
        spans
          .filter(({ span }) => spanTouches(span, day, zone))
          .map(({ occurrence }) => occurrence)
      ),
    }))
    .filter((entry) => entry.events.length > 0);
};

/** The day an instant falls on, for "today" and the now line. */
export const dayOf = (ms: number, zone: string): CalendarDate =>
  dateInZone(ms, zone);

/** Steps of a range drawn on the week or day view, in minutes. */
export const DRAW_STEP = 15;

/**
 * The [from, to) wall minutes of a range drawn from `anchor` to `current`
 * (either way up), in whole steps, never empty, within the day.
 */
export const drawnRange = (
  anchor: number,
  current: number
): { readonly from: number; readonly to: number } => {
  const low = Math.min(anchor, current);
  const high = Math.max(anchor, current);
  const from = Math.min(
    DAY_MINUTES - DRAW_STEP,
    Math.max(0, Math.floor(low / DRAW_STEP) * DRAW_STEP)
  );
  const to = Math.min(
    DAY_MINUTES,
    Math.max(from + DRAW_STEP, Math.ceil(high / DRAW_STEP) * DRAW_STEP)
  );
  return { from, to };
};

/** "2026-10-26T09:30" for a wall minute of a day (1440 is the next day). */
export const localAt = (day: CalendarDate, minutes: number): string => {
  const next = minutes >= DAY_MINUTES;
  const date = next ? day.add({ days: 1 }) : day;
  const rest = next ? minutes - DAY_MINUTES : minutes;
  const hours = String(Math.floor(rest / 60)).padStart(2, "0");
  return `${date.toString()}T${hours}:${String(rest % 60).padStart(2, "0")}`;
};
