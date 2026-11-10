// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Assigned tasks in the mail reader: an
// iMIP email whose calendar part is a VTODO, read with
// `CalendarEvent/parse` like an invitation (calendar/invitations.ts).
// Nothing is stored until the user presses a button: Accept (adds the
// task to their default task list, or records the answer in the copy the
// server already put there) / Decline, remove a task taken back, or
// write an assignee's answer into the user's own task. Answers go to the
// organizer as a REPLY mailed by the app. An assignee may also propose
// another due (COUNTER) or pass the task on (delegate); the organizer
// accepts a proposal (the task moves, with scheduling) or keeps the due
// (DECLINECOUNTER).

import type { BlobId } from "@nebulamail/jmap-client";
import {
  AccountId as AccountIdSchema,
  CAPABILITIES,
  CalendarEventId as CalendarEventIdSchema,
  TASK_PROPERTIES,
  isTask,
} from "@nebulamail/jmap-client";

import type { ReplyTexts } from "../calendar/invitations";
import { analyzeInvitation, mainEvent, responseStamp } from "../calendar/itip";
import type { Invitation, InvitationSender } from "../calendar/itip";
import { getClient } from "../mail/client";
import { applyTaskChange, defaultTaskListKey } from "./api";
import type {
  Assignment,
  AssigneeInput,
  TaskAnswer,
  TaskAssignee,
  TaskCardAction,
} from "./assignment";
import {
  answerPatch,
  assignmentOf,
  replyDelegates,
  counterDuePatch,
  delegationOf,
  fromSender,
  replyPatch,
  senderCounterAuthor,
  taskCardAction,
  taskSenderBlocked,
  taskToImport,
} from "./assignment";
import type { TaskReplyTexts } from "./assignment-api";
import {
  itipDue,
  sendTaskCounter,
  sendTaskDeclineCounter,
  sendTaskDelegation,
  sendTaskReply,
} from "./assignment-api";
import type { RawTask, TaskDue } from "./model";
import { dueOf } from "./model";

export interface TaskAssignmentView {
  readonly invitation: Invitation;
  /** The task as the email has it (untrusted text). */
  readonly task: RawTask;
  /**
   * `task` with only the participants its sender may speak for: what a
   * REPLY or COUNTER may change (`fromSender`).
   */
  readonly reply: RawTask;
  readonly assignment: Assignment | null;
  /** The same task in the user's tasks (by UID). */
  readonly stored: RawTask | null;
  readonly action: TaskCardAction;
  /** It would offer something if the right person had sent it. */
  readonly senderBlocked: boolean;
  /** The user's tasks have a newer version (this email is an old one). */
  readonly outdated: boolean;
  /** Who proposes another due (a COUNTER for the user's task). */
  readonly counterAuthor: TaskAssignee | null;
  /** Who applying this REPLY adds to the user's task (passed on to them). */
  readonly delegates: readonly TaskAssignee[];
}

const PARSED_ID = CalendarEventIdSchema.parse("assignment");

