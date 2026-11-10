// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the Tasks UI does to tasks: every change is one
// TaskChange (tasks/editing.ts) run by `useTaskChange`, shown at once where
// it can be; completing and deleting offer "Undo" like the mail app. Only
// what the list's `myRights` allow is done (`canEdit`).

import type { CalendarId, RecurrenceRule } from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { useToast } from "@nebulamail/ui";
import { useQueryClient } from "@tanstack/react-query";

import {
  useMe,
  useOwnCalendarAddresses,
  useTimeZone,
} from "../../calendar/hooks";
import { formatDate, parseLocalDate } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import type { TaskSnapshot } from "../../tasks/adapter";
import { TaskSaveError, storedKey } from "../../tasks/adapter";
import { createTask, rememberTaskList, savedTaskLists } from "../../tasks/api";
import type {
  AssigneeInput,
  Delegation,
  TaskAnswer,
  TaskAssignee,
} from "../../tasks/assignment";
import {
  answerPatch,
  assigneesPatch,
  assignmentOf,
  delegationOf,
} from "../../tasks/assignment";
import {
  itipDue,
  sendTaskCounter,
  sendTaskDelegation,
  sendTaskReply,
} from "../../tasks/assignment-api";
import type { AlertInput, NewTask, TaskChange } from "../../tasks/editing";
import {
  alertsPatch,
  duePatch,
  estimatePatch,
  listPatch,
  notesPatch,
  parentPatch,
  priorityPatch,
  recurrencePatch,
  sortOrderPatch,
  titlePatch,
} from "../../tasks/editing";
import {
  refreshTasks,
  useCompleteTask,
  useTaskChange,
  useTasks,
  withTask,
} from "../../tasks/hooks";
import type {
  RawTask,
  Task,
  TaskDue,
  TaskList,
  TaskPriority,
} from "../../tasks/model";
import { dueInstant, dueOf, estimatedMinutesOf } from "../../tasks/model";
import { shiftedSeriesDue } from "../../tasks/moving";
import { reorder } from "../../tasks/ordering";
import { withSubtasks } from "../../tasks/outline";
import { titleText } from "../calendar/event-text";
import { useDateLocale } from "../calendar/use-calendar-prefs";
import { sendErrorText } from "../calendar/use-event-actions";
import { defaultReminderMinutes } from "./day-reminders-field";
import {
  fullDueText,
  taskCounterTexts,
  taskDelegationTexts,
  taskReplyTexts,
} from "./task-format";

/** Date and time formatters by locale and zone (made once each). */
const dueTimeFormats = new Map<string, Intl.DateTimeFormat>();

const dueTimeFormat = (locale: string, zone: string): Intl.DateTimeFormat => {
  const key = `${locale}|${zone}`;
  let format = dueTimeFormats.get(key);
  if (format === undefined) {
    format = new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: zone,
    });
    dueTimeFormats.set(key, format);
  }
  return format;
};

/** Writable lists of a snapshot by key (once per snapshot). */
const writableCache = new WeakMap<readonly TaskList[], ReadonlySet<string>>();

const writableLists = (lists: readonly TaskList[]): ReadonlySet<string> => {
  let keys = writableCache.get(lists);
  if (keys === undefined) {
    keys = new Set(
      lists.filter((list) => list.mayWrite).map((list) => list.key)
    );
    writableCache.set(lists, keys);
  }
  return keys;
};

const NO_ADDRESSES: readonly string[] = [];

/**
 * The assignees the user organizes, per stored task and own addresses:
 * the same array while neither changes (rows stay memoized).
 */
const assigneesCache = new WeakMap<
  readonly string[],
  WeakMap<RawTask, readonly TaskAssignee[] | null>
>();

const organizedAssignees = (
  series: RawTask,
  own: readonly string[]
): readonly TaskAssignee[] | null => {
  let bySeries = assigneesCache.get(own);
  if (bySeries === undefined) {
    bySeries = new WeakMap();
    assigneesCache.set(own, bySeries);
  }
  const cached = bySeries.get(series);
  if (cached !== undefined) {
    return cached;
  }
  const assignment = assignmentOf(series, own);
  const assignees =
    assignment?.iAmOrganizer === true ? assignment.assignees : null;
  bySeries.set(series, assignees);
  return assignees;
};

