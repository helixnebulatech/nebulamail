// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// React hooks over the Tasks domain functions (TanStack Query). Only in
// the browser: the access token lives in memory, never during SSR.
//
// Every change goes through `useTaskChange`: it runs one TaskChange and
// answers the change that undoes it (for an "Undo" toast), worked out from
// the stored tasks before the change.

import type { CalendarEventId } from "@nebulamail/jmap-client";
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { listOwnAddresses, me } from "../calendar/api";
import { calendarKeys, useTimeZone } from "../calendar/hooks";
import { dateInZone } from "../calendar/time";
import { syncedOnce } from "../prefs-sync/sync";
import type { TaskSnapshot } from "./adapter";
import { storedKey } from "./adapter";
import {
  applyTaskChange,
  assignableAccount,
  createTaskList,
  loadTasks,
  rememberTaskList,
  removeTaskList,
  tasksAvailable,
} from "./api";
import { scheduleChange } from "./assignment";
import type { TaskReplyTexts } from "./assignment-api";
import { sendTaskReply } from "./assignment-api";
import type { TaskChange } from "./editing";
import { completeChange, undoOf } from "./editing";
import type { RawTask, Task } from "./model";
import type { TasksPush } from "./push";
import { watchTasks } from "./push";

export const taskKeys = {
  all: ["tasks"] as const,
  /** Assigned-task cards in mail (assignment-card.tsx). */
  assignments: () => [...taskKeys.all, "assignment"] as const,
  available: () => [...taskKeys.all, "available"] as const,
  snapshot: (zone: string, day: string) =>
    [...taskKeys.snapshots(), zone, day] as const,
  snapshots: () => [...taskKeys.all, "snapshot"] as const,
};

/** Whether the server can keep tasks (undefined while it loads). */
export const useTasksAvailable = (): boolean | undefined =>
  useQuery({
    queryFn: tasksAvailable,
    queryKey: taskKeys.available(),
    staleTime: Infinity,
  }).data;

/** The account whose tasks can be assigned (null: none; loading: undefined). */
export const useAssignableAccount = () =>
  useQuery({
    queryFn: assignableAccount,
    queryKey: [...taskKeys.all, "assignable"],
    staleTime: Infinity,
  }).data;

export const todayIn = (zone: string): string =>
  dateInZone(Date.now(), zone).toString();

/** Today's date in `zone`, "YYYY-MM-DD", updated when the day changes. */
export const useToday = (zone: string): string => {
  const [day, setDay] = useState(() => todayIn(zone));
  useEffect(() => {
    // Within a minute after a change of day or zone (zones hardly change).
    const timer = setInterval(() => {
      setDay(todayIn(zone));
    }, 60_000);
    return () => {
      clearInterval(timer);
    };
  }, [zone]);
  return day;
};

/**
 * Every task of `day` (the key; refetched when it changes) shown in `zone`.
 * Also the /tasks route's loader (lib/route-prefetch.ts), with the same
 * zone and day the screen will use.
 */
export const tasksQuery = (zone: string, day: string) =>
  queryOptions({
    queryFn: async () =>
      await loadTasks({
        display: zone,
        // Lists saved on another device count (prefs-sync): synced while
        // the tasks are read, awaited only to read the lists; a failed
        // sync never blocks them.
        listsReady: syncedOnce().catch(() => null),
        now: Date.now(),
      }),
    queryKey: taskKeys.snapshot(zone, day),
  });

/**
 * Every task, the lists and what editing needs (refetched each day).
 * `enabled: false` reads nothing (the calendar with tasks hidden).
 */
export const useTasks = ({
  enabled = true,
}: { readonly enabled?: boolean } = {}) => {
  const zone = useTimeZone();
  const day = useToday(zone);
  return useQuery({
    ...tasksQuery(zone, day),
    enabled,
    placeholderData: (previous: TaskSnapshot | undefined) => previous,
  });
};

/**
 * After a change: what can show a task. The calendar shows tasks from the
 * same snapshot (calendar-tasks.tsx), so only an open event and assigned
 * tasks' cards in mail are read again; `calendars` too when a list (a
 * calendar) was created or removed. The rest under `taskKeys.all`
 * (capabilities, the own account) doesn't change with tasks.
 */
export const refreshTasks = async (
  client: QueryClient,
  { calendars = false }: { readonly calendars?: boolean } = {}
): Promise<void> => {
  await Promise.all([
    client.invalidateQueries({ queryKey: taskKeys.snapshots() }),
    client.invalidateQueries({ queryKey: taskKeys.assignments() }),
    client.invalidateQueries({ queryKey: calendarKeys.events() }),
    calendars
      ? client.invalidateQueries({ queryKey: calendarKeys.calendars() })
      : null,
  ]);
};

