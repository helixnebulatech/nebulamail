// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which calendars are task lists. Pure, with tests.
//
// Stalwart lets every calendar hold tasks (each one announces VEVENT and
// VTODO over CalDAV), so a calendar is a list when it has tasks or when
// the user made it one from Tasks (its key is kept in the synced
// preferences, `taskLists`). A server can say more per calendar (the
// bridge, behind a CalDAV server that keeps tasks apart): that goes
// through `ListHint`, so this file doesn't depend on how it is announced.
//
// "Remove from Tasks" (2026-11-01) keeps the key apart (`removedTaskLists`):
// forgetting it from `taskLists` isn't enough, a calendar with tasks would
// still be a list. A removed one is never a list, whatever its tasks or the
// server say, until the user adds it again.

import type { CalendarSummary } from "../calendar/model";
import type { TaskList } from "./model";

/**
 * What the server says about one calendar: it holds tasks (true), it
 * can't (false), or nothing (null).
 */
export type ListHint = (calendar: CalendarSummary) => boolean | null;

/** No server hints (Stalwart). */
export const noHint: ListHint = () => null;

/**
 * The bridge's word (its Calendar property of components): a calendar
 * with VTODO holds tasks, one without can't; none said: no hint.
 */
export const bridgeHint: ListHint = (calendar) =>
  calendar.components === null || calendar.components === undefined
    ? null
    : calendar.components.includes("VTODO");

export interface ListInput {
  readonly calendars: readonly CalendarSummary[];
  /** Open tasks per calendar key (`openCounts`). */
  readonly open: ReadonlyMap<string, number>;
  /** Calendars with any task, completed ones too. */
  readonly withTasks: ReadonlySet<string>;
  /** Keys the user made task lists (synced preferences). */
  readonly saved: readonly string[];
  /** Keys the user took out of Tasks (synced preferences). */
  readonly removed?: readonly string[];
  readonly hint?: ListHint;
}

/**
 * The task lists: calendars the server marks for tasks, with tasks, or
 * saved as lists; never one the server says can't hold tasks, nor one the
 * user removed. Own lists first, then by the calendar order.
 */
export const taskListsOf = (input: ListInput): TaskList[] => {
  const hint = input.hint ?? noHint;
  const saved = new Set(input.saved);
  const removed = new Set(input.removed);
  return input.calendars
    .filter((calendar) => {
      if (removed.has(calendar.key)) {
        return false;
      }
      const said = hint(calendar);
      if (said !== null) {
        return said;
      }
      return input.withTasks.has(calendar.key) || saved.has(calendar.key);
    })
    .map((calendar) => ({
      accountId: calendar.accountId,
      color: calendar.color,
      id: calendar.id,
      isDefault: calendar.isDefault,
      key: calendar.key,
      mayWrite: calendar.mayWrite,
      name: calendar.name,
      open: input.open.get(calendar.key) ?? 0,
      owner: calendar.owner,
    }));
};

/**
 * Where a new task goes: the first saved list the user can write to, else
 * the first own writable list, else null (the UI offers to create one).
 */
export const defaultListKey = (
  lists: readonly TaskList[],
  saved: readonly string[]
): string | null => {
  const writable = lists.filter((list) => list.mayWrite);
  const keys = new Set(writable.map((list) => list.key));
  const first = saved.find((key) => keys.has(key));
  return (
    first ??
    writable.find((list) => list.owner === null)?.key ??
    writable[0]?.key ??
    null
  );
};

/** At most this many saved lists (the synced document is small). */
export const MAX_SAVED_LISTS = 200;

/** `saved` with `key` added (first) or removed. */
export const withSavedList = (
  saved: readonly string[],
  key: string,
  on: boolean
): string[] => {
  const others = saved.filter((item) => item !== key);
  return (on ? [key, ...others] : others).slice(0, MAX_SAVED_LISTS);
};

/**
 * Calendars that could be shown as lists but aren't yet ("Add a
 * calendar…" in Tasks): every one that isn't a list, except those the
 * server says can't hold tasks. The user's own first, then by the
 * calendar order.
 */
export const addableCalendars = (
  calendars: readonly CalendarSummary[],
  lists: readonly Pick<TaskList, "key">[],
  hint: ListHint = bridgeHint
): CalendarSummary[] => {
  const shown = new Set(lists.map((list) => list.key));
  const addable = calendars.filter(
    (calendar) => !shown.has(calendar.key) && hint(calendar) !== false
  );
  return [
    ...addable.filter((calendar) => calendar.owner === null),
    ...addable.filter((calendar) => calendar.owner !== null),
  ];
};
