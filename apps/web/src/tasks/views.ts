// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Tasks views: Today (overdue and due today),
// Upcoming (the next 30 days, by day), All, Completed and one list. Pure,
// with tests: the zone and "now" are parameters.
//
// A repeating task arrives as its occurrences (the server expands them).
// Today and All show one per series, the earliest not completed (the one
// to do next; completing it brings the next one), Upcoming shows each,
// Completed shows the ones completed.

import { dateInZone } from "../calendar/time";
import type { Task, TaskPriority } from "./model";
import { PRIORITY_RANK } from "./model";

export type TaskSort = "priority" | "due" | "manual";

export interface TaskNode {
  readonly task: Task;
  /** Subtasks (in the same view), sorted the same way. */
  readonly children: readonly TaskNode[];
}

export interface DayGroup {
  /** "YYYY-MM-DD" in the user's zone. */
  readonly day: string;
  readonly tasks: readonly TaskNode[];
}

export const UPCOMING_DAYS = 30;

const byNumber = (a: number | null, b: number | null): number => {
  if (a === b) {
    return 0;
  }
  if (a === null) {
    return 1;
  }
  return b === null ? -1 : a - b;
};

const byPriority = (a: TaskPriority, b: TaskPriority): number =>
  PRIORITY_RANK[a] - PRIORITY_RANK[b];

const byTitle = (a: Task, b: Task): number =>
  a.title.localeCompare(b.title) || a.id.localeCompare(b.id);

/** The order of tasks in a view. Ties end with the title, then the id. */
export const compareTasks =
  (sort: TaskSort) =>
  (a: Task, b: Task): number => {
    switch (sort) {
      case "priority": {
        return (
          byPriority(a.priority, b.priority) ||
          byNumber(a.dueAt, b.dueAt) ||
          byNumber(a.sortOrder, b.sortOrder) ||
          byTitle(a, b)
        );
      }
      case "due": {
        return (
          byNumber(a.dueAt, b.dueAt) ||
          byPriority(a.priority, b.priority) ||
          byNumber(a.sortOrder, b.sortOrder) ||
          byTitle(a, b)
        );
      }
      case "manual": {
        return (
          byNumber(a.sortOrder, b.sortOrder) ||
          byNumber(a.created, b.created) ||
          byTitle(a, b)
        );
      }
      default: {
        return byTitle(a, b);
      }
    }
  };

/** How deep subtasks nest at most (a loop in `relatedTo` stops here). */
const MAX_DEPTH = 8;

/**
 * Tasks as a tree: a task whose parent (by UID) is among `tasks` goes
 * under it; the others are at the top (also subtasks whose parent is in
 * another view). Each level is sorted. A loop of parents can't hide tasks:
 * they come out at the top.
 */
export const nestTasks = (
  tasks: readonly Task[],
  sort: TaskSort
): TaskNode[] => {
  const compare = compareTasks(sort);
  const uids = new Set(
    tasks.flatMap((task) => (task.uid === null ? [] : [task.uid]))
  );
  const children = new Map<string, Task[]>();
  const top: Task[] = [];
  for (const task of tasks) {
    const parent = task.parentUid;
    if (parent !== null && parent !== task.uid && uids.has(parent)) {
      const siblings = children.get(parent);
      if (siblings === undefined) {
        children.set(parent, [task]);
      } else {
        siblings.push(task);
      }
    } else {
      top.push(task);
    }
  }
  const placed = new Set<string>();
  const build = (task: Task, depth: number): TaskNode => {
    placed.add(task.id);
    const kids =
      depth >= MAX_DEPTH || task.uid === null
        ? []
        : (children.get(task.uid) ?? [])
            .filter((child) => !placed.has(child.id))
            .toSorted(compare);
    return {
      children: kids.map((child) => build(child, depth + 1)),
      task,
    };
  };
  const tree = top.toSorted(compare).map((task) => build(task, 0));
  // In a loop (a → b → a) nobody is at the top: show them anyway, each once.
  for (const task of tasks.toSorted(compare)) {
    if (!placed.has(task.id)) {
      tree.push(build(task, 0));
    }
  }
  return tree;
};

/** "YYYY-MM-DD" a task is due, in the user's zone (null: no due). */
export const dueDay = (
  task: Pick<Task, "due" | "dueAt">,
  zone: string
): string | null => {
  if (task.due?.kind === "date") {
    return task.due.date;
  }
  return task.dueAt === null ? null : dateInZone(task.dueAt, zone).toString();
};