/** Parses the calendar part; null if it isn't a task. */
export const loadTaskAssignment = async (
  blobId: BlobId,
  own: readonly string[],
  sender: InvitationSender
): Promise<TaskAssignmentView | null> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [parsed] = await client.request((b) => [
    b.call("CalendarEvent/parse", { accountId, blobIds: [blobId] }),
  ]);
  const list = parsed.parsed[blobId] ?? [];
  const main = mainEvent(list);
  if (main === null || !isTask(main)) {
    return null;
  }
  const invitation = analyzeInvitation(list, own);
  if (invitation === null) {
    return null;
  }
  const [, found] = await client.request((b) => {
    const query = b.call("CalendarEvent/query", {
      accountId,
      filter: { uid: invitation.uid },
    });
    const get = b.call("CalendarEvent/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: [...TASK_PROPERTIES, "participants"],
    });
    return [query, get] as const;
  });
  // `uid` matches words for some servers: check it exactly.
  const stored: RawTask | null =
    found.list.find((item) => item.uid === invitation.uid && isTask(item)) ??
    null;
  // The parsed object is a task without a JMAP id.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  const task = { ...main, id: PARSED_ID } as unknown as RawTask;
  const assignment = assignmentOf(task, own);
  // What a REPLY or COUNTER may change: only what its sender may say.
  const reply = fromSender(task, sender, stored);
  const card = {
    assignment,
    // A task's STATUS:CANCELLED is its `progress` in JSCalendar.
    cancelled:
      invitation.cancelled || task.progress?.toLowerCase() === "cancelled",
    method: invitation.method,
    outdated:
      stored !== null && Math.trunc(stored.sequence ?? 0) > invitation.sequence,
    own,
    sender,
    stamp: responseStamp(task),
    stored,
  };
  const action = taskCardAction(card);
  return {
    action,
    assignment,
    counterAuthor:
      stored === null || action !== "counter"
        ? null
        : senderCounterAuthor(assignment, stored, own, sender),
    delegates:
      stored === null || invitation.method !== "reply"
        ? []
        : replyDelegates(stored, reply, own),
    invitation,
    outdated: card.outdated,
    reply,
    senderBlocked: taskSenderBlocked(card),
    stored,
    task,
  };
};

const ownAccount = async () => {
  const client = await getClient();
  return client.primaryAccountId(CAPABILITIES.calendars);
};

export interface AnswerOptions {
  readonly own: readonly string[];
  readonly name: string | null;
  readonly texts: TaskReplyTexts;
}

/**
 * The user's answer to an assigned task: stored (added to the default task
 * list when accepting a task that isn't there yet) and mailed.
 */
export const answerAssignment = async (
  view: TaskAssignmentView,
  answer: TaskAnswer,
  options: AnswerOptions
): Promise<void> => {
  // Only what the card offered: a task assigned to the user.
  if (view.action !== "answer") {
    throw new Error("Not a task assigned to this user");
  }
  const me = view.assignment?.me ?? null;
  if (me === null) {
    throw new Error("The task isn't assigned to this user");
  }
  if (view.stored === null) {
    if (answer !== "declined") {
      const key = await defaultTaskListKey();
      const slash = key?.indexOf("/") ?? -1;
      if (key === null || key === undefined || slash < 0) {
        throw new Error("No task list to add the task to");
      }
      await applyTaskChange({
        accountId: AccountIdSchema.parse(key.slice(0, slash)),
        create: {
          task: {
            ...taskToImport(view.task, me.key, answer),
            calendarIds: { [key.slice(slash + 1)]: true },
          },
        },
      });
    }
  } else {
    const storedMe = assignmentOf(view.stored, options.own)?.me ?? null;
    if (storedMe !== null) {
      const accountId = await ownAccount();
      await applyTaskChange({
        accountId,
        update: {
          [view.stored.id]: answerPatch(view.stored, storedMe.key, answer),
        },
      });
    }
  }
  await sendTaskReply(view.task, answer, options);
};

/** A task taken back: removes it from the user's tasks (sends nothing). */
export const removeAssignment = async (
  view: TaskAssignmentView
): Promise<void> => {
  // Only what the card offered: a CANCEL from the stored organizer.
  if (view.action !== "remove") {
    throw new Error("Not a cancellation from the task's organizer");
  }
  if (view.stored !== null) {
    await applyTaskChange({
      accountId: await ownAccount(),
      destroy: [view.stored.id],
    });
  }
};

/** An assignee's REPLY: writes their answer into the user's task. */
export const applyAssignmentReply = async (
  view: TaskAssignmentView,
  own: readonly string[]
): Promise<void> => {
  // Only what the card offered (from the right person, not outdated).
  if (view.action !== "apply-reply" || view.stored === null) {
    throw new Error("Not an answer to the user's task");
  }
  const patch = replyPatch(view.stored, view.reply, own);
  if (Object.keys(patch).length > 0) {
    await applyTaskChange({
      accountId: await ownAccount(),
      update: { [view.stored.id]: patch },
    });
  }
};

