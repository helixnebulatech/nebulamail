// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where tasks are stored, behind one interface.
// Today there is one adapter, `calendar-task`: tasks are JSCalendar
// `Task` objects inside calendars (`CalendarEvent/*`), as Stalwart and the
// bridge (VTODO over CalDAV) keep them. A `jmap-tasks` adapter (`Task/*`,
// `TaskList/*`, JMAP_TASKS_CAPABILITY) can implement the same interface
// when a server offers it. JMAP only here and in api.ts.
//
// The connection and the calendars come in as dependencies, so the live
// test (stalwart.live.test.ts) runs this same code against Stalwart.

import type {
  AccountId,
  CalendarEventId,
  JmapClient,
} from "@nebulamail/jmap-client";
import {
  CalendarId,
  isTask,
  TASK_OCCURRENCE_PROPERTIES,
  TASK_PROPERTIES,
} from "@nebulamail/jmap-client";

import type { CalendarSummary } from "../calendar/model";
import { expandWindows, maxExpandDays, nextPosition } from "../calendar/paging";
import { dateInZone } from "../calendar/time";
import type { TaskChange } from "./editing";
import type { ListHint } from "./lists";
import { noHint, taskListsOf } from "./lists";
import type { RawTask, Task, TaskList } from "./model";
import { toTask } from "./model";
import { UPCOMING_DAYS, openCounts } from "./views";

/** Everything the Tasks views need, read at once. */
export interface TaskSnapshot {
  readonly tasks: readonly Task[];
  readonly lists: readonly TaskList[];
  /** Stored tasks by `storedKey` (to build patches and undo). */
  readonly stored: ReadonlyMap<string, RawTask>;
  /** The keys the user saved as lists (synced preferences). */
  readonly saved: readonly string[];
  /** More tasks than MAX_TASKS: some are left out. */
  readonly truncated: boolean;
  /**
   * `CalendarEvent` state of each account read, as reading began (RFC 8620
   * §5.1 `state`): a push with the same state needs no other read (§7.1).
   */
  readonly states: ReadonlyMap<string, string>;
}

export interface LoadOptions {
  /** The user's zone. */
  readonly display: string;
  readonly now: number;
  /**
   * Settles once the saved lists are worth reading (the first prefs-sync);
   * the tasks are read meanwhile and it is awaited only before
   * `savedLists`. Never rejects.
   */
  readonly listsReady?: Promise<unknown>;
}

export interface TaskAdapter {
  readonly kind: "calendar-task" | "jmap-tasks";
  readonly load: (options: LoadOptions) => Promise<TaskSnapshot>;
  /** Runs a change; returns the ids of what it created, by creation key. */
  readonly apply: (
    change: TaskChange
  ) => Promise<Readonly<Record<string, CalendarEventId>>>;
}

/** A task the server refused to save (its SetError). */
export class TaskSaveError extends Error {
  readonly type: string;
  readonly properties: readonly string[];
  constructor(
    type: string,
    description: string | null,
    properties: readonly string[] = []
  ) {
    super(description ?? type);
    this.name = "TaskSaveError";
    this.type = type;
    this.properties = properties;
  }
}

/** Key of a stored task across accounts (ids repeat between accounts). */
export const storedKey = (accountId: string, id: string): string =>
  `${accountId}/${id}`;

/** At most this many tasks are read (like contacts' 10 000 cards). */
export const MAX_TASKS = 5000;

/**
 * Overdue occurrences of repeating tasks are looked for this far back:
 * Stalwart expands at most P52W1D per query, and the window also covers
 * the next UPCOMING_DAYS.
 */
export const PAST_DAYS = 300;

export interface CalendarTaskDeps {
  readonly client: () => Promise<JmapClient>;
  /** Calendars of every account (calendar/api.ts `calendarsOf`). */
  readonly calendars: (client: JmapClient) => Promise<CalendarSummary[]>;
  /** Keys of calendars the user made task lists. */
  readonly savedLists: () => readonly string[];
  /** Keys of calendars the user took out of Tasks (their tasks don't show). */
  readonly removedLists?: () => readonly string[];
  /** What the server says about a calendar holding tasks. */
  readonly hint?: (client: JmapClient) => ListHint;
}

const chunks = <T>(items: readonly T[], size: number): T[][] => {
  const out: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    out.push(items.slice(index, index + size));
  }
  return out;
};

