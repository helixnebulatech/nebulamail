// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The mail side of assigned tasks: the
// assignee's REPLY, mailed by the app (iMIP, saved in Sent), as for
// events (calendar/invitations.ts). The organizer's REQUEST and CANCEL
// are the server's or the bridge's (`sendSchedulingMessages`).
//
// The app also mails, with Stalwart and with the bridge alike, an
// assignee's proposal of another due (COUNTER), the organizer's "keep the
// due" (DECLINECOUNTER) and a delegation (REPLY to the organizer, REQUEST
// to the delegate). Neither Stalwart (1.0.1: COUNTER is an "unsupported
// method") nor the bridge's `CalendarEvent/counter` (events only) does
// that for tasks.

import { sendItip } from "../calendar/invitations";
import type { ReplyTexts } from "../calendar/invitations";
import type { ItipDue } from "../calendar/itip";
import {
  MAX_COMMENT,
  buildTaskCounter,
  buildTaskDeclineCounter,
  buildTaskDelegation,
  buildTaskReply,
  buildTaskRequest,
} from "../calendar/itip";
import type { Person } from "../calendar/model";
import type {
  Delegation,
  TaskAnswer,
  TaskAssignee,
  WithParticipants,
} from "./assignment";
import { ASSIGNEE_PARTSTAT, PARTSTAT_OF, assignmentOf } from "./assignment";
import type { SeriesTask } from "./itip-series";
import { repeats, taskSeries } from "./itip-series";
import type { TaskDue } from "./model";
import { dueInstant } from "./model";

/** The REPLY email's texts, in the user's language (from the UI). */
export type TaskReplyTexts = (
  answer: TaskAnswer,
  title: string,
  name: string
) => ReplyTexts;

export interface ReplyableTask extends WithParticipants {
  readonly uid?: string | null | undefined;
  readonly title?: string | null | undefined;
  readonly sequence?: number | null | undefined;
}

/** A due as sent: a day, or the instant in UTC (null: can't be worked out). */
export const itipDue = (due: TaskDue, display: string): ItipDue | null => {
  if (due.kind === "date") {
    return { date: due.date, kind: "date" };
  }
  const at = dueInstant(due, display);
  return at === null ? null : { at, kind: "instant" };
};

const cleanComment = (comment: string | undefined): string | undefined => {
  const note = comment?.trim().slice(0, MAX_COMMENT) ?? "";
  return note === "" ? undefined : note;
};

/** The note, as the last lines of the email's text. */
const withComment = (
  texts: ReplyTexts,
  comment: string | undefined
): ReplyTexts =>
  comment === undefined
    ? texts
    : { ...texts, text: `${texts.text}\n\n${comment}` };

/** Who the user is in a task assigned to them, and who assigned it. */
const replyParties = (
  task: ReplyableTask,
  own: readonly string[],
  name: string | null
): {
  readonly me: TaskAssignee;
  readonly attendee: Person;
  readonly organizer: Person;
  readonly uid: string;
} | null => {
  const assignment = assignmentOf(task, own);
  const uid = task.uid ?? "";
  if (
    assignment?.me === null ||
    assignment?.me === undefined ||
    assignment.organizer === null ||
    assignment.iAmOrganizer ||
    uid === ""
  ) {
    return null;
  }
  return {
    attendee: { email: assignment.me.email, name: name ?? assignment.me.name },
    me: assignment.me,
    organizer: assignment.organizer,
    uid,
  };
};

const sequenceOf = (task: ReplyableTask): number =>
  Math.max(0, Math.trunc(task.sequence ?? 0));

/**
 * Proposes another due to whoever assigned the task (COUNTER). The task
 * doesn't change: the organizer decides.
 */
