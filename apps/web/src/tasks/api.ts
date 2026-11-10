// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks domain functions: the UI calls these, never
// JMAP. They pick the adapter (adapter.ts) for this tab's connection and
// keep the user's task lists in the synced preferences.
//
// Decided 2026-10-28 after checking Stalwart live
// tasks go straight to the account's server through its calendars;
// no second server for tasks.

import type {
  AccountId,
  CalendarEventId,
  CalendarId,
  JmapClient,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  TASK_PROPERTIES,
  hasCapability,
} from "@nebulamail/jmap-client";

import { dismissPatch } from "../calendar/alerts";
import {
  calendarsOf,
  createCalendar,
  listOwnAddresses,
  me,
} from "../calendar/api";
import { calendarKey } from "../calendar/model";
import { getClient } from "../mail/client";
import { recordChange, syncedValue } from "../prefs-sync/sync";
import type { LoadOptions, TaskAdapter, TaskSnapshot } from "./adapter";
import { calendarTaskAdapter } from "./adapter";
import { scheduleChange } from "./assignment";
import type { TaskReplyTexts } from "./assignment-api";
import { sendTaskReply } from "./assignment-api";
import type { NewTask, TaskChange } from "./editing";
import { completeChange, newTaskProperties, undoOf } from "./editing";
import type { ListHint } from "./lists";
import { bridgeHint, noHint, withSavedList } from "./lists";
import type { Task } from "./model";
import { toTask } from "./model";

/** Calendars the user made task lists, as this device last synced them. */
export const savedTaskLists = (): readonly string[] =>
  syncedValue("taskLists") ?? [];

/** Calendars the user took out of Tasks, as this device last synced them. */
export const removedTaskLists = (): readonly string[] =>
  syncedValue("removedTaskLists") ?? [];

/**
 * Keeps (or forgets) a calendar as a task list, on every device. Either
 * way it is no longer one the user took out of Tasks: adding it again
 * brings it back, and a deleted calendar needs no entry.
 */
export const rememberTaskList = (key: string, on: boolean): void => {
  recordChange("taskLists", withSavedList(savedTaskLists(), key, on));
  if (removedTaskLists().includes(key)) {
    recordChange(
      "removedTaskLists",
      withSavedList(removedTaskLists(), key, false)
    );
  }
};

/**
 * "Remove from Tasks": the calendar stops being a list, even with tasks
 * in it; the calendar and its tasks stay as they are.
 */
export const removeTaskList = (key: string): void => {
  recordChange("taskLists", withSavedList(savedTaskLists(), key, false));
  recordChange(
    "removedTaskLists",
    withSavedList(removedTaskLists(), key, true)
  );
};

/**
 * What the server says per calendar about tasks. Stalwart says nothing
 * (every calendar holds VTODO). The bridge says which of its CalDAV
 * calendars take VTODO (`BRIDGE_COMPONENTS`, read by `calendarsOf` with
 * its capability in `using`).
 */
const listHint = (client: JmapClient): ListHint =>
  hasCapability(client.session, CAPABILITIES.bridge) ? bridgeHint : noHint;

let adapter: TaskAdapter | undefined;

/**
 * The tab's task adapter. Only `calendar-task` exists: a server with
 * JMAP_TASKS_CAPABILITY would get a `jmap-tasks` adapter here.
 */
export const taskAdapter = (): TaskAdapter => {
  adapter ??= calendarTaskAdapter({
    calendars: calendarsOf,
    client: getClient,
    hint: listHint,
    removedLists: removedTaskLists,
    savedLists: savedTaskLists,
  });
  return adapter;
};

/**
 * The account whose tasks can be assigned to others
 * or null. Needs calendars (participants are
 * scheduled with `sendSchedulingMessages`) and sending mail (the
 * assignee's REPLY). Only the user's own account: assigning a task of a
 * list someone shares would make the user organize another's task.
 */
export const assignableAccount = async (): Promise<AccountId | null> => {
  const client = await getClient();
  if (
    !hasCapability(client.session, CAPABILITIES.calendars) ||
    !hasCapability(client.session, CAPABILITIES.submission)
  ) {
    return null;
  }
  return client.primaryAccountId(CAPABILITIES.calendars);
};

/** Whether the server can keep tasks (the app rail hides Tasks if not). */
export const tasksAvailable = async (): Promise<boolean> => {
  const client = await getClient();
  return hasCapability(client.session, CAPABILITIES.calendars);
};

export const loadTasks = async (options: LoadOptions): Promise<TaskSnapshot> =>
  await taskAdapter().load(options);

export const applyTaskChange = async (
  change: TaskChange
): Promise<Readonly<Record<string, CalendarEventId>>> =>
  await taskAdapter().apply(change);

/** A new UID (RFC 4122 URN, no "/" so paths to it need no escaping). */
export const newTaskUid = (): string => crypto.randomUUID();

