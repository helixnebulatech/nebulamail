// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Paging `/query` results and splitting expanded queries by what the
// server allows. Pure, with tests (paging.test.ts).
//
// - RFC 8620 §5.5: the server may cap `limit` (it then returns the limit it
//   used) and its `ids` run "until it hits the end of the results or
//   reaches the limit number of ids". So a page shorter than the requested
//   limit is not the end when the server applied a smaller one.
// - JMAP for Calendars (draft-ietf-jmap-calendars §5.11):
//   `maxExpandedQueryDuration` caps `before` − `after` when
//   `expandRecurrences` is true (`expandDurationTooLarge`).

import type { CalendarDate } from "@internationalized/date";
import type { AccountId, JmapClient } from "@nebulamail/jmap-client";
import { CAPABILITIES } from "@nebulamail/jmap-client";

import { parseDuration } from "./time";

/** What a `/query` response says about its page. */
export interface QueryPage {
  readonly position: number;
  readonly ids: readonly unknown[];
  readonly total?: number | undefined;
  readonly limit?: number | undefined;
}

/**
 * Where the next page starts; null if this one was the last. With a
 * `total`, until it; without, a page shorter than the limit the server
 * applied (its `limit`, else the one asked) is the last.
 */
export const nextPosition = (page: QueryPage, asked: number): number | null => {
  if (page.ids.length === 0) {
    return null;
  }
  const next = page.position + page.ids.length;
  if (page.total !== undefined) {
    return next < page.total ? next : null;
  }
  return page.ids.length < (page.limit ?? asked) ? null : next;
};

/**
 * Whole days in a `maxExpandedQueryDuration` (null: none or unreadable).
 * One day less than it says: the windows are wall-clock days in the
 * user's zone, and a DST change makes one of them an hour longer.
 */
export const expandDays = (duration: unknown): number | null => {
  const parsed = typeof duration === "string" ? parseDuration(duration) : null;
  if (parsed === null || parsed.years !== 0 || parsed.months !== 0) {
    return null;
  }
  const hours =
    (parsed.weeks * 7 + parsed.days) * 24 +
    parsed.hours +
    parsed.minutes / 60 +
    parsed.seconds / 3600;
  const days = Math.floor(hours / 24) - 1;
  return days >= 1 ? days : null;
};

/** The account's `maxExpandedQueryDuration` in days (null: no limit known). */
export const maxExpandDays = (
  client: JmapClient,
  accountId: AccountId
): number | null => {
  const capability: unknown =
    client.session.accounts[accountId]?.accountCapabilities[
      CAPABILITIES.calendars
    ];
  return typeof capability === "object" && capability !== null
    ? expandDays(Reflect.get(capability, "maxExpandedQueryDuration"))
    : null;
};

/** [start, end) in windows of at most `days` days (all of it if null). */
export const expandWindows = (
  start: CalendarDate,
  end: CalendarDate,
  days: number | null
): (readonly [CalendarDate, CalendarDate])[] => {
  if (days === null || start.compare(end) >= 0) {
    return [[start, end]];
  }
  const windows: (readonly [CalendarDate, CalendarDate])[] = [];
  for (let from = start; from.compare(end) < 0; from = from.add({ days })) {
    const to = from.add({ days });
    windows.push([from, to.compare(end) < 0 ? to : end]);
  }
  return windows;
};