const isRecurring = (task: RawTask): boolean =>
  task.recurrenceRule !== null && task.recurrenceRule !== undefined;

/**
 * Ids of every task of an account. There is no filter by `@type`
 * (Stalwart: "unsupportedFilter"), so the objects are listed with only
 * `@type`, page by page.
 */
const taskIds = async (
  client: JmapClient,
  accountId: AccountId,
  limit: number
): Promise<{ ids: CalendarEventId[]; state: string }> => {
  const page = client.session.core.maxObjectsInGet;
  const ids: CalendarEventId[] = [];
  // The state before the first page: what came later may be missing.
  let state: string | null = null;
  let position: number | null = 0;
  while (position !== null) {
    const from: number = position;
    // Each page starts where the server's last one ended: one at a time.
    // oxlint-disable-next-line no-await-in-loop
    const [query, objects] = await client.request((b) => {
      const q = b.call("CalendarEvent/query", {
        accountId,
        calculateTotal: true,
        limit: page,
        position: from,
      });
      const g = b.call("CalendarEvent/get", {
        "#ids": q.ref("/ids"),
        accountId,
        properties: ["@type"],
      });
      return [q, g] as const;
    });
    state ??= objects.state;
    for (const item of objects.list) {
      if (isTask(item)) {
        ids.push(item.id);
      }
    }
    if (ids.length >= limit) {
      return { ids: ids.slice(0, limit), state };
    }
    // By the server's own page: it may cap `limit` (RFC 8620 §5.5).
    position = nextPosition(query, page);
  }
  return { ids, state: state ?? "" };
};

/**
 * Full objects of these ids. Every `maxObjectsInGet` batch is one call of
 * a single request (RFC 8620 §5.1 limits the ids per `/get`, not the calls);
 * the client splits it at `maxCallsInRequest` (RFC 8620 §3.4) by itself.
 */
const getTasks = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly CalendarEventId[],
  display: string,
  properties:
    | typeof TASK_OCCURRENCE_PROPERTIES
    | typeof TASK_PROPERTIES = TASK_PROPERTIES
): Promise<RawTask[]> => {
  if (ids.length === 0) {
    return [];
  }
  const results = await client.request((b) =>
    chunks(ids, client.session.core.maxObjectsInGet).map((batch) =>
      b.call("CalendarEvent/get", {
        accountId,
        ids: batch,
        properties: [...properties],
        timeZone: display,
      })
    )
  );
  return results.flatMap((result) => result.list);
};

/**
 * The windows repeating tasks are expanded in (PAST_DAYS back, upcoming
 * ahead), none longer than the server expands (`maxExpandedQueryDuration`,
 * draft-ietf-jmap-calendars §5.11; else `expandDurationTooLarge`).
 */
const windowsOf = (
  options: LoadOptions,
  maxDays: number | null
): { readonly after: string; readonly before: string }[] => {
  const today = dateInZone(options.now, options.display);
  return expandWindows(
    today.subtract({ days: PAST_DAYS }),
    today.add({ days: UPCOMING_DAYS + 2 }),
    maxDays
  ).map(([after, before]) => ({
    after: `${after.toString()}T00:00:00`,
    before: `${before.toString()}T00:00:00`,
  }));
};

/** Base ids of every occurrence in one window, page by page. */
const expandedIds = async (
  client: JmapClient,
  accountId: AccountId,
  calendarId: string,
  window: { readonly after: string; readonly before: string },
  options: LoadOptions
): Promise<{ readonly id: CalendarEventId; readonly base: string }[]> => {
  const page = client.session.core.maxObjectsInGet;
  const found: { readonly id: CalendarEventId; readonly base: string }[] = [];
  let position: number | null = 0;
  while (position !== null) {
    const from: number = position;
    // Each page starts where the server's last one ended (RFC 8620 §5.5:
    // the server may cap `limit` even when none is asked).
    // oxlint-disable-next-line no-await-in-loop
    const [query, bases] = await client.request((b) => {
      const q = b.call("CalendarEvent/query", {
        accountId,
        expandRecurrences: true,
        filter: { ...window, inCalendar: CalendarId.parse(calendarId) },
        limit: page,
        position: from,
        timeZone: options.display,
      });
      const get = b.call("CalendarEvent/get", {
        "#ids": q.ref("/ids"),
        accountId,
        properties: ["baseEventId"],
      });
      return [q, get] as const;
    });
    for (const item of bases.list) {
      found.push({ base: item.baseEventId ?? item.id, id: item.id });
    }
    position = nextPosition(query, page);
  }
  return found;
};