/** "6 Nov 2026" or "6 Nov 2026, 17:30" for a toast. */
const dueWords = (due: TaskDue, zone: string, locale: string): string => {
  if (due.kind === "date") {
    const date = parseLocalDate(due.date);
    return date === null
      ? due.date
      : formatDate(date, locale, { dateStyle: "medium" });
  }
  const at = dueInstant(due, zone);
  return at === null ? due.local : dueTimeFormat(locale, zone).format(at);
};

const errorText = (error: unknown): string =>
  error instanceof TaskSaveError && error.type === "forbidden"
    ? m.tasks_error_forbidden()
    : sendErrorText(error, m.tasks_error_save());

export interface TaskActions {
  readonly snapshot: TaskSnapshot | undefined;
  /** The list allows changing the task (`myRights`). */
  readonly canEdit: (task: Task) => boolean;
  /** The stored task (the series of an occurrence). */
  readonly seriesOf: (task: Task) => RawTask | undefined;
  readonly complete: (task: Task, done: boolean) => void;
  readonly remove: (task: Task) => void;
  readonly setTitle: (task: Task, title: string) => void;
  readonly setNotes: (task: Task, notes: string) => void;
  readonly setPriority: (task: Task, priority: TaskPriority) => void;
  /** The estimated time in minutes (RFC 8984 §5.2.3); null: none. */
  readonly setEstimate: (task: Task, minutes: number | null) => void;
  readonly setDue: (task: Task, due: TaskDue | null) => void;
  readonly setRecurrence: (task: Task, rule: RecurrenceRule | null) => void;
  readonly setAlerts: (task: Task, alerts: readonly AlertInput[]) => void;
  readonly moveTo: (task: Task, calendarId: CalendarId) => void;
  /**
   * Assigns the task to these people (none: to nobody); the user becomes
   * its organizer and the server or the bridge tells them by email.
   */
  readonly assign: (task: Task, people: readonly AssigneeInput[]) => void;
  /**
   * Who the task is assigned to, if the user assigned it (shown in the
   * list); null otherwise.
   */
  readonly assigneesOf: (task: Task) => readonly TaskAssignee[] | null;
  /** The user's answer to a task someone assigned them (mails a REPLY). */
  readonly answer: (task: Task, answer: TaskAnswer) => void;
  /**
   * Proposes another due to whoever assigned the task (mails a COUNTER;
   * the task doesn't change). Resolves to whether it went out.
   */
  readonly propose: (
    task: Task,
    due: TaskDue,
    comment: string | undefined
  ) => Promise<boolean>;
  /**
   * Passes a task assigned to the user on to someone else: their copy says
   * delegated, and the organizer and the delegate are mailed.
   */
  readonly delegate: (
    task: Task,
    person: AssigneeInput,
    comment: string | undefined
  ) => Promise<boolean>;
  /** Manual order: `task` (at `from` of `siblings`, on screen) goes to `to`. */
  readonly reorder: (
    siblings: readonly Task[],
    from: number,
    to: number
  ) => void;
  /**
   * A new due from the calendar (dragging or Alt + arrows), with "Undo". A
   * repeating task moves as a series, as much as this occurrence moved.
   */
  readonly moveDue: (task: Task, due: TaskDue) => void;
  readonly setParent: (task: Task, parentUid: string | null) => void;
  /**
   * Dragging in the list: `task` goes under `parentUid` (null: the top)
   * and, with `order`, to index `to` of its new `siblings` (without it).
   * "Undo" in the notification; `parent` names the new parent in it.
   */
  readonly place: (
    task: Task,
    parentUid: string | null,
    order: { readonly siblings: readonly Task[]; readonly to: number } | null,
    parent: Task | null
  ) => void;
  readonly create: (
    input: Omit<NewTask, "calendarId"> & { readonly listKey: string }
  ) => Promise<{ readonly id: string; readonly uid: string } | null>;
  /** A change is being saved. */
  readonly saving: boolean;
}