/** How long pushes are gathered into one refresh. */
export const PUSH_COALESCE_MS = 300;

const isSnapshot = (data: unknown): data is TaskSnapshot =>
  typeof data === "object" && data !== null && "states" in data;

/**
 * Whether the snapshots shown already hold these `CalendarEvent` states:
 * the push is about a change they read (RFC 8620 §7.1, the state string
 * changes with every change and only then).
 */
const shownStates = (
  client: QueryClient,
  events: Readonly<Record<string, string>>
): boolean => {
  const shown = client
    .getQueryCache()
    .findAll({ queryKey: taskKeys.snapshots(), type: "active" })
    .map((query) => query.state.data)
    .filter((data): data is TaskSnapshot => isSnapshot(data));
  return (
    shown.length > 0 &&
    shown.every((snapshot) =>
      Object.entries(events).every(
        ([accountId, state]) => snapshot.states.get(accountId) === state
      )
    )
  );
};

/**
 * The push handler. Changes within PUSH_COALESCE_MS make one refresh;
 * while the snapshot loads, it waits for that load rather than cancel it.
 * Then the snapshot is read again only if it doesn't hold the pushed
 * states yet: the push that follows the user's own change (whose load
 * `useTaskChange` started) reads nothing more.
 */
export const pushRefresher = (
  client: QueryClient,
  wait: number = PUSH_COALESCE_MS
) => {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let calendars = false;
  let events: Record<string, string> = {};
  const flush = (): void => {
    if (client.isFetching({ queryKey: taskKeys.snapshots() }) > 0) {
      timer = setTimeout(flush, wait);
      return;
    }
    timer = null;
    const lists = calendars;
    const pushed = events;
    calendars = false;
    events = {};
    if (lists || !shownStates(client, pushed)) {
      void refreshTasks(client, { calendars: lists });
    }
  };
  return {
    cancel: (): void => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    },
    onChange: (change: TasksPush): void => {
      calendars ||= change.calendars;
      events = { ...events, ...change.events };
      timer ??= setTimeout(flush, wait);
    },
  };
};

/** The stored tasks a change touches, from the snapshot shown now. */
const storedBefore = (
  snapshot: TaskSnapshot | undefined,
  change: TaskChange
): Map<CalendarEventId, RawTask> => {
  const before = new Map<CalendarEventId, RawTask>();
  const ids = [...Object.keys(change.update ?? {}), ...(change.destroy ?? [])];
  for (const id of ids) {
    const raw = snapshot?.stored.get(storedKey(change.accountId, id));
    if (raw !== undefined) {
      before.set(raw.id, raw);
    }
  }
  return before;
};

export interface ChangeInput {
  readonly change: TaskChange;
  /** How the snapshot looks once it's done (shown at once). */
  readonly optimistic?: (snapshot: TaskSnapshot) => TaskSnapshot;
}

export interface ChangeResult {
  readonly created: Readonly<Record<string, CalendarEventId>>;
  /** The change that undoes this one. */
  readonly undo: TaskChange;
  /** The change was saved, but a REPLY to whoever assigned it was not sent. */
  readonly replyFailed: boolean;
}

/** The user's addresses and name, read once (calendar/hooks keys). */
const whoAmI = async (client: QueryClient) => {
  const [own, self] = await Promise.all([
    client.query({
      queryFn: listOwnAddresses,
      queryKey: calendarKeys.own(),
      staleTime: Infinity,
    }),
    client
      .query({
        queryFn: me,
        queryKey: calendarKeys.me(),
        staleTime: Infinity,
      })
      .catch(() => null),
  ]);
  return { name: self?.name ?? null, own };
};

const snapshotsOf = (client: QueryClient) =>
  client.getQueriesData<TaskSnapshot>({ queryKey: taskKeys.snapshots() });

/**
 * Runs a TaskChange (shown at once with `optimistic`, rolled back if the
 * server says no) and answers its undo. Assigned tasks
 * the organizer's changes notify the assignees; an
 * assignee's completing or reopening mails a REPLY (`texts` gives its
 * words; without them nothing is mailed).
 */