/** Ids of the occurrences (in the window) of the repeating tasks of one calendar. */
const occurrenceIdsOf = async (
  client: JmapClient,
  accountId: AccountId,
  calendarId: string,
  series: ReadonlySet<string>,
  options: LoadOptions
): Promise<CalendarEventId[]> => {
  // The same calendar has events too: read the base first, then only ours.
  const windows = windowsOf(options, maxExpandDays(client, accountId));
  const lists = await Promise.all(
    windows.map(
      async (window) =>
        await expandedIds(client, accountId, calendarId, window, options)
    )
  );
  // An occurrence in two windows (it spans their edge) once.
  return [
    ...new Set(
      lists
        .flat()
        .filter((item) => series.has(item.base))
        .map((item) => item.id)
    ),
  ];
};

/**
 * Occurrences of the repeating tasks: every calendar is asked at once
 * (the client keeps to `maxConcurrentRequests`), then their full objects
 * are read in one request, in the calendars' order.
 */
const occurrencesOf = async (
  client: JmapClient,
  accountId: AccountId,
  byCalendar: ReadonlyMap<string, ReadonlySet<string>>,
  options: LoadOptions
): Promise<RawTask[]> => {
  const perCalendar = await Promise.all(
    [...byCalendar].map(
      async ([calendarId, series]) =>
        await occurrenceIdsOf(client, accountId, calendarId, series, options)
    )
  );
  return await getTasks(
    client,
    accountId,
    perCalendar.flat(),
    options.display,
    TASK_OCCURRENCE_PROPERTIES
  );
};

const firstCalendarId = (task: RawTask): string | null =>
  Object.entries(task.calendarIds ?? {}).find(([, on]) => on)?.[0] ?? null;

/** Repeating tasks' ids by the calendar that holds them. */
export const seriesByCalendar = (
  raw: readonly RawTask[]
): Map<string, Set<string>> => {
  const byCalendar = new Map<string, Set<string>>();
  for (const task of raw) {
    if (!isRecurring(task)) {
      continue;
    }
    const calendarId = firstCalendarId(task);
    if (calendarId === null) {
      continue;
    }
    const ids = byCalendar.get(calendarId);
    if (ids === undefined) {
      byCalendar.set(calendarId, new Set([task.id]));
    } else {
      ids.add(task.id);
    }
  }
  return byCalendar;
};

const loadAccount = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly CalendarEventId[],
  options: LoadOptions
): Promise<{ raw: RawTask[]; tasks: Task[] }> => {
  const raw = await getTasks(client, accountId, ids, options.display);
  const context = { accountId, display: options.display };
  const occurrences = await occurrencesOf(
    client,
    accountId,
    seriesByCalendar(raw),
    options
  );
  const byId = new Map(raw.map((task) => [task.id, task]));
  const tasks: Task[] = [];
  const expanded = new Set<string>();
  for (const occurrence of occurrences) {
    const base = byId.get(occurrence.baseEventId ?? occurrence.id);
    const task = toTask(occurrence, base, context);
    if (task !== null && base !== undefined) {
      expanded.add(base.id);
      tasks.push(task);
    }
  }
  for (const task of raw) {
    // A series without occurrences in the window (ended, or far ahead)
    // still shows, as itself.
    if (isRecurring(task) && expanded.has(task.id)) {
      continue;
    }
    const mapped = toTask(task, undefined, context);
    if (mapped !== null) {
      tasks.push(mapped);
    }
  }
  return { raw, tasks };
};

interface AccountShare {
  readonly accountId: AccountId;
  readonly ids: readonly CalendarEventId[];
  readonly state: string;
}

/**
 * Shares MAX_TASKS between accounts in their order, as if they were read
 * one after another: each gets what the ones before it left. An account
 * whose ids couldn't be listed (null) gets nothing and takes nothing.
 */