/**
 * One task per repeating series: the earliest occurrence not completed
 * (by due); a series with every occurrence completed shows none.
 */
export const nextOccurrences = (tasks: readonly Task[]): Task[] => {
  const single: Task[] = [];
  const next = new Map<string, Task>();
  for (const task of tasks) {
    if (!task.recurring) {
      single.push(task);
      continue;
    }
    if (task.completed) {
      continue;
    }
    const key = `${task.accountId}/${task.baseId}`;
    const current = next.get(key);
    if (current === undefined || byNumber(task.dueAt, current.dueAt) < 0) {
      next.set(key, task);
    }
  }
  return [...single, ...next.values()];
};

export interface TodayView {
  /** Due before today, not completed. */
  readonly overdue: readonly TaskNode[];
  /** Due today, not completed. */
  readonly today: readonly TaskNode[];
}

/** The last Today worked out (the sidebar's count asks on every render). */
let lastToday: {
  readonly tasks: readonly Task[];
  readonly today: string;
  readonly zone: string;
  readonly sort: TaskSort;
  readonly view: TodayView;
} | null = null;

/** Today: overdue and due today, not completed. */
export const todayView = (
  tasks: readonly Task[],
  now: number,
  zone: string,
  sort: TaskSort = "priority"
): TodayView => {
  const today = dateInZone(now, zone).toString();
  if (
    lastToday?.tasks === tasks &&
    lastToday.today === today &&
    lastToday.zone === zone &&
    lastToday.sort === sort
  ) {
    return lastToday.view;
  }
  const overdue: Task[] = [];
  const dueToday: Task[] = [];
  for (const task of nextOccurrences(tasks)) {
    if (task.completed) {
      continue;
    }
    const due = dueDay(task, zone);
    if (due === today) {
      dueToday.push(task);
    } else if (due !== null && due < today) {
      overdue.push(task);
    }
  }
  const view = {
    overdue: nestTasks(overdue, sort),
    today: nestTasks(dueToday, sort),
  };
  lastToday = { sort, tasks, today, view, zone };
  return view;
};

/** How many rows Today shows at its top level (the sidebar's count). */
export const todayCount = (
  tasks: readonly Task[],
  now: number,
  zone: string
): number => {
  const view = todayView(tasks, now, zone);
  return view.overdue.length + view.today.length;
};

/**
 * Upcoming: not completed, due from tomorrow to `days` days ahead, by day
 * (days without tasks are left out). Each occurrence of a repeating task.
 */
export const upcomingView = (
  tasks: readonly Task[],
  now: number,
  zone: string,
  days = UPCOMING_DAYS,
  sort: TaskSort = "priority"
): DayGroup[] => {
  const today = dateInZone(now, zone);
  const first = today.add({ days: 1 }).toString();
  const last = today.add({ days }).toString();
  const byDay = new Map<string, Task[]>();
  for (const task of tasks) {
    const day = dueDay(task, zone);
    if (task.completed || day === null || day < first || day > last) {
      continue;
    }
    const list = byDay.get(day);
    if (list === undefined) {
      byDay.set(day, [task]);
    } else {
      list.push(task);
    }
  }
  return [...byDay.entries()]
    .toSorted(([a], [b]) => a.localeCompare(b))
    .map(([day, list]) => ({ day, tasks: nestTasks(list, sort) }));
};

/** All: every task not completed (the next occurrence of a series). */
export const allView = (
  tasks: readonly Task[],
  sort: TaskSort = "due"
): TaskNode[] =>
  nestTasks(
    nextOccurrences(tasks).filter((task) => !task.completed),
    sort
  );

/** Completed: newest first (also completed occurrences), flat. */
export const completedView = (tasks: readonly Task[]): Task[] =>
  tasks
    .filter((task) => task.completed)
    .toSorted(
      (a, b) => byNumber(b.completedAt, a.completedAt) || byTitle(a, b)
    );

/** One list: its tasks not completed, as All. */
export const listView = (
  tasks: readonly Task[],
  listKey: string,
  sort: TaskSort = "manual"
): TaskNode[] =>
  allView(
    tasks.filter((task) => task.listKey === listKey),
    sort
  );

/** Open (not completed) tasks per list key, a series counting once. */
export const openCounts = (tasks: readonly Task[]): Map<string, number> => {
  const counts = new Map<string, number>();
  for (const task of nextOccurrences(tasks)) {
    if (!task.completed && task.listKey !== null) {
      counts.set(task.listKey, (counts.get(task.listKey) ?? 0) + 1);
    }
  }
  return counts;
};