export const useTaskChange = (texts?: TaskReplyTexts) => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ change }: ChangeInput): Promise<ChangeResult> => {
      const current = snapshotsOf(client)
        .map(([, data]) => data)
        .find((data) => data !== undefined);
      const before = storedBefore(current, change);
      const assigned = [...before.values()].some(
        (raw) => Object.keys(raw.participants ?? {}).length > 0
      );
      const creates = Object.values(change.create ?? {}).some(
        (body) => body.participants !== undefined
      );
      const touchesAssignees =
        creates ||
        Object.values(change.update ?? {}).some(
          (patch) => patch !== undefined && "participants" in patch
        );
      const who = assigned || touchesAssignees ? await whoAmI(client) : null;
      const scheduled =
        who === null
          ? { change, replies: [] }
          : scheduleChange(change, before, who.own);
      const created = await applyTaskChange(scheduled.change);
      let replyFailed = false;
      if (who !== null && texts !== undefined) {
        for (const reply of scheduled.replies) {
          try {
            // oxlint-disable-next-line no-await-in-loop -- one email after another, usually one.
            await sendTaskReply(reply.task, reply.answer, {
              name: who.name,
              own: who.own,
              texts,
            });
          } catch {
            replyFailed = true;
          }
        }
      }
      return {
        created,
        replyFailed,
        undo: undoOf(scheduled.change, before, created),
      };
    },
    onError: (
      _error,
      _input,
      saved: { readonly list?: ReturnType<typeof snapshotsOf> } | undefined
    ) => {
      for (const [key, data] of saved?.list ?? []) {
        client.setQueryData(key, data);
      }
    },
    onMutate: async ({ optimistic }: ChangeInput) => {
      if (optimistic === undefined) {
        return {};
      }
      await client.cancelQueries({ queryKey: taskKeys.snapshots() });
      const list = snapshotsOf(client);
      for (const [key, data] of list) {
        if (data !== undefined) {
          client.setQueryData(key, optimistic(data));
        }
      }
      return { list };
    },
    onSettled: async () => {
      await refreshTasks(client);
    },
  });
};

/** The snapshot with one task (by id) replaced. */
export const withTask = (
  snapshot: TaskSnapshot,
  id: CalendarEventId,
  update: (task: Task) => Task
): TaskSnapshot => ({
  ...snapshot,
  tasks: snapshot.tasks.map((task) => (task.id === id ? update(task) : task)),
});

/**
 * Ticks a task (or one occurrence) as done or not, shown at once; answers
 * the undo. Null if its stored task isn't in the snapshot.
 */
export const useCompleteTask = (texts?: TaskReplyTexts) => {
  const change = useTaskChange(texts);
  const client = useQueryClient();
  return {
    ...change,
    complete: async (
      task: Task,
      done: boolean
    ): Promise<ChangeResult | null> => {
      const snapshot = snapshotsOf(client)
        .map(([, data]) => data)
        .find((data) => data !== undefined);
      const series = snapshot?.stored.get(
        storedKey(task.accountId, task.baseId)
      );
      if (snapshot === undefined || series === undefined) {
        return null;
      }
      const now = Date.now();
      return await change.mutateAsync({
        change: completeChange(task, series, done, { now }),
        optimistic: (data) =>
          withTask(data, task.id, (item) => ({
            ...item,
            completed: done,
            completedAt: done ? now : null,
            progress: done ? "completed" : "needs-action",
          })),
      });
    },
  };
};

/** A new task list (a calendar kept as a list). */
export const useCreateTaskList = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (input: {
      readonly name: string;
      readonly color: string;
    }) => await createTaskList(input),
    onSettled: async () => {
      // The new calendar is a new list: the calendar app lists it too.
      await refreshTasks(client, { calendars: true });
    },
  });
};

/** Shows a calendar as a task list, or stops (its tasks stay). */
export const useRememberTaskList = () => {
  const client = useQueryClient();
  return (key: string, on: boolean): void => {
    rememberTaskList(key, on);
    void refreshTasks(client);
  };
};

/** "Remove from Tasks": the calendar stays, it stops being a list. */
export const useRemoveTaskList = () => {
  const client = useQueryClient();
  return (key: string): void => {
    removeTaskList(key);
    void refreshTasks(client);
  };
};

/** Live updates of tasks (JMAP push) while mounted. */
export const useTasksPush = (): void => {
  const queryClient = useQueryClient();
  useEffect(() => {
    const controller = new AbortController();
    const refresher = pushRefresher(queryClient);
    // Push is a nicety: without it the views refresh when they are opened.
    watchTasks(refresher.onChange, controller.signal).catch(() => null);
    return () => {
      controller.abort();
      refresher.cancel();
    };
  }, [queryClient]);
};
