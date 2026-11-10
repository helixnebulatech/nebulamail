// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the Tasks app shows: a view or one list, and the
// open task. Pure, with tests.

import type { Task, TaskList } from "./model";
import type { DayGroup, TaskNode, TaskSort } from "./views";
import {
  allView,
  completedView,
  listView,
  todayView,
  upcomingView,
} from "./views";

export const TASK_VIEWS = ["today", "upcoming", "all", "done"] as const;
export type TaskViewName = (typeof TASK_VIEWS)[number];

/** A view, or one list (by its key). */
export type TaskScope =
  | { readonly kind: "view"; readonly view: TaskViewName }
  | { readonly kind: "list"; readonly key: string };

/** A titled group of the list on screen. */
export interface TaskSection {
  readonly id: string;
  /** Null: no heading (All, a list). */
  readonly heading:
    | { readonly kind: "overdue" }
    | { readonly kind: "today" }
    | { readonly kind: "day"; readonly day: string }
    | { readonly kind: "completed" }
    | null;
  readonly nodes: readonly TaskNode[];
  /** Done tasks (struck through, no subtasks under them). */
  readonly done?: boolean;
}

const flat = (tasks: readonly Task[]): TaskNode[] =>
  tasks.map((task) => ({ children: [], task }));

/** Words that match a task (title and notes), accents and case aside. */
const fold = (text: string): string =>
  text
    .normalize("NFD")
    .replaceAll(/\p{Diacritic}/gu, "")
    .toLowerCase();

/**
 * Folded title and notes of each task, kept while the task object lives:
 * a search folds them once, not once per keystroke (tasks are immutable;
 * a changed task is a new object).
 */
const foldedText = new WeakMap<Task, string>();

const searchText = (task: Task): string => {
  let text = foldedText.get(task);
  if (text === undefined) {
    text = fold(`${task.title} ${task.notes ?? ""}`);
    foldedText.set(task, text);
  }
  return text;
};

/** Only the tasks whose title or notes have every word of `query`. */
export const searchTasks = (
  tasks: readonly Task[],
  query: string
): readonly Task[] => {
  const words = fold(query).split(/\s+/u).filter(Boolean);
  if (words.length === 0) {
    return tasks;
  }
  return tasks.filter((task) => {
    const text = searchText(task);
    return words.every((word) => text.includes(word));
  });
};

export interface SectionOptions {
  readonly now: number;
  readonly zone: string;
  /** In a list: also its done tasks, in their own section. */
  readonly showDone: boolean;
  readonly sort?: TaskSort;
}

/** The sections of a view or list, in order (empty ones left out). */
export const sectionsOf = (
  scope: TaskScope,
  tasks: readonly Task[],
  options: SectionOptions
): TaskSection[] => {
  if (scope.kind === "list") {
    const open = listView(tasks, scope.key, options.sort);
    const done = options.showDone
      ? completedView(tasks.filter((task) => task.listKey === scope.key))
      : [];
    return [
      { heading: null, id: "open", nodes: open },
      ...(done.length === 0
        ? []
        : [
            {
              done: true,
              heading: { kind: "completed" as const },
              id: "done",
              nodes: flat(done),
            },
          ]),
    ];
  }
  switch (scope.view) {
    case "today": {
      const view = todayView(tasks, options.now, options.zone, options.sort);
      return [
        {
          heading: { kind: "overdue" as const },
          id: "overdue",
          nodes: view.overdue,
        },
        { heading: { kind: "today" as const }, id: "today", nodes: view.today },
      ].filter((section) => section.nodes.length > 0);
    }
    case "upcoming": {
      return upcomingView(
        tasks,
        options.now,
        options.zone,
        undefined,
        options.sort
      ).map((group: DayGroup) => ({
        heading: { day: group.day, kind: "day" as const },
        id: group.day,
        nodes: group.tasks,
      }));
    }
    case "all": {
      return [
        { heading: null, id: "all", nodes: allView(tasks, options.sort) },
      ];
    }
    case "done": {
      return [
        {
          done: true,
          heading: null,
          id: "done",
          nodes: flat(completedView(tasks)),
        },
      ];
    }
    default: {
      return scope.view satisfies never;
    }
  }
};

/**
 * Where a task typed in the quick add goes: Today → due today, Upcoming →
 * due tomorrow, a list → that list; else the default list.
 */
export const quickAddTarget = (
  scope: TaskScope,
  lists: readonly TaskList[],
  fallback: string | null
): {
  readonly listKey: string | null;
  readonly due: "today" | "tomorrow" | null;
} => {
  if (scope.kind === "list") {
    const list = lists.find((item) => item.key === scope.key);
    return {
      due: null,
      listKey: list?.mayWrite === true ? list.key : fallback,
    };
  }
  if (scope.view === "today") {
    return { due: "today", listKey: fallback };
  }
  if (scope.view === "upcoming") {
    return { due: "tomorrow", listKey: fallback };
  }
  return { due: null, listKey: fallback };
};