/** Creates a task in its list; returns its id and UID. */
export const createTask = async (
  input: NewTask & { readonly accountId: TaskChange["accountId"] },
  display: string
): Promise<{ readonly id: CalendarEventId; readonly uid: string }> => {
  const uid = newTaskUid();
  const created = await applyTaskChange({
    accountId: input.accountId,
    create: { task: newTaskProperties(input, { display, uid }) },
  });
  const id = created.task;
  if (id === undefined) {
    throw new Error("The server created no task");
  }
  return { id, uid };
};

/**
 * A new task list: a calendar in the user's own account, kept as a list
 * (an empty calendar can't be told from any other on Stalwart). The
 * bridge makes it a task-only calendar (VTODO), which the calendar hides.
 */
export const createTaskList = async (input: {
  readonly name: string;
  readonly color: string;
}): Promise<{ readonly id: CalendarId; readonly key: string }> => {
  const id = await createCalendar({ ...input, components: ["VTODO"] });
  const client = await getClient();
  const key = calendarKey(client.primaryAccountId(CAPABILITIES.calendars), id);
  rememberTaskList(key, true);
  return { id, key };
};

/**
 * Where a task made outside Tasks (from an email) goes without loading
 * every task: the first saved list the user can write to, else their
 * default writable calendar (kept as a list once it has a task).
 */
export const defaultTaskListKey = async (): Promise<string | null> => {
  const client = await getClient();
  const calendars = await calendarsOf(client);
  const removed = new Set(removedTaskLists());
  const writable = calendars.filter(
    (calendar) =>
      calendar.mayWrite &&
      !removed.has(calendar.key) &&
      (listHint(client)(calendar) ?? true)
  );
  const keys = new Set(writable.map((calendar) => calendar.key));
  const own = writable.filter((calendar) => calendar.owner === null);
  return (
    savedTaskLists().find((key) => keys.has(key)) ??
    (own.find((calendar) => calendar.isDefault) ?? own[0])?.key ??
    null
  );
};

/** The ids of the user's own calendars account (for URLs). */
export const ownTasksAccount = async (): Promise<string> => {
  const client = await getClient();
  return client.primaryAccountId(CAPABILITIES.calendars);
};

/**
 * The occurrence of `series` that an alert's `recurrenceId` names (for
 * `completeChange`): its id stays the series' (only `baseId` and
 * `recurrenceId` are written), its due is that day.
 */
export const occurrenceOf = (series: Task, recurrenceId: string): Task => {
  const date = recurrenceId.slice(0, 10);
  let { due } = series;
  if (due?.kind === "date") {
    due = { date, kind: "date" };
  } else if (due?.kind === "datetime") {
    due = { ...due, local: `${date}${due.local.slice(10)}` };
  }
  return { ...series, due, recurrenceId };
};

/**
 * Marks the task of a reminder as done ("Mark done" on the reminder card)
 * and dismisses the reminder; answers the change that undoes it (null if
 * the task is gone). An assigned task goes as from the list: the assignees
 * hear if the user assigned it, the organizer gets the user's REPLY if
 * someone else did (`replyFailed`: saved, but not mailed).
 */
export const completeFromAlert = async (
  alert: {
    readonly accountId: AccountId;
    readonly eventId: CalendarEventId;
    readonly alertId: string;
    readonly recurrenceId: string | null;
    readonly stored: boolean;
  },
  display: string,
  texts: TaskReplyTexts
): Promise<{
  readonly undo: TaskChange;
  readonly replyFailed: boolean;
} | null> => {
  const client = await getClient();
  const [result] = await client.request((b) => [
    b.call("CalendarEvent/get", {
      accountId: alert.accountId,
      ids: [alert.eventId],
      properties: [...TASK_PROPERTIES],
      timeZone: display,
    }),
  ]);
  const [raw] = result.list;
  const mapped =
    raw === undefined
      ? null
      : toTask(raw, undefined, { accountId: alert.accountId, display });
  if (raw === undefined || mapped === null) {
    return null;
  }
  const task =
    alert.recurrenceId === null || !mapped.recurring
      ? mapped
      : occurrenceOf(mapped, alert.recurrenceId);
  const done = completeChange(task, raw, true, { now: Date.now() });
  const update = done.update?.[raw.id];
  const change: TaskChange = {
    ...done,
    update: {
      ...done.update,
      [raw.id]: {
        ...update,
        ...(alert.stored ? dismissPatch(alert.alertId, Date.now()) : {}),
      },
    },
  };
  const before = new Map([[raw.id, raw]]);
  if (Object.keys(raw.participants ?? {}).length === 0) {
    const created = await applyTaskChange(change);
    return { replyFailed: false, undo: undoOf(change, before, created) };
  }
  const [own, self] = await Promise.all([
    listOwnAddresses(),
    me().catch(() => null),
  ]);
  const scheduled = scheduleChange(change, before, own);
  const created = await applyTaskChange(scheduled.change);
  let replyFailed = false;
  for (const reply of scheduled.replies) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- one email after another, usually one.
      await sendTaskReply(reply.task, reply.answer, {
        name: self?.name ?? null,
        own,
        texts,
      });
    } catch {
      replyFailed = true;
    }
  }
  return { replyFailed, undo: undoOf(scheduled.change, before, created) };
};