export const useTaskActions = ({
  enabled = true,
}: { readonly enabled?: boolean } = {}): TaskActions => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const toast = useToast();
  const client = useQueryClient();
  const query = useTasks({ enabled });
  const change = useTaskChange(taskReplyTexts);
  const completer = useCompleteTask(taskReplyTexts);
  const me = useMe();
  const own = useOwnCalendarAddresses();
  const snapshot = query.data;

  const seriesOf = (task: Task): RawTask | undefined =>
    snapshot?.stored.get(storedKey(task.accountId, task.baseId));

  const canEdit = (task: Task): boolean =>
    snapshot !== undefined &&
    task.listKey !== null &&
    writableLists(snapshot.lists).has(task.listKey);

  const failed = (error: unknown): void => {
    toast.show({ priority: "high", title: errorText(error), tone: "danger" });
  };

  /** Saved, but the REPLY to whoever assigned the task didn't go out. */
  const checkReply = <T extends { readonly replyFailed: boolean } | null>(
    result: T
  ): T => {
    if (result?.replyFailed === true) {
      toast.show({ title: m.tasks_reply_failed(), tone: "danger" });
    }
    return result;
  };

  /** The REPLY to the organizer and the REQUEST to the delegate. */
  const mailDelegation = async (
    series: RawTask,
    delegation: Delegation,
    input: {
      readonly person: AssigneeInput;
      readonly me: string;
      readonly organizer: string;
      readonly comment: string | undefined;
    }
  ): Promise<void> => {
    const name = me.data?.name ?? null;
    const due = dueOf(series);
    await sendTaskDelegation(series, delegation, {
      due: due === null ? null : itipDue(due, zone),
      name,
      own: own.data ?? [],
      texts: taskDelegationTexts({
        delegate: input.person.name ?? input.person.email,
        name: name ?? input.me,
        organizer: input.organizer,
        title: series.title ?? "",
      }),
      ...(input.comment === undefined ? {} : { comment: input.comment }),
    });
  };

  const run = (
    input: TaskChange,
    optimistic?: (data: TaskSnapshot) => TaskSnapshot
  ): void => {
    // The promise (not mutate's callbacks): those only run for the
    // latest call, and changes overlap.
    change
      .mutateAsync({
        change: input,
        ...(optimistic === undefined ? {} : { optimistic }),
      })
      .then(checkReply)
      .catch(failed);
  };

  const undoToast = (title: string, undo: TaskChange): void => {
    toast.show({
      action: {
        label: m.toast_undo(),
        onClick: () => {
          run(undo);
        },
      },
      title,
    });
  };

  /** Runs a change and offers "Undo" in a notification titled `title`. */
  const runWithUndo = (
    input: TaskChange,
    title: string,
    optimistic?: (data: TaskSnapshot) => TaskSnapshot
  ): void => {
    void (async () => {
      try {
        const result = checkReply(
          await change.mutateAsync({
            change: input,
            ...(optimistic === undefined ? {} : { optimistic }),
          })
        );
        undoToast(title, result.undo);
      } catch (error) {
        failed(error);
      }
    })();
  };

  const patch = (
    task: Task,
    body: Readonly<Record<string, unknown>> | null,
    optimistic?: (task: Task) => Task
  ): void => {
    if (body === null || Object.keys(body).length === 0 || !canEdit(task)) {
      return;
    }
    run(
      { accountId: task.accountId, update: { [task.baseId]: body } },
      optimistic === undefined
        ? undefined
        : (data) => ({
            ...data,
            tasks: data.tasks.map((item) =>
              item.baseId === task.baseId && item.accountId === task.accountId
                ? optimistic(item)
                : item
            ),
          })
    );
  };

  /** The stored task, with the parent shown (it may be newer: just set). */
  const withShownParent = (task: Task): RawTask | undefined => {
    const stored = seriesOf(task);
    return stored === undefined ||
      task.parentUid === null ||
      stored.relatedTo?.[task.parentUid] !== undefined
      ? stored
      : {
          ...stored,
          relatedTo: {
            ...stored.relatedTo,
            [task.parentUid]: {
              "@type": "Relation",
              relation: { parent: true },
            },
          },
        };
  };

  return {
    answer: (task, answer) => {
      const series = seriesOf(task);
      const mine =
        series === undefined
          ? null
          : (assignmentOf(series, own.data ?? [])?.me ?? null);
      if (series === undefined || mine === null || !canEdit(task)) {
        return;
      }
      void (async () => {
        try {
          await change.mutateAsync({
            change: {
              accountId: task.accountId,
              update: { [task.baseId]: answerPatch(series, mine.key, answer) },
            },
          });
          await sendTaskReply(series, answer, {
            name: me.data?.name ?? null,
            own: own.data ?? [],
            texts: taskReplyTexts,
          });
          const organizer = assignmentOf(series, own.data ?? [])?.organizer;
          if (organizer !== null && organizer !== undefined) {
            toast.show({
              title: m.tasks_answer_sent({
                name: organizer.name ?? organizer.email,
              }),
            });
          }
        } catch (error) {
          failed(error);
        }
      })();
    },
    propose: async (task, due, comment) => {
      const series = seriesOf(task);
      const organizer =
        series === undefined
          ? null
          : (assignmentOf(series, own.data ?? [])?.organizer ?? null);
      if (series === undefined || organizer === null) {
        return false;
      }
      const name = me.data?.name ?? null;
      try {
        await sendTaskCounter(series, due, {
          display: zone,
          name,
          own: own.data ?? [],
          texts: taskCounterTexts(
            series.title ?? "",
            name ?? me.data?.email ?? "",
            fullDueText(due, zone, locale)
          ),
          ...(comment === undefined ? {} : { comment }),
        });
        toast.show({
          title: m.tasks_proposal_sent({
            name: cleanDisplayText(organizer.name ?? organizer.email),
          }),
        });
        return true;
      } catch (error) {
        failed(error);
        return false;
      }
    },
    delegate: async (task, person, comment) => {
      const series = seriesOf(task);
      const assignment =
        series === undefined ? null : assignmentOf(series, own.data ?? []);
      const mine = assignment?.me ?? null;
      const organizer = assignment?.organizer ?? null;
      if (
        series === undefined ||
        mine === null ||
        organizer === null ||
        !canEdit(task)
      ) {
        return false;
      }
      const delegation = delegationOf(series, mine.key, person, own.data ?? []);
      if (delegation === null) {
        toast.show({ title: m.tasks_delegate_taken(), tone: "danger" });
        return false;
      }
      try {
        await change.mutateAsync({
          change: {
            accountId: task.accountId,
            update: { [task.baseId]: delegation.patch },
          },
        });
      } catch (error) {
        failed(error);
        return false;
      }
      try {
        await mailDelegation(series, delegation, {
          comment,
          me: mine.email,
          organizer: organizer.name ?? organizer.email,
          person,
        });
        toast.show({
          title: m.tasks_delegated_toast({
            name: cleanDisplayText(person.name ?? person.email),
          }),
        });
      } catch {
        toast.show({ title: m.tasks_reply_failed(), tone: "danger" });
      }
      return true;
    },
    assigneesOf: (task) => {
      const series = seriesOf(task);
      return series === undefined
        ? null
        : organizedAssignees(series, own.data ?? NO_ADDRESSES);
    },
    assign: (task, people) => {
      const series = seriesOf(task);
      if (series === undefined || me.data === undefined) {
        return;
      }
      patch(task, assigneesPatch(series, people, me.data));
    },
    canEdit,
    complete: (task, done) => {
      if (!canEdit(task)) {
        return;
      }
      void (async () => {
        try {
          const result = checkReply(await completer.complete(task, done));
          if (result !== null && done) {
            undoToast(
              m.tasks_completed_toast({ title: titleText(task.title) }),
              result.undo
            );
          }
        } catch (error) {
          failed(error);
        }
      })();
    },
    create: async (input) => {
      const list = snapshot?.lists.find((item) => item.key === input.listKey);
      const [accountId, calendarId] = input.listKey.split("/");
      if (accountId === undefined || calendarId === undefined) {
        return null;
      }
      try {
        const created = await createTask(
          {
            ...input,
            // The key's parts come from the server's own ids.
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            accountId: (list?.accountId ?? accountId) as Task["accountId"],
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            calendarId: (list?.id ?? calendarId) as CalendarId,
          },
          zone
        );
        if (!savedTaskLists().includes(input.listKey)) {
          // A calendar without tasks only stays a list if it's saved.
          rememberTaskList(input.listKey, true);
        }
        await refreshTasks(client);
        return created;
      } catch (error) {
        failed(error);
        return null;
      }
    },
    moveDue: (task, due) => {
      const series = seriesOf(task);
      if (series === undefined || !canEdit(task)) {
        return;
      }
      // A repeating task: the series moves as much as this occurrence.
      const seriesDue = dueOf(series);
      const target =
        task.recurring && task.due !== null && seriesDue !== null
          ? shiftedSeriesDue(seriesDue, task.due, due)
          : due;
      if (target === null) {
        return;
      }
      const body = duePatch(
        task,
        series,
        target,
        zone,
        defaultReminderMinutes()
      );
      const title = titleText(task.title);
      runWithUndo(
        { accountId: task.accountId, update: { [task.baseId]: body } },
        task.recurring
          ? m.tasks_series_moved_toast({ title })
          : m.tasks_due_moved_toast({
              title,
              when: dueWords(due, zone, locale),
            }),
        task.recurring
          ? undefined
          : (data) =>
              withTask(data, task.id, (item) => ({
                ...item,
                due,
                dueAt: dueInstant(due, zone),
              }))
      );
    },
    moveTo: (task, calendarId) => {
      const list = snapshot?.lists.find(
        (item) => item.accountId === task.accountId && item.id === calendarId
      );
      if (!canEdit(task) || list === undefined || !list.mayWrite) {
        return;
      }
      runWithUndo(
        {
          accountId: task.accountId,
          update: { [task.baseId]: listPatch(calendarId) },
        },
        m.tasks_moved_to_list_toast({
          list: titleText(list.name),
          title: titleText(task.title),
        }),
        (data) => ({
          ...data,
          tasks: data.tasks.map((item) =>
            item.baseId === task.baseId && item.accountId === task.accountId
              ? { ...item, calendarId, listKey: list.key }
              : item
          ),
        })
      );
    },
    place: (task, parentUid, order, parent) => {
      const series = withShownParent(task);
      if (series === undefined || !canEdit(task)) {
        return;
      }
      const orders: ReadonlyMap<string, number> =
        order === null
          ? new Map()
          : reorder([...order.siblings, task], order.siblings.length, order.to);
      const update: Record<string, Readonly<Record<string, unknown>>> = {};
      for (const [id, value] of orders) {
        const stored = snapshot?.stored.get(storedKey(task.accountId, id));
        if (stored !== undefined) {
          update[id] = sortOrderPatch(stored, value);
        }
      }
      if (parentUid !== task.parentUid) {
        update[task.baseId] = {
          ...update[task.baseId],
          ...parentPatch(series, parentUid),
        };
      }
      if (Object.keys(update).length === 0) {
        return;
      }
      const title = titleText(task.title);
      runWithUndo(
        { accountId: task.accountId, update },
        parent === null
          ? m.tasks_reordered_toast({ title })
          : m.tasks_nested_toast({ parent: titleText(parent.title), title }),
        (data) => ({
          ...data,
          tasks: data.tasks.map((item) => {
            if (item.accountId !== task.accountId) {
              return item;
            }
            const value = orders.get(item.baseId);
            const ordered =
              value === undefined ? item : { ...item, sortOrder: value };
            return item.baseId === task.baseId
              ? { ...ordered, parentUid }
              : ordered;
          }),
        })
      );
    },
    reorder: (siblings, from, to) => {
      const task = siblings[from];
      if (task === undefined || !canEdit(task)) {
        return;
      }
      const orders = reorder(siblings, from, to);
      const update: Record<string, Readonly<Record<string, unknown>>> = {};
      for (const [id, order] of orders) {
        const series = snapshot?.stored.get(storedKey(task.accountId, id));
        if (series !== undefined) {
          update[id] = sortOrderPatch(series, order);
        }
      }
      if (Object.keys(update).length === 0) {
        return;
      }
      runWithUndo(
        { accountId: task.accountId, update },
        m.tasks_reordered_toast({ title: titleText(task.title) }),
        (data) => ({
          ...data,
          tasks: data.tasks.map((item) => {
            const order =
              item.accountId === task.accountId
                ? orders.get(item.baseId)
                : undefined;
            return order === undefined ? item : { ...item, sortOrder: order };
          }),
        })
      );
    },
    remove: (task) => {
      if (!canEdit(task) || snapshot === undefined) {
        return;
      }
      // A task goes with its subtasks (their parent would be gone).
      const doomed = withSubtasks(snapshot.tasks, task);
      const ids = [...new Set(doomed.map((item) => item.baseId))];
      const gone = new Set(ids);
      void (async () => {
        try {
          const result = await change.mutateAsync({
            change: { accountId: task.accountId, destroy: ids },
            optimistic: (data) => ({
              ...data,
              tasks: data.tasks.filter(
                (item) =>
                  item.accountId !== task.accountId || !gone.has(item.baseId)
              ),
            }),
          });
          undoToast(
            doomed.length > 1
              ? m.tasks_deleted_with_subtasks({
                  count: doomed.length - 1,
                  title: titleText(task.title),
                })
              : m.tasks_deleted_toast({ title: titleText(task.title) }),
            result.undo
          );
        } catch (error) {
          failed(error);
        }
      })();
    },
    saving: change.isPending,
    seriesOf,
    setAlerts: (task, alerts) => {
      const series = seriesOf(task);
      if (series === undefined) {
        return;
      }
      // Keys made here, so the panel shows the change at once and the
      // next edit (a time typed digit by digit) builds on it.
      const stamp = Date.now().toString(36);
      const keyed = alerts.map((alert, index) => ({
        ...alert,
        key: alert.key ?? `a${stamp}${String(index)}`,
      }));
      patch(task, alertsPatch(series, keyed, task.due, zone), (item) => ({
        ...item,
        alerts: keyed.map((alert) =>
          "at" in alert
            ? { at: alert.at, key: alert.key, minutesBefore: null }
            : { at: null, key: alert.key, minutesBefore: alert.minutesBefore }
        ),
      }));
    },
    setDue: (task, due) => {
      const series = seriesOf(task);
      if (series !== undefined) {
        patch(
          task,
          duePatch(task, series, due, zone, defaultReminderMinutes())
        );
      }
    },
    setNotes: (task, notes) => {
      patch(task, notesPatch(notes), (item) => ({
        ...item,
        notes: notes.trim() === "" ? null : notes.trim(),
      }));
    },
    setParent: (task, parentUid) => {
      const series = withShownParent(task);
      if (series !== undefined) {
        patch(task, parentPatch(series, parentUid), (item) => ({
          ...item,
          parentUid,
        }));
      }
    },
    setPriority: (task, priority) => {
      patch(task, priorityPatch(priority), (item) => ({ ...item, priority }));
    },
    setEstimate: (task, minutes) => {
      const body = estimatePatch(minutes);
      patch(task, body, (item) => ({
        ...item,
        estimatedMinutes: estimatedMinutesOf(
          typeof body.estimatedDuration === "string"
            ? body.estimatedDuration
            : null
        ),
      }));
    },
    setRecurrence: (task, rule) => {
      const series = seriesOf(task);
      if (series !== undefined) {
        patch(task, recurrencePatch(task, series, rule, zone));
      }
    },
    setTitle: (task, title) => {
      if (title.trim() === "") {
        return;
      }
      patch(task, titlePatch(title), (item) => ({
        ...item,
        title: title.trim(),
      }));
    },
    snapshot,
  };
};
