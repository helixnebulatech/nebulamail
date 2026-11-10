// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where a task dragged in the list lands. Dropped on the
// middle of another task it becomes its subtask; on its top or bottom
// edge it goes next to it, at that task's level: among its siblings it
// changes place (manual order), elsewhere it changes parent too (so a
// subtask dragged next to a top task leaves its parent). Pure, with tests.
//
// Same rules as Tab (outline.ts): only within one list of one account,
// never under itself or one of its own subtasks (no loops), never under a
// task without UID or one shown as done.

import type { PlacedRow } from "./list-model";
import type { Task } from "./model";
import { isWithin } from "./outline";

/** Which part of a row the pointer is on. */
export type DropZone = "before" | "inside" | "after";

/** The top and bottom quarter of a row are its edges; the rest, its middle. */
export const zoneAt = (top: number, height: number, y: number): DropZone => {
  const edge = height / 4;
  if (y < top + edge) {
    return "before";
  }
  if (y > top + height - edge) {
    return "after";
  }
  return "inside";
};

export type TaskDrop =
  /** It becomes a subtask of `parent` (the row under the pointer). */
  | { readonly kind: "nest"; readonly parent: PlacedRow }
  /**
   * It goes next to `target` (`at`), under `parentUid` (null: the top).
   * `siblings` are the rows of that level without the dragged one, and
   * `to` its index among them. `moves` says whether its parent changes.
   */
  | {
      readonly kind: "place";
      readonly target: PlacedRow;
      readonly at: "before" | "after";
      readonly parentUid: string | null;
      readonly siblings: readonly PlacedRow[];
      readonly to: number;
      readonly moves: boolean;
    };

const sameList = (a: Task, b: Task): boolean =>
  a.accountId === b.accountId && a.listKey === b.listKey;

/** The rows at the level of `row`: same section and parent. */
export const levelOf = (
  rows: readonly PlacedRow[],
  row: PlacedRow
): PlacedRow[] =>
  rows.filter(
    (item) =>
      item.section === row.section &&
      item.depth === row.depth &&
      (item.parent?.id ?? null) === (row.parent?.id ?? null)
  );

/** Whether `child` can become a subtask of `parent`. */
export const canNest = (
  tasks: readonly Task[],
  child: Task,
  parent: PlacedRow
): boolean =>
  parent.task.uid !== null &&
  !parent.done &&
  child.parentUid !== parent.task.uid &&
  sameList(child, parent.task) &&
  !isWithin(tasks, child, parent.task.uid);

interface DropInput {
  readonly dragged: PlacedRow;
  readonly over: PlacedRow;
  readonly zone: DropZone;
  readonly rows: readonly PlacedRow[];
  /** Every task (to find the subtasks of the dragged one). */
  readonly tasks: readonly Task[];
}

/** On an edge: next to `over`, at its level (null: nowhere new). */
const placeNext = ({
  dragged,
  over,
  rows,
  zone,
}: DropInput): TaskDrop | null => {
  if (over.section !== dragged.section || over.done !== dragged.done) {
    return null;
  }
  const parentUid = over.parent?.uid ?? null;
  if (over.parent !== null && parentUid === null) {
    return null;
  }
  const siblings = levelOf(rows, over).filter(
    (item) => item.task.id !== dragged.task.id
  );
  const to = siblings.indexOf(over) + (zone === "after" ? 1 : 0);
  const moves = (dragged.parent?.id ?? null) !== (over.parent?.id ?? null);
  // Its own place among the same siblings: nothing changes.
  if (!moves && levelOf(rows, dragged).indexOf(dragged) === to) {
    return null;
  }
  return {
    at: zone === "before" ? "before" : "after",
    kind: "place",
    moves,
    parentUid,
    siblings,
    target: over,
    to,
  };
};

/**
 * What dropping `dragged` on `zone` of `over` does; null: nothing (its own
 * row or subtasks, another list, nowhere new).
 */
export const dropOf = (input: DropInput): TaskDrop | null => {
  const { dragged, over, tasks, zone } = input;
  const { task } = dragged;
  if (
    over.task.id === task.id ||
    !sameList(task, over.task) ||
    (over.task.uid !== null && isWithin(tasks, task, over.task.uid))
  ) {
    return null;
  }
  // The bottom edge of a task with subtasks shown is above its first one:
  // dropping there makes it a subtask too.
  if (zone === "inside" || (zone === "after" && over.children > 0)) {
    return canNest(tasks, task, over) ? { kind: "nest", parent: over } : null;
  }
  return placeNext(input);
};
