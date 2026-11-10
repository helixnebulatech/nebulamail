// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the task list shows: the sections of a view or
// list as rows on screen, the counts of subtasks and the moves of the
// keyboard. Pure, with tests.

import type { Task, TaskList } from "./model";
import type { TaskScope, TaskSection } from "./nav";
import { searchTasks, sectionsOf } from "./nav";
import type { OutlineRow } from "./outline";
import { outlineRows } from "./outline";

/** A row on screen with its section (Tab moves only inside a section). */
export interface PlacedRow extends OutlineRow {
  readonly section: string;
  /** Its index among its section's rows. */
  readonly index: number;
  /** Shown as done. */
  readonly done: boolean;
}

export interface ListModelInput {
  readonly tasks: readonly Task[];
  readonly scope: TaskScope;
  /** Ticked here: they stay where they were until the view changes. */
  readonly kept: ReadonlySet<string>;
  readonly search: string;
  /** A list's done tasks too. */
  readonly showDone: boolean;
  readonly now: number;
  readonly zone: string;
}

export interface ListModel {
  readonly sections: readonly TaskSection[];
  readonly rows: readonly PlacedRow[];
  /** Each section's rows (for Tab / Shift+Tab). */
  readonly bySection: ReadonlyMap<string, readonly OutlineRow[]>;
  /** Each section's rows on screen (`rows` of that section, in order). */
  readonly placedBySection: ReadonlyMap<string, readonly PlacedRow[]>;
}

export const listModel = (input: ListModelInput): ListModel => {
  const shown = searchTasks(
    input.tasks.map((task) =>
      input.kept.has(task.id) && task.completed
        ? { ...task, completed: false }
        : task
    ),
    input.search
  );
  const done = new Set(
    input.tasks.filter((task) => task.completed).map((task) => task.id)
  );
  const sections = sectionsOf(input.scope, shown, {
    now: input.now,
    showDone: input.showDone || input.search.trim() !== "",
    zone: input.zone,
  });
  const bySection = new Map(
    sections.map((section) => [section.id, outlineRows(section.nodes)])
  );
  const placedBySection = new Map(
    sections.map((section) => [
      section.id,
      (bySection.get(section.id) ?? []).map((row, index): PlacedRow => ({
        ...row,
        done: section.done === true || done.has(row.task.id),
        index,
        section: section.id,
      })),
    ])
  );
  const rows = sections.flatMap(
    (section) => placedBySection.get(section.id) ?? []
  );
  return { bySection, placedBySection, rows, sections };
};

export interface SubtaskCount {
  readonly done: number;
  readonly all: number;
}

/** Subtasks done / all per parent (`account/uid`); a series counts once. */
export const subtaskCounts = (
  tasks: readonly Task[]
): Map<string, SubtaskCount> => {
  const counts = new Map<string, SubtaskCount>();
  const counted = new Set<string>();
  for (const task of tasks) {
    const key = `${task.accountId}/${task.baseId}`;
    if (task.parentUid !== null && !counted.has(key)) {
      counted.add(key);
      const parent = `${task.accountId}/${task.parentUid}`;
      const count = counts.get(parent) ?? { all: 0, done: 0 };
      counts.set(parent, {
        all: count.all + 1,
        done: count.done + (task.completed ? 1 : 0),
      });
    }
  }
  return counts;
};

/**
 * The row j/k (step +1/−1) goes to from `current` (−1: none yet, which
 * starts at the first or the last).
 */
export const stepIndex = (
  current: number,
  step: number,
  length: number
): number => {
  if (length === 0) {
    return -1;
  }
  if (current === -1) {
    return step > 0 ? 0 : length - 1;
  }
  return Math.max(0, Math.min(length - 1, current + step));
};

/**
 * Where a new task goes when there is no list yet: the user's default
 * writable calendar (or their first one).
 */
export const fallbackList = (
  defaultKey: string | null,
  calendars: readonly {
    readonly key: string;
    readonly owner: string | null;
    readonly mayWrite: boolean;
    readonly isDefault: boolean;
  }[]
): string | null => {
  if (defaultKey !== null) {
    return defaultKey;
  }
  const own = calendars.filter(
    (calendar) => calendar.owner === null && calendar.mayWrite
  );
  return (own.find((calendar) => calendar.isDefault) ?? own[0])?.key ?? null;
};

/** Lists by key. */
export const listsByKey = (
  lists: readonly TaskList[]
): ReadonlyMap<string, TaskList> =>
  new Map(lists.map((list) => [list.key, list]));