export const sendTaskCounter = async (
  task: ReplyableTask,
  due: TaskDue,
  options: {
    readonly own: readonly string[];
    readonly name: string | null;
    /** The user's zone (a floating due is read in it). */
    readonly display: string;
    readonly texts: ReplyTexts;
    readonly comment?: string;
  }
): Promise<void> => {
  const parties = replyParties(task, options.own, options.name);
  const proposed = itipDue(due, options.display);
  if (parties === null || proposed === null) {
    throw new Error("The task isn't assigned to this user");
  }
  const comment = cleanComment(options.comment);
  await sendItip({
    from: parties.attendee,
    ics: buildTaskCounter({
      attendee: parties.attendee,
      due: proposed,
      now: Date.now(),
      organizer: parties.organizer,
      partstat: ASSIGNEE_PARTSTAT[parties.me.status],
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: sequenceOf(task),
      title: task.title ?? "",
      uid: parties.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "COUNTER",
    texts: withComment(options.texts, comment),
    to: parties.organizer,
  });
};

/** What a delegated task tells the delegate. */
export interface DelegatedTask extends ReplyableTask, SeriesTask {
  readonly description?: string | null | undefined;
  readonly descriptionContentType?: string | null | undefined;
}

/**
 * Passes the task on: a REPLY to the organizer (DELEGATED, with the
 * delegate) and a REQUEST to the delegate (the task, with the delegation).
 * `delegation` is what the user's copy now says (`delegationOf`).
 */
export const sendTaskDelegation = async (
  task: DelegatedTask,
  delegation: Delegation,
  options: {
    readonly own: readonly string[];
    readonly name: string | null;
    readonly due: ItipDue | null;
    readonly texts: {
      readonly reply: ReplyTexts;
      readonly request: ReplyTexts;
    };
    readonly comment?: string;
  }
): Promise<void> => {
  const parties = replyParties(task, options.own, options.name);
  const after = assignmentOf(
    { ...task, participants: delegation.participants },
    options.own
  );
  const delegate = after?.assignees.find(
    (assignee) => assignee.key === delegation.delegateKey
  );
  if (parties === null || after === null || delegate === undefined) {
    throw new Error("The task isn't assigned to this user");
  }
  // A repeating task goes as the whole series (`mayDelegate` checked it
  // can be written).
  const series = repeats(task) ? taskSeries(task) : null;
  if (repeats(task) && series === null) {
    throw new Error("The repeating task can't be passed on");
  }
  const comment = cleanComment(options.comment);
  const title = task.title ?? "";
  const to: Person = { email: delegate.email, name: delegate.name };
  await sendItip({
    from: parties.attendee,
    ics: buildTaskDelegation({
      attendee: parties.attendee,
      delegate: to,
      delegatedFrom: parties.me.delegatedFrom,
      now: Date.now(),
      organizer: parties.organizer,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: sequenceOf(task),
      title,
      uid: parties.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "REPLY",
    texts: withComment(options.texts.reply, comment),
    to: parties.organizer,
  });
  const html = task.descriptionContentType
    ?.toLowerCase()
    .startsWith("text/html");
  await sendItip({
    from: parties.attendee,
    ics: buildTaskRequest({
      attendees: after.assignees.map((assignee) => ({
        delegatedFrom: assignee.delegatedFrom,
        delegatedTo: assignee.delegatedTo,
        email: assignee.email,
        name: assignee.isMe ? parties.attendee.name : assignee.name,
        partstat: ASSIGNEE_PARTSTAT[assignee.status],
        rsvp: assignee.key === delegation.delegateKey,
      })),
      // An HTML description isn't sent (it would arrive as markup).
      description: html === true ? null : (task.description ?? null),
      due: options.due,
      now: Date.now(),
      organizer: parties.organizer,
      sequence: sequenceOf(task),
      series,
      title,
      uid: parties.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "REQUEST",
    texts: withComment(options.texts.request, comment),
    to,
  });
};

/** The organizer keeps the due: a DECLINECOUNTER to who proposed. */
export const sendTaskDeclineCounter = async (
  task: ReplyableTask,
  author: Person,
  options: {
    readonly own: readonly string[];
    readonly texts: ReplyTexts;
    readonly comment?: string;
  }
): Promise<void> => {
  const assignment = assignmentOf(task, options.own);
  const uid = task.uid ?? "";
  if (
    assignment?.iAmOrganizer !== true ||
    assignment.organizer === null ||
    uid === ""
  ) {
    throw new Error("Not a task the user assigned");
  }
  const comment = cleanComment(options.comment);
  await sendItip({
    from: assignment.organizer,
    ics: buildTaskDeclineCounter({
      attendee: author,
      now: Date.now(),
      organizer: assignment.organizer,
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: sequenceOf(task),
      uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "DECLINECOUNTER",
    texts: withComment(options.texts, comment),
    to: author,
  });
};

/**
 * Mails the user's answer to whoever assigned the task. Does nothing for
 * a task that isn't assigned to the user by someone else.
 */
export const sendTaskReply = async (
  task: ReplyableTask,
  answer: TaskAnswer,
  options: {
    readonly own: readonly string[];
    /** The user's name for the organizer (null: the address). */
    readonly name: string | null;
    readonly texts: TaskReplyTexts;
  }
): Promise<void> => {
  const assignment = assignmentOf(task, options.own);
  const uid = task.uid ?? "";
  if (
    assignment?.me === null ||
    assignment?.me === undefined ||
    assignment.organizer === null ||
    assignment.iAmOrganizer ||
    uid === ""
  ) {
    return;
  }
  const attendee: Person = {
    email: assignment.me.email,
    name: options.name ?? assignment.me.name,
  };
  const title = task.title ?? "";
  await sendItip({
    from: attendee,
    ics: buildTaskReply({
      attendee,
      now: Date.now(),
      organizer: assignment.organizer,
      partstat: PARTSTAT_OF[answer],
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: Math.max(0, Math.trunc(task.sequence ?? 0)),
      title,
      uid,
    }),
    method: "REPLY",
    texts: options.texts(answer, title, attendee.name ?? attendee.email),
    to: assignment.organizer,
  });
};