export interface ProposeOptions {
  readonly own: readonly string[];
  readonly name: string | null;
  readonly display: string;
  readonly texts: ReplyTexts;
  readonly comment?: string;
}

/** Proposes another due for a task assigned by email (sends a COUNTER). */
export const proposeForAssignment = async (
  view: TaskAssignmentView,
  due: TaskDue,
  options: ProposeOptions
): Promise<void> => {
  // Only what the card offered: a task assigned to the user.
  if (view.action !== "answer") {
    throw new Error("Not a task assigned to this user");
  }
  await sendTaskCounter(view.task, due, options);
};

export interface DelegateOptions {
  readonly own: readonly string[];
  readonly name: string | null;
  readonly display: string;
  readonly texts: { readonly reply: ReplyTexts; readonly request: ReplyTexts };
  readonly comment?: string;
}

/** The delegate is already in the task (the organizer or an assignee). */
export class DelegateTakenError extends Error {
  constructor() {
    super("That person already has this task");
    this.name = "DelegateTakenError";
  }
}

/**
 * Passes a task on to `delegate`: the user's copy (if stored) says it is
 * delegated, then the REPLY and the delegate's REQUEST are mailed. Works
 * for a task in the user's tasks or one only in an email.
 */
export const delegateTask = async (
  task: RawTask,
  stored: boolean,
  delegate: AssigneeInput,
  options: DelegateOptions
): Promise<void> => {
  const me = assignmentOf(task, options.own)?.me ?? null;
  if (me === null) {
    throw new Error("The task isn't assigned to this user");
  }
  const delegation = delegationOf(task, me.key, delegate, options.own);
  if (delegation === null) {
    throw new DelegateTakenError();
  }
  if (stored) {
    await applyTaskChange({
      accountId: await ownAccount(),
      update: { [task.id]: delegation.patch },
    });
  }
  const due = dueOf(task);
  await sendTaskDelegation(task, delegation, {
    ...options,
    due: due === null ? null : itipDue(due, options.display),
  });
};

/** Passes on a task assigned by email (its stored copy, if any). */
export const delegateAssignment = async (
  view: TaskAssignmentView,
  delegate: AssigneeInput,
  options: DelegateOptions
): Promise<void> => {
  // Only what the card offered: a task assigned to the user.
  if (view.action !== "answer") {
    throw new Error("Not a task assigned to this user");
  }
  const storedMe =
    view.stored === null
      ? null
      : (assignmentOf(view.stored, options.own)?.me ?? null);
  await (view.stored !== null && storedMe !== null
    ? delegateTask(view.stored, true, delegate, options)
    : delegateTask(view.task, false, delegate, options));
};

/**
 * The organizer accepts the proposed due: the task moves there and the
 * server (or the bridge) mails the change to every assignee
 * (`sendSchedulingMessages`), as for any change.
 */
export const acceptTaskCounter = async (
  view: TaskAssignmentView,
  display: string
): Promise<void> => {
  const proposed = dueOf(view.task);
  const patch =
    view.action !== "counter" ||
    view.stored === null ||
    proposed === null ||
    view.counterAuthor === null
      ? null
      : counterDuePatch(view.stored, proposed, display);
  if (view.stored === null || patch === null) {
    throw new Error("Not a proposal for the user's task");
  }
  await applyTaskChange({
    accountId: await ownAccount(),
    notify: true,
    update: { [view.stored.id]: patch },
  });
};

/** The organizer keeps the due: a DECLINECOUNTER to who proposed. */
export const declineTaskCounter = async (
  view: TaskAssignmentView,
  options: {
    readonly own: readonly string[];
    readonly texts: ReplyTexts;
    readonly comment?: string;
  }
): Promise<void> => {
  if (view.action !== "counter") {
    throw new Error("Not a proposal for the user's task");
  }
  const author = view.counterAuthor;
  if (view.stored === null || author === null) {
    throw new Error("Not a proposal for the user's task");
  }
  await sendTaskDeclineCounter(
    view.stored,
    { email: author.email, name: author.name },
    options
  );
};
