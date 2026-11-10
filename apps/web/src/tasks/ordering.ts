// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Manual order of a list: dragging a task, or Alt+↑/↓,
// among the tasks of its level (same parent). Stored as
// X-APPLE-SORT-ORDER in `iCalendar.properties`, what Apple Reminders and
// other CalDAV clients keep (lower first; tasks without one go last, by
// creation). Pure, with tests.
//
// Moving a task writes as little as it can: just its own order when there
// is room between its new neighbours, else the whole level again.

import type { CalendarEventId } from "@nebulamail/jmap-client";

import type { Task } from "./model";

/** The gap between renumbered tasks (room for later moves). */
export const ORDER_STEP = 1024;

type Ordered = Pick<Task, "baseId" | "sortOrder">;

/**
 * New sort orders when the task at `from` moves to `to` (indexes of
 * `siblings`, in the order on screen). Only the tasks whose order changes,
 * by stored task (`baseId`). Empty when nothing moves.
 */
export const reorder = (
  siblings: readonly Ordered[],
  from: number,
  to: number
): Map<CalendarEventId, number> => {
  const moving = siblings[from];
  const target = Math.max(0, Math.min(siblings.length - 1, to));
  if (moving === undefined || target === from) {
    return new Map();
  }
  const rest = siblings.filter((_, index) => index !== from);
  const placed = [...rest.slice(0, target), moving, ...rest.slice(target)];
  const before = placed[target - 1];
  const after = placed[target + 1];
  const prev = before?.sortOrder ?? null;
  const next = after?.sortOrder ?? null;
  // Tasks without an order sort last, so anything after a number is fine.
  if (before === undefined && next !== null) {
    return new Map([[moving.baseId, next - ORDER_STEP]]);
  }
  if (prev !== null && after === undefined) {
    return new Map([[moving.baseId, prev + ORDER_STEP]]);
  }
  if (prev !== null && next === null) {
    return new Map([[moving.baseId, prev + ORDER_STEP]]);
  }
  if (prev !== null && next !== null && next - prev >= 2) {
    return new Map([[moving.baseId, Math.floor((prev + next) / 2)]]);
  }
  // No room (or no numbers yet): number the whole level.
  const changes = new Map<CalendarEventId, number>();
  for (const [index, task] of placed.entries()) {
    const order = (index + 1) * ORDER_STEP;
    if (task.sortOrder !== order) {
      changes.set(task.baseId, order);
    }
  }
  return changes;
};

/** Alt+↑ / Alt+↓: one place up or down; null at the edge. */
export const stepTarget = (
  from: number,
  step: -1 | 1,
  count: number
): number | null => {
  const to = from + step;
  return to < 0 || to >= count ? null : to;
};

/**
 * Where a dragged task lands among its siblings, from the pointer's
 * height: before the first sibling whose middle is below it. `middles`
 * are the siblings' centres on screen (the dragged one included, at
 * `from`); answers an index of the siblings.
 */
export const dropIndex = (
  middles: readonly number[],
  from: number,
  y: number
): number => {
  const above = middles.filter(
    (middle, index) => index !== from && middle < y
  ).length;
  return above;
};
