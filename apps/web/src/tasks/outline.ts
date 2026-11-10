// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The task list as rows on screen, and where Tab / Shift+Tab move a task
// (subtasks are made with Tab and Shift+Tab, like in
// Apple Reminders or Things). Pure, with tests.
//
// Tab makes a task a subtask of the task just above it at the same level
// (its previous sibling); Shift+Tab takes it one level up (it becomes a
// sibling of its parent). A task's own subtasks move with it: the
// relation is stored on each child. Only within one list of one account
// (a parent in another list would hide the subtask from that list).

import type { Task } from "./model";
import type { TaskNode } from "./views";

export interface OutlineRow {
  readonly task: Task;
  /** 0 at the top. */
  readonly depth: number;
  /** The task it hangs from in this view (null at the top). */
  readonly parent: Task | null;
  /** How many subtasks (in this view) it has. */
  readonly children: number;
}

/** The rows of a tree, depth first (the order on screen). */
export const outlineRows = (
  nodes: readonly TaskNode[],
  depth = 0,
  parent: Task | null = null
): OutlineRow[] =>
  nodes.flatMap((node) => [
    { children: node.children.length, depth, parent, task: node.task },
    ...outlineRows(node.children, depth + 1, node.task),
  ]);

const sameList = (a: Task, b: Task): boolean =>
  a.accountId === b.accountId && a.listKey === b.listKey;

/**
 * The task `rows[index]` would become a subtask of with Tab: the closest
 * row above at the same depth with the same parent. Null if there is none
 * (first at its level), it's in another list, or it has no UID.
 */
export const indentTarget = (
  rows: readonly OutlineRow[],
  index: number
): Task | null => {
  const row = rows[index];
  if (row === undefined) {
    return null;
  }
  for (let above = index - 1; above >= 0; above -= 1) {
    const candidate = rows[above];
    if (candidate === undefined || candidate.depth < row.depth) {
      return null;
    }
    if (candidate.depth === row.depth) {
      return candidate.task.uid !== null &&
        candidate.task.uid !== row.task.uid &&
        sameList(candidate.task, row.task)
        ? candidate.task
        : null;
    }
  }
  return null;
};

/**
 * The parent `rows[index]` would get with Shift+Tab: its parent's parent
 * (null: the top). Undefined if it is already at the top.
 */
export const outdentTarget = (
  rows: readonly OutlineRow[],
  index: number
): string | null | undefined => {
  const parent = rows[index]?.parent;
  if (parent === null || parent === undefined) {
    return undefined;
  }
  return parent.parentUid;
};

const byUidCache = new WeakMap<readonly Task[], ReadonlyMap<string, Task>>();

/**
 * Tasks by UID (the last one wins, as a Map built in order). Built once
 * per tasks array: a drag asks for every pointer move.
 */
const tasksByUid = (tasks: readonly Task[]): ReadonlyMap<string, Task> => {
  const cached = byUidCache.get(tasks);
  if (cached !== undefined) {
    return cached;
  }
  const byUid = new Map<string, Task>();
  for (const item of tasks) {
    if (item.uid !== null) {
      byUid.set(item.uid, item);
    }
  }
  byUidCache.set(tasks, byUid);
  return byUid;
};

/** Whether `uid` is `task` or one of its subtasks (no loops allowed). */
export const isWithin = (
  tasks: readonly Task[],
  task: Task,
  uid: string
): boolean => {
  if (task.uid === uid) {
    return true;
  }
  const byUid = tasksByUid(tasks);
  let current = byUid.get(uid);
  // The depth of `nestTasks` and a little more: a loop stops here.
  for (let step = 0; step < 16 && current !== undefined; step += 1) {
    if (current.parentUid === task.uid) {
      return true;
    }
    current =
      current.parentUid === null ? undefined : byUid.get(current.parentUid);
  }
  return false;
};

/** `task` and every subtask under it (by UID, same account). */
export const withSubtasks = (tasks: readonly Task[], task: Task): Task[] => {
  const out: Task[] = [task];
  const seen = new Set([task.id]);
  // `out` grows while it is walked: each new one's subtasks come too.
  for (const parent of out) {
    const { uid } = parent;
    if (uid === null) {
      continue;
    }
    for (const child of tasks) {
      if (
        child.parentUid === uid &&
        child.accountId === task.accountId &&
        !seen.has(child.id)
      ) {
        seen.add(child.id);
        out.push(child);
      }
    }
  }
  return out;
};