export const shareBudget = (
  found: readonly (AccountShare | null)[],
  max: number = MAX_TASKS
): { shares: AccountShare[]; truncated: boolean } => {
  const shares: AccountShare[] = [];
  let truncated = false;
  let budget = max;
  for (const account of found) {
    if (budget <= 0) {
      truncated = true;
      break;
    }
    if (account === null) {
      continue;
    }
    // Reaching the budget counts as truncated (taskIds did the same).
    truncated ||= account.ids.length >= budget;
    const ids = account.ids.slice(0, budget);
    budget -= ids.length;
    shares.push({ ...account, ids });
  }
  return { shares, truncated };
};

/** Tasks inside calendars (`CalendarEvent/*` with `@type: "Task"`). */
export const calendarTaskAdapter = (deps: CalendarTaskDeps): TaskAdapter => ({
  apply: async (change) => {
    const client = await deps.client();
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId: change.accountId,
        ...(change.create === undefined ||
        Object.keys(change.create).length === 0
          ? {}
          : {
              // The adapter writes tasks; the type wants an event's shape.
              // oxlint-disable-next-line typescript/no-unsafe-type-assertion
              create: change.create as never,
            }),
        ...(change.update === undefined ||
        Object.keys(change.update).length === 0
          ? {}
          : { update: change.update }),
        ...(change.destroy === undefined || change.destroy.length === 0
          ? {}
          : { destroy: change.destroy }),
        ...(change.notify === true ? { sendSchedulingMessages: true } : {}),
      }),
    ]);
    const failure =
      Object.values(result.notCreated)[0] ??
      Object.values(result.notUpdated)[0] ??
      Object.values(result.notDestroyed)[0];
    if (failure !== undefined) {
      throw new TaskSaveError(
        failure.type,
        failure.description ?? null,
        failure.properties ?? []
      );
    }
    return Object.fromEntries(
      Object.entries(result.created).flatMap(([key, created]) =>
        created === undefined ? [] : [[key, created.id]]
      )
    );
  },
  kind: "calendar-task",
  load: async (options) => {
    const client = await deps.client();
    const calendars = await deps.calendars(client);
    const accounts = [...new Set(calendars.map((item) => item.accountId))];
    const primary = calendars[0]?.accountId;
    // Shared accounts may fail alone; the user's own account may not.
    const settle = <T>(
      results: readonly PromiseSettledResult<T>[]
    ): (T | null)[] =>
      results.map((result, index) => {
        if (result.status === "fulfilled") {
          return result.value;
        }
        if (accounts[index] === primary) {
          throw result.reason;
        }
        return null;
      });
    // Every account at once; MAX_TASKS is then shared in their order.
    const found = settle(
      await Promise.allSettled(
        accounts.map(async (accountId) => ({
          accountId,
          ...(await taskIds(client, accountId, MAX_TASKS)),
        }))
      )
    );
    const { shares, truncated } = shareBudget(found);
    const loaded = await Promise.allSettled(
      shares.map(
        async (share) =>
          await loadAccount(client, share.accountId, share.ids, options)
      )
    );
    const stored = new Map<string, RawTask>();
    const tasks: Task[] = [];
    const withTasks = new Set<string>();
    const states = new Map<string, string>();
    for (const [index, result] of loaded.entries()) {
      const share = shares[index];
      if (share === undefined) {
        continue;
      }
      if (result.status === "rejected") {
        if (share.accountId === primary) {
          throw result.reason;
        }
        continue;
      }
      const { accountId } = share;
      states.set(accountId, share.state);
      for (const raw of result.value.raw) {
        stored.set(storedKey(accountId, raw.id), raw);
        const calendarId = firstCalendarId(raw);
        if (calendarId !== null) {
          withTasks.add(`${accountId}/${calendarId}`);
        }
      }
      tasks.push(...result.value.tasks);
    }
    await options.listsReady;
    const saved = deps.savedLists();
    const removed = new Set(deps.removedLists?.());
    // Tasks of a removed list show nowhere (views, counts, the calendar).
    const shown = tasks.filter(
      (task) => task.listKey === null || !removed.has(task.listKey)
    );
    const lists = taskListsOf({
      calendars,
      hint: deps.hint?.(client) ?? noHint,
      open: openCounts(shown),
      removed: [...removed],
      saved,
      withTasks,
    });
    return {
      lists,
      saved,
      states,
      stored,
      tasks: shown,
      truncated,
    };
  },
});
