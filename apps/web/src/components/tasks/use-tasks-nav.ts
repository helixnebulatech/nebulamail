// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Tasks app's state in the URL: `/tasks?view=…` or
// `?list=…`, and `&task=…` for the open task. Reload, back and links work.

import { useQuery } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";

import { calendarKey } from "../../calendar/model";
import { ownTasksAccount } from "../../tasks/api";
import { taskKeys } from "../../tasks/hooks";
import { parseRefParam, refParam } from "../../tasks/links";
import type { Task } from "../../tasks/model";
import type { TaskScope, TaskViewName } from "../../tasks/nav";

const route = getRouteApi("/tasks");

export interface TaskRef {
  readonly accountId: string;
  readonly id: string;
}

export interface TasksNav {
  readonly scope: TaskScope;
  /** The open task (null: none). */
  readonly open: TaskRef | null;
  /** The user's own account (null while it loads). */
  readonly own: string | null;
  readonly setView: (view: TaskViewName) => void;
  readonly setList: (key: string) => void;
  readonly openTask: (task: Pick<Task, "accountId" | "id"> | null) => void;
}

/** The user's own calendars account (for ids in URLs). */
export const useOwnTasksAccount = (): string | null =>
  useQuery({
    queryFn: ownTasksAccount,
    queryKey: [...taskKeys.all, "own-account"],
    staleTime: Infinity,
  }).data ?? null;

/** "account/calendar" ↔ the `list` param. */
const listParam = (key: string, own: string | null): string => {
  const [accountId = "", id = ""] = key.split("/");
  return refParam(accountId, id, own);
};

export const useTasksNav = (): TasksNav => {
  const search = route.useSearch();
  const navigate = useNavigate();
  const own = useOwnTasksAccount();
  const list = parseRefParam(search.list, own);
  const open = parseRefParam(search.task, own);
  const scope: TaskScope =
    list === null
      ? { kind: "view", view: search.view ?? "today" }
      : { key: calendarKey(list.accountId, list.id), kind: "list" };
  return {
    open,
    openTask: (task) => {
      const next =
        task === null ? undefined : refParam(task.accountId, task.id, own);
      void navigate({
        search: {
          ...(search.list === undefined ? {} : { list: search.list }),
          ...(search.view === undefined ? {} : { view: search.view }),
          ...(next === undefined ? {} : { task: next }),
        },
        to: "/tasks",
      });
    },
    own,
    scope,
    setList: (key) => {
      void navigate({
        search: { list: listParam(key, own) },
        to: "/tasks",
      });
    },
    setView: (view) => {
      void navigate({
        search: view === "today" ? {} : { view },
        to: "/tasks",
      });
    },
  };
};
