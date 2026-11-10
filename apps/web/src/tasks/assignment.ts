// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Assigned tasks: a task with
// participants, as JSCalendar has them (RFC 8984 §4.4.6) and iTIP sends
// them (RFC 5546 §3.4, VTODO). Pure, with tests.
//
// - The user who assigns is the organizer (`organizerCalendarAddress`,
//   and a participant with `owner`, like events: `participantsOf`).
//   Every change the organizer makes to such a task goes with
//   `sendSchedulingMessages` (`scheduleChange`): the server (Stalwart, a
//   CalDAV server that schedules) or the bridge sends the REQUEST or
//   CANCEL to the assignees.
// - An assignee answers like an invitation (accept, decline) and, when
//   completing or reopening the task, the app writes their status in the
//   stored copy *without* scheduling and mails the REPLY itself (PARTSTAT
//   COMPLETED, IN-PROCESS…), as it does for events.
// - A participant's status: `participationStatus` (accepted, declined…)
//   and, for tasks, `progress` (in-process, completed…). calcard reads
//   PARTSTAT=COMPLETED as `accepted` + `progress: "completed"`.
// - An assignee may also propose another due (COUNTER, RFC 5546 §3.4.7),
//   which the organizer accepts (a change with scheduling) or declines
//   (DECLINECOUNTER), or pass the task on to someone else (delegating:
//   PARTSTAT=DELEGATED with `delegatedTo`, the delegate with
//   `delegatedFrom`). The app mails those itself, like the REPLY.
//
// Names and addresses are untrusted text (shown with `cleanDisplayText`
// in `<bdi>`).

import { fromAbsolute } from "@internationalized/date";
import type { CalendarEventId, Participant } from "@nebulamail/jmap-client";

import { utcDateTime } from "../calendar/alerts";
import type { Me } from "../calendar/editing";
import { participantsOf } from "../calendar/editing";
import type { InvitationSender, ResponseStamp } from "../calendar/itip";
import {
  addressOf,
  isNewerResponse,
  mayAct,
  ownSet,
  responseStamp,
  responseStampOf,
} from "../calendar/itip";
import type { Person } from "../calendar/model";
import { formatLocalDateTime, parseLocalDateTime } from "../calendar/time";
import type { Patch, TaskChange } from "./editing";
import { mapPatch } from "./editing";
import type { SeriesTask } from "./itip-series";
import { repeats, taskSeries } from "./itip-series";
import type { RawTask, TaskDue } from "./model";
import { dueInstant, dueOf } from "./model";

/** Where an assignee is with the task. */
export type AssigneeStatus =
  | "needs-action"
  | "accepted"
  | "tentative"
  | "declined"
  | "delegated"
  | "in-process"
  | "completed"
  | "failed";

/** What an assignee can answer. */
export type TaskAnswer = "accepted" | "declined" | "in-process" | "completed";

export interface TaskAssignee extends Person {
  /** Key in the task's `participants`. */
  readonly key: string;
  readonly status: AssigneeStatus;
  readonly isMe: boolean;
  /** Who they passed the task on to (DELEGATED-TO), if anyone. */
  readonly delegatedTo: string | null;
  /** Who passed it on to them (DELEGATED-FROM), if anyone. */
  readonly delegatedFrom: string | null;
}

export interface Assignment {
  /** Who assigned it (null: participants but no organizer). */
  readonly organizer: Person | null;
  readonly iAmOrganizer: boolean;
  /** Everyone but the organizer. */
  readonly assignees: readonly TaskAssignee[];
  /** The user among the assignees (null: not assigned to the user). */
  readonly me: TaskAssignee | null;
}

/** What `assignmentOf` reads (a stored or a parsed task). */
export interface WithParticipants {
  readonly participants?:
    | Readonly<Record<string, Participant>>
    | null
    | undefined;
  readonly organizerCalendarAddress?: string | null | undefined;
  /** RFC 8984 §5.2.5 ("cancelled": taken back). */
  readonly progress?: string | null | undefined;
  /** A REPLY's SEQUENCE and DTSTAMP order it (RFC 5546 §2.1.5). */
  readonly sequence?: number | null | undefined;
  readonly updated?: string | null | undefined;
}

const addressOfParticipant = (participant: Participant): string | null =>
  addressOf(participant.calendarAddress) ?? addressOf(participant.email);

/** The status of a participant of a task. */
export const assigneeStatus = (participant: Participant): AssigneeStatus => {
  const status = participant.participationStatus?.toLowerCase() ?? "";
  if (
    status === "declined" ||
    status === "tentative" ||
    status === "delegated"
  ) {
    return status;
  }
  const progress = participant.progress?.toLowerCase() ?? "";
  if (
    progress === "in-process" ||
    progress === "completed" ||
    progress === "failed"
  ) {
    return progress;
  }
  return status === "accepted" ? "accepted" : "needs-action";
};

/** The first address of `delegatedTo` / `delegatedFrom` (`mailto:` keys). */
const firstAddress = (
  map: Readonly<Record<string, boolean>> | null | undefined
): string | null => {
  for (const [uri, on] of Object.entries(map ?? {})) {
    const email = on ? addressOf(uri) : null;
    if (email !== null) {
      return email;
    }
  }
  return null;
};

const nameOf = (participant: Participant): string | null => {
  const name = participant.name?.trim() ?? "";
  return name === "" ? null : name;
};

/**
 * Who assigned the task and to whom; null for a task nobody else is in
 * (no participants, or only the organizer).
 */
export const assignmentOf = (
  task: WithParticipants,
  ownAddresses: readonly string[]
): Assignment | null => {
  const own = ownSet(ownAddresses);
  const organizerAddress = addressOf(task.organizerCalendarAddress);
  let organizer: Person | null = null;
  const assignees: TaskAssignee[] = [];
  for (const [key, participant] of Object.entries(task.participants ?? {})) {
    const email = addressOfParticipant(participant);
    if (email === null) {
      continue;
    }
    const isOrganizer =
      email === organizerAddress ||
      (organizerAddress === null && participant.roles?.owner === true);
    if (isOrganizer) {
      organizer ??= { email, name: nameOf(participant) };
      continue;
    }
    if (!assignees.some((other) => other.email === email)) {
      assignees.push({
        delegatedFrom: firstAddress(participant.delegatedFrom),
        delegatedTo: firstAddress(participant.delegatedTo),
        email,
        isMe: own.has(email),
        key,
        name: nameOf(participant),
        status: assigneeStatus(participant),
      });
    }
  }
  if (organizer === null && organizerAddress !== null) {
    organizer = { email: organizerAddress, name: null };
  }
  if (assignees.length === 0) {
    return null;
  }
  return {
    assignees,
    iAmOrganizer: organizer !== null && own.has(organizer.email),
    me: assignees.find((assignee) => assignee.isMe) ?? null,
    organizer,
  };
};

/** Someone to assign the task to (from the contacts or typed). */
export interface AssigneeInput {
  readonly email: string;
  readonly name: string | null;
}

/**
 * The patch that assigns the task to `people` (none: not assigned any
 * more). The user becomes the organizer; people already there keep their
 * key and status.
 */
export const assigneesPatch = (
  task: WithParticipants,
  people: readonly AssigneeInput[],
  me: Me
): Patch => {
  const participants = participantsOf(
    people.map((person) => ({ ...person, optional: false })),
    task.participants,
    me
  );
  return {
    organizerCalendarAddress:
      participants === null ? null : `mailto:${me.email.toLowerCase()}`,
    participants,
  };
};

/** RFC 8984 §1.4.4 UTCDateTime ("Z"; a fraction only if not zero). */
const UTC_DATE_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d*[1-9])?Z$/u;

const isUtcDateTime = (value: unknown): value is string =>
  typeof value === "string" &&
  UTC_DATE_TIME.test(value) &&
  Number.isFinite(Date.parse(value));

/**
 * A participant's `progress` (RFC 8984 §4.4.6; null: none) and the
 * `progressUpdated` that goes with it (§5.2.6, "the date-time the
 * progress property was last set on this participant"): `at` when the
 * progress changes, the one it had when it doesn't, none without a
 * progress.
 */
const progressOf = (
  before: Participant,
  progress: string | null,
  at: string
): Pick<Participant, "progress" | "progressUpdated"> => {
  if (progress === null) {
    return {};
  }
  const kept =
    before.progress === progress && isUtcDateTime(before.progressUpdated)
      ? before.progressUpdated
      : null;
  return { progress, progressUpdated: kept ?? at };
};

/** The participant after the user's answer (`now`: epoch ms). */
const answered = (
  participant: Participant,
  answer: TaskAnswer,
  now: number
): Participant => {
  const { progress: _old, progressUpdated: _at, ...rest } = participant;
  if (answer === "declined") {
    // §4.4.6: no progress unless accepted.
    return { ...rest, participationStatus: "declined" };
  }
  return {
    ...rest,
    participationStatus: "accepted",
    ...progressOf(
      participant,
      answer === "accepted" ? null : answer,
      utcDateTime(now)
    ),
  };
};

/**
 * The patch that records the user's answer in their copy of the task, with
 * the time their progress changed (`now`, epoch ms).
 */
export const answerPatch = (
  task: RawTask,
  key: string,
  answer: TaskAnswer,
  now: number = Date.now()
): Patch => {
  const participant = task.participants?.[key];
  if (participant === undefined) {
    return {};
  }
  return mapPatch("participants", task.participants, {
    [key]: answered(participant, answer, now),
  });
};

/** The answer that goes with a task's `progress` (completing, reopening). */
export const answerOfProgress = (progress: unknown): TaskAnswer => {
  if (progress === "completed" || progress === "in-process") {
    return progress;
  }
  return "accepted";
};

/** The PARTSTAT of an answer in a REPLY (RFC 5545 §3.2.12, VTODO). */
export const PARTSTAT_OF: Readonly<Record<TaskAnswer, string>> = {
  accepted: "ACCEPTED",
  completed: "COMPLETED",
  declined: "DECLINED",
  "in-process": "IN-PROCESS",
};

/** An assignee's status as a VTODO PARTSTAT (in a REQUEST). */
export const ASSIGNEE_PARTSTAT: Readonly<Record<AssigneeStatus, string>> = {
  accepted: "ACCEPTED",
  completed: "COMPLETED",
  declined: "DECLINED",
  delegated: "DELEGATED",
  // Not a PARTSTAT (JSCalendar only): the delegate need not know.
  failed: "NEEDS-ACTION",
  "in-process": "IN-PROCESS",
  "needs-action": "NEEDS-ACTION",
  tentative: "TENTATIVE",
};

/**
 * Whether the user may pass this task on: assigned to them by someone
 * else and not already passed on. A repeating task goes as a whole series
 * (one occurrence alone isn't passed on), only if the series can be
 * written exactly into the REQUEST to the delegate (`taskSeries`).
 */
export const mayDelegate = (
  task: WithParticipants & SeriesTask,
  own: readonly string[]
): boolean => {
  const assignment = assignmentOf(task, own);
  return (
    assignment?.me !== null &&
    assignment?.me !== undefined &&
    !assignment.iAmOrganizer &&
    assignment.organizer !== null &&
    assignment.me.status !== "delegated" &&
    (!repeats(task) || taskSeries(task) !== null)
  );
};

export interface Delegation {
  /** The patch for the user's own copy (they stay in it as delegated). */
  readonly patch: Patch;
  /** The participants after it (for the REQUEST to the delegate). */
  readonly participants: Readonly<Record<string, Participant>>;
  /** The delegate's key in them. */
  readonly delegateKey: string;
}

const newDelegateKey = (taken: Readonly<Record<string, unknown>>): string => {
  let key = `d${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
  while (key in taken) {
    key = `d${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
  }
  return key;
};

/**
 * The user passes the task on to `delegate` (RFC 5546 §3.2.2.3): their
 * entry says `delegated` with `delegatedTo`, and the delegate comes in
 * with `delegatedFrom`, waiting for an answer. Null if the delegate is
 * already in the task (organizer or assignee) or the user isn't in it.
 */
export const delegationOf = (
  task: WithParticipants,
  myKey: string,
  delegate: AssigneeInput,
  own: readonly string[]
): Delegation | null => {
  const participants = task.participants ?? {};
  const mine = participants[myKey];
  const email = addressOf(delegate.email);
  const myEmail = mine === undefined ? null : addressOfParticipant(mine);
  if (mine === undefined || email === null || myEmail === null) {
    return null;
  }
  const taken = new Set([
    ...ownSet(own),
    ...Object.values(participants).flatMap((participant) => {
      const address = addressOfParticipant(participant);
      return address === null ? [] : [address];
    }),
    addressOf(task.organizerCalendarAddress) ?? "",
  ]);
  if (taken.has(email)) {
    return null;
  }
  const { progress: _old, progressUpdated: _at, ...rest } = mine;
  const delegated: Participant = {
    ...rest,
    delegatedTo: { [`mailto:${email}`]: true },
    participationStatus: "delegated",
  };
  const delegateKey = newDelegateKey(participants);
  const added: Participant = {
    "@type": "Participant",
    calendarAddress: `mailto:${email}`,
    delegatedFrom: { [`mailto:${myEmail}`]: true },
    expectReply: true,
    kind: "individual",
    ...(delegate.name === null || delegate.name === ""
      ? {}
      : { name: delegate.name }),
    participationStatus: "needs-action",
    roles: { attendee: true },
  };
  return {
    delegateKey,
    participants: { ...participants, [myKey]: delegated, [delegateKey]: added },
    patch: mapPatch("participants", task.participants, {
      [delegateKey]: added,
      [myKey]: delegated,
    }),
  };
};

/** A REPLY the app mails after a change (the user is an assignee). */
export interface PendingReply {
  readonly task: RawTask;
  readonly answer: TaskAnswer;
}

export interface ScheduledChange {
  readonly change: TaskChange;
  readonly replies: readonly PendingReply[];
}

/**
 * The scheduling side of a change:
 *
 * - a change to a task the user assigned to others (or a new one with
 *   assignees) goes with `notify`, so the assignees hear about it;
 * - completing or reopening a task assigned to the user records their
 *   status too and asks for a REPLY to the organizer. Only the whole task
 *   (one occurrence of a repeating task isn't answered on its own).
 */
/** Whether a patch makes the user the organizer of an assigned task. */
const patchAssigns = (patch: Patch, own: readonly string[]): boolean => {
  if (!("participants" in patch)) {
    return false;
  }
  return (
    assignmentOf(
      {
        organizerCalendarAddress:
          typeof patch.organizerCalendarAddress === "string"
            ? patch.organizerCalendarAddress
            : null,
        // A whole map the app wrote (assigneesPatch), or null.
        // oxlint-disable-next-line typescript/no-unsafe-type-assertion
        participants: patch.participants as Record<string, Participant> | null,
      },
      own
    )?.iAmOrganizer === true
  );
};

interface ScheduledUpdate {
  readonly patch: Patch;
  readonly notify: boolean;
  readonly reply: PendingReply | null;
}

/** One update of `scheduleChange`. */
const scheduleUpdate = (
  patch: Patch,
  raw: RawTask | undefined,
  own: readonly string[]
): ScheduledUpdate => {
  const assignment = raw === undefined ? null : assignmentOf(raw, own);
  // The patch itself may assign the task (or take it back: then the
  // stored one says the user organizes it).
  if (assignment?.iAmOrganizer === true || patchAssigns(patch, own)) {
    return { notify: true, patch, reply: null };
  }
  const me = assignment?.me ?? null;
  const answers =
    raw !== undefined &&
    me !== null &&
    assignment?.organizer !== null &&
    "progress" in patch &&
    !Object.keys(patch).some((path) => path.startsWith("participants"));
  if (!answers || raw === undefined || me === null) {
    return { notify: false, patch, reply: null };
  }
  const answer = answerOfProgress(patch.progress);
  // The same moment as the task's own `progressUpdated` (completeChange).
  const at =
    typeof patch.progressUpdated === "string"
      ? Date.parse(patch.progressUpdated)
      : Number.NaN;
  return {
    notify: false,
    patch: {
      ...patch,
      ...answerPatch(
        raw,
        me.key,
        answer,
        Number.isFinite(at) ? at : Date.now()
      ),
    },
    reply: { answer, task: raw },
  };
};

export const scheduleChange = (
  change: TaskChange,
  before: ReadonlyMap<CalendarEventId, RawTask>,
  own: readonly string[]
): ScheduledChange => {
  let notify = Object.values(change.create ?? {}).some(
    (body) => assignmentOf(body, own)?.iAmOrganizer === true
  );
  const replies: PendingReply[] = [];
  const update: Partial<Record<CalendarEventId, Patch>> = {};
  for (const [key, patch] of Object.entries(change.update ?? {})) {
    // Keys come from the change's own update map.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    const id = key as CalendarEventId;
    if (patch !== undefined) {
      const scheduled = scheduleUpdate(patch, before.get(id), own);
      update[id] = scheduled.patch;
      notify ||= scheduled.notify;
      if (scheduled.reply !== null) {
        replies.push(scheduled.reply);
      }
    }
  }
  notify ||= (change.destroy ?? []).some((id) => {
    const raw = before.get(id);
    return raw !== undefined && assignmentOf(raw, own)?.iAmOrganizer === true;
  });
  return {
    change: {
      ...change,
      ...(change.update === undefined ? {} : { update }),
      ...(notify ? { notify: true } : {}),
    },
    replies,
  };
};

// ---------------------------------------------------------------- in mail

/** What a parsed task copies into the user's tasks when accepted. */
const IMPORTED = [
  "@type",
  "description",
  "descriptionContentType",
  "due",
  "estimatedDuration",
  "keywords",
  "organizerCalendarAddress",
  "priority",
  "privacy",
  "progress",
  "recurrenceRule",
  "excludedRecurrenceRules",
  "sequence",
  "showWithoutTime",
  "start",
  "timeZone",
  "title",
  "uid",
] as const;

/**
 * What an occurrence of a parsed series keeps (`recurrenceOverrides`):
 * taken out, moved, renamed, done. Never the sender's alerts, links or
 * people.
 */
const IMPORTED_OVERRIDE: ReadonlySet<string> = new Set([
  "description",
  "descriptionContentType",
  "due",
  "excluded",
  "priority",
  "progress",
  "start",
  "title",
]);

const importedOverrides = (
  overrides: unknown
): Record<string, Record<string, unknown>> | null => {
  if (typeof overrides !== "object" || overrides === null) {
    return null;
  }
  const out: Record<string, Record<string, unknown>> = {};
  for (const key of Object.keys(overrides)) {
    const patch: unknown = Reflect.get(overrides, key);
    if (
      parseLocalDateTime(key) !== null &&
      typeof patch === "object" &&
      patch !== null
    ) {
      // Kept even with nothing left: it may be an added occurrence (an
      // empty patch, like an RDATE, RFC 8984 §4.3.5).
      out[key] = Object.fromEntries(
        Object.entries(patch).filter(([name]) => IMPORTED_OVERRIDE.has(name))
      );
    }
  }
  return Object.keys(out).length > 0 ? out : null;
};

/**
 * The task of an assignment email, to add to the user's tasks with their
 * answer. Like accepting an invitation: only the task's
 * own data, never the sender's alerts or links.
 */
export const taskToImport = (
  task: WithParticipants & Readonly<Record<string, unknown>>,
  myKey: string | null,
  answer: TaskAnswer,
  now: number = Date.now()
): Record<string, unknown> => {
  const copy: Record<string, unknown> = {};
  for (const key of IMPORTED) {
    const value = task[key];
    if (value !== undefined && value !== null) {
      copy[key] = value;
    }
  }
  copy["@type"] = "Task";
  // A series (a repeating task passed on): its occurrences that differ.
  const overrides =
    copy.recurrenceRule === undefined
      ? null
      : importedOverrides(task.recurrenceOverrides);
  if (overrides !== null) {
    copy.recurrenceOverrides = overrides;
  }
  // A repeating task needs a start (its due) for its rule and alerts
  // (Stalwart's quirks).
  if (
    copy.recurrenceRule !== undefined &&
    copy.start === undefined &&
    copy.due !== undefined
  ) {
    copy.start = copy.due;
  }
  const participants: Record<string, Participant> = {};
  for (const [key, participant] of Object.entries(task.participants ?? {})) {
    participants[key] =
      key === myKey ? answered(participant, answer, now) : participant;
  }
  if (Object.keys(participants).length > 0) {
    copy.participants = participants;
  }
  return copy;
};

/**
 * The people in a REPLY that an assignee of the user's task passed it on
 * to, and that aren't in the task yet.
 */
const newDelegates = (
  mine: Assignment,
  theirs: Assignment
): readonly TaskAssignee[] =>
  theirs.assignees.filter(
    (author) =>
      author.delegatedFrom !== null &&
      !mine.assignees.some((assignee) => assignee.email === author.email) &&
      mine.assignees.some(
        (assignee) =>
          assignee.email === author.delegatedFrom &&
          theirs.assignees.some(
            (other) =>
              other.email === assignee.email &&
              other.delegatedTo === author.email
          )
      )
  );

/**
 * Who applying a REPLY would add to the user's task (people an assignee
 * passed it on to): shown before applying, as the email may not be theirs.
 */
export const replyDelegates = (
  stored: WithParticipants,
  reply: WithParticipants,
  own: readonly string[]
): readonly TaskAssignee[] => {
  const mine = assignmentOf(stored, own);
  const theirs = assignmentOf(reply, own);
  return mine?.iAmOrganizer === true && theirs !== null
    ? newDelegates(mine, theirs)
    : [];
};

/**
 * A stored assignee with the status (and progress) of their REPLY. Its
 * `progressUpdated` is the REPLY's if it carries a valid one, else when the
 * organizer's copy records it (`now`).
 */
const withAnswer = (
  entry: Participant,
  answer: Participant,
  stamp: ResponseStamp,
  now: number
): Participant => {
  const {
    delegatedTo: _to,
    progress: _old,
    progressUpdated: _at,
    scheduleSequence: _sequence,
    scheduleUpdated: _updated,
    ...rest
  } = entry;
  return {
    ...rest,
    participationStatus: answer.participationStatus ?? "needs-action",
    // The response's SEQUENCE and DTSTAMP (RFC 5546 §2.1.5).
    ...responseStampOf(stamp),
    ...progressOf(
      entry,
      typeof answer.progress === "string" ? answer.progress : null,
      isUtcDateTime(answer.progressUpdated)
        ? answer.progressUpdated
        : utcDateTime(now)
    ),
    ...(answer.delegatedTo === null || answer.delegatedTo === undefined
      ? {}
      : { delegatedTo: answer.delegatedTo }),
  };
};

/** The delegate as the organizer's copy gets them from a REPLY. */
const delegateEntry = (delegate: TaskAssignee): Participant => ({
  "@type": "Participant",
  calendarAddress: `mailto:${delegate.email}`,
  // Only the assignee that passed it on (checked by `newDelegates`), not
  // every DELEGATED-FROM the REPLY lists.
  delegatedFrom:
    delegate.delegatedFrom === null
      ? {}
      : { [`mailto:${delegate.delegatedFrom}`]: true },
  expectReply: true,
  kind: "individual",
  ...(delegate.name === null ? {} : { name: delegate.name }),
  // Their own answer comes in their own email: never what the assignee
  // who passed it on says they answered.
  participationStatus: "needs-action",
  roles: { attendee: true },
});

/**
 * The patch that writes the answers of a REPLY into the user's task (the
 * organizer's copy): each assignee in the reply that is in the task gets
 * its status (and progress) from it. Empty when nothing differs.
 */
export const replyPatch = (
  stored: WithParticipants,
  reply: WithParticipants,
  own: readonly string[],
  now: number = Date.now()
): Patch => {
  const mine = assignmentOf(stored, own);
  const theirs = assignmentOf(reply, own);
  if (mine === null || theirs === null) {
    return {};
  }
  const changes: Record<string, Participant> = {};
  const stamp = responseStamp(reply);
  for (const author of theirs.assignees) {
    const current = mine.assignees.find(
      (assignee) => assignee.email === author.email
    );
    const entry = stored.participants?.[current?.key ?? ""];
    const answer = reply.participants?.[author.key];
    if (
      current !== undefined &&
      entry !== undefined &&
      answer !== undefined &&
      current.status !== author.status &&
      // Never an older response over a newer one (RFC 5546 §2.1.5).
      isNewerResponse(entry, stamp)
    ) {
      changes[current.key] = withAnswer(entry, answer, stamp, now);
    }
  }
  // Someone an assignee passed the task on to (in their REPLY): added, as
  // Stalwart does with an iMIP REPLY (RFC 5546 §3.2.2.3).
  for (const delegate of newDelegates(mine, theirs)) {
    const entry = reply.participants?.[delegate.key];
    if (entry !== undefined) {
      const taken = { ...stored.participants, ...changes };
      const key = delegate.key in taken ? newDelegateKey(taken) : delegate.key;
      changes[key] = delegateEntry(delegate);
    }
  }
  return mapPatch("participants", stored.participants, changes);
};

/**
 * Who proposes in a COUNTER for the user's task: the first attendee of the
 * message that is an assignee of the stored task (anyone can send one).
 */
export const counterAuthor = (
  counter: Assignment | null,
  stored: WithParticipants,
  own: readonly string[]
): TaskAssignee | null => {
  const mine = assignmentOf(stored, own);
  if (mine?.iAmOrganizer !== true || counter === null) {
    return null;
  }
  return (
    counter.assignees.find(
      (author) =>
        !author.isMe &&
        mine.assignees.some((assignee) => assignee.email === author.email)
    ) ?? null
  );
};

/** Which buttons the assignment card shows. */
export type TaskCardAction =
  /** Accept / Decline (the task is assigned to the user). */
  | "answer"
  /** Taken back, and in the user's tasks: offer to remove it. */
  | "remove"
  /** An assignee answered the user's task: write it in the task. */
  | "apply-reply"
  /** An assignee proposes another due: accept it or keep the current one. */
  | "counter"
  | "none";

/** Whether a REPLY says something the user's task doesn't yet. */
const replyChanges = (
  reply: Assignment | null,
  stored: WithParticipants,
  own: readonly string[],
  stamp: ResponseStamp | null
): boolean => {
  const mine = assignmentOf(stored, own);
  if (mine?.iAmOrganizer !== true || reply === null) {
    return false;
  }
  const differs = reply.assignees.some((author) => {
    const current = mine.assignees.find(
      (assignee) => assignee.email === author.email
    );
    return (
      current !== undefined &&
      current.status !== author.status &&
      // An older response than the last one applied: nothing (RFC 5546
      // §2.1.5).
      (stamp === null ||
        isNewerResponse(stored.participants?.[current.key] ?? null, stamp))
    );
  });
  return differs || newDelegates(mine, reply).length > 0;
};

/** Every address in a stored task (organizer and assignees). */
const addressesOf = (task: WithParticipants | null): ReadonlySet<string> =>
  new Set(
    Object.values(task?.participants ?? {})
      .map(addressOfParticipant)
      .filter((email) => email !== null)
  );

/**
 * Whether the sender may speak for this participant of a REPLY or COUNTER:
 * it is the sender, or someone new the sender says they passed it on to.
 * Never someone already in the stored task: "Dana, delegated from me,
 * declines" is not the sender's to say.
 */
const speaksFor = (
  person: { readonly email: string; readonly delegatedFrom: string | null },
  sender: string,
  stored: ReadonlySet<string>
): boolean =>
  person.email === sender ||
  (person.delegatedFrom === sender && !stored.has(person.email));

/**
 * The participants of an emailed REPLY or COUNTER that its sender may speak
 * for (RFC 6047 §2.3: the sender is who acts): their own entry and new
 * people they say they passed it on to (`speaksFor`). Anyone can write
 * other attendees in a .ics, so a guest could answer "Dana declines" for
 * Dana (the forgery of CVE-2026-93628 in Zimbra). Empty when the sender may not act
 * (several From, failed DKIM/DMARC).
 */
export const fromSender = <T extends WithParticipants>(
  task: T,
  sender: InvitationSender,
  stored: WithParticipants | null
): T => {
  const participants: Record<string, Participant> = {};
  if (mayAct(sender)) {
    const known = addressesOf(stored);
    for (const [key, participant] of Object.entries(task.participants ?? {})) {
      const email = addressOfParticipant(participant);
      const person = {
        delegatedFrom: firstAddress(participant.delegatedFrom),
        email: email ?? "",
      };
      if (email !== null && speaksFor(person, sender.email, known)) {
        participants[key] = participant;
      }
    }
  }
  return { ...task, participants };
};

/** Whose emails may act: who sent this one, or anyone (to say why not). */
type TaskSenderCheck = InvitationSender | "any";

export interface TaskCardInput {
  readonly method: string;
  readonly cancelled: boolean;
  /** The email's assignment, whole (filtered by sender here). */
  readonly assignment: Assignment | null;
  /** The same task in the user's tasks (by UID), if there. */
  readonly stored: WithParticipants | null;
  /** The stored copy is newer than the email: it changes nothing. */
  readonly outdated: boolean;
  readonly own: readonly string[];
  /** Who sent the email (`invitationSender`). */
  readonly sender: InvitationSender;
  /** The email's SEQUENCE and DTSTAMP (`responseStamp`; absent: not checked). */
  readonly stamp?: ResponseStamp;
}

/** The stored copy's organizer sent the email (a CANCEL or an update). */
const fromStoredOrganizer = (
  stored: WithParticipants,
  own: readonly string[],
  sender: TaskSenderCheck
): boolean => {
  if (sender === "any") {
    return true;
  }
  // The stored organizer, even with no assignees left (assignmentOf is null).
  const organizer =
    addressOf(stored.organizerCalendarAddress) ??
    assignmentOf(stored, own)?.organizer?.email ??
    null;
  return mayAct(sender) && organizer !== null && organizer === sender.email;
};

/** Only the assignees the sender speaks for (see `speaksFor`). */
const sendersOnly = (
  assignment: Assignment | null,
  stored: WithParticipants,
  sender: TaskSenderCheck
): Assignment | null => {
  if (assignment === null || sender === "any") {
    return assignment;
  }
  if (!mayAct(sender)) {
    return { ...assignment, assignees: [] };
  }
  const known = addressesOf(stored);
  return {
    ...assignment,
    assignees: assignment.assignees.filter((assignee) =>
      speaksFor(assignee, sender.email, known)
    ),
  };
};

/**
 * Who proposes in an emailed COUNTER: its sender, if an assignee of the
 * user's task. Only for themselves (a delegate proposes in their own
 * email), never another attendee written in the .ics.
 */
export const senderCounterAuthor = (
  counter: Assignment | null,
  stored: WithParticipants,
  own: readonly string[],
  sender: TaskSenderCheck
): TaskAssignee | null => {
  if (counter === null || sender === "any") {
    return counterAuthor(counter, stored, own);
  }
  if (!mayAct(sender)) {
    return null;
  }
  const { email } = sender;
  return counterAuthor(
    {
      ...counter,
      assignees: counter.assignees.filter(
        (assignee) => assignee.email === email
      ),
    },
    stored,
    own
  );
};

type ActionInput = Omit<TaskCardInput, "sender">;

/** A REQUEST (new or an update of a stored task): answer it. */
const requestTaskAction = (
  { assignment, own, stored }: ActionInput,
  sender: TaskSenderCheck
): TaskCardAction => {
  // An update of a task in the user's tasks: only from its organizer.
  if (stored !== null && !fromStoredOrganizer(stored, own, sender)) {
    return "none";
  }
  return assignment?.me !== null &&
    assignment?.me !== undefined &&
    !assignment.iAmOrganizer
    ? "answer"
    : "none";
};

/** A REPLY or COUNTER to the user's task: only what the sender may say. */
const answerTaskAction = (
  input: ActionInput,
  stored: WithParticipants,
  sender: TaskSenderCheck
): TaskCardAction => {
  const assignment = sendersOnly(input.assignment, stored, sender);
  if (input.method === "reply") {
    return replyChanges(assignment, stored, input.own, input.stamp ?? null)
      ? "apply-reply"
      : "none";
  }
  if (input.method === "counter") {
    // Nothing to move in a task that was taken back.
    if (stored.progress?.toLowerCase() === "cancelled") {
      return "none";
    }
    return senderCounterAuthor(input.assignment, stored, input.own, sender) ===
      null
      ? "none"
      : "counter";
  }
  return "none";
};

const taskActionFor = (
  input: ActionInput,
  sender: TaskSenderCheck
): TaskCardAction => {
  const { stored, own } = input;
  // The user's tasks have a newer version: an old email changes nothing.
  if (input.outdated || (sender !== "any" && !mayAct(sender))) {
    return "none";
  }
  // STATUS:CANCELLED only means "taken back" in the organizer's messages:
  // a REPLY or COUNTER that says it is still an answer (like events).
  const takenBack =
    input.method === "cancel" ||
    (input.cancelled &&
      (input.method === "request" || input.method === "publish"));
  if (takenBack) {
    // Taking a task back: only its organizer (by UID anyone could).
    return stored !== null && fromStoredOrganizer(stored, own, sender)
      ? "remove"
      : "none";
  }
  if (input.method === "request" || input.method === "publish") {
    return requestTaskAction(input, sender);
  }
  return stored === null ? "none" : answerTaskAction(input, stored, sender);
};

/**
 * Which buttons the assignment card shows. Any action needs the email to
 * come from the right person: the stored organizer to take a task back or
 * update it, the answering assignee for a REPLY or a COUNTER.
 */
export const taskCardAction = (input: TaskCardInput): TaskCardAction =>
  taskActionFor(input, input.sender);

/**
 * The card would offer something if the email came from the right person.
 * For a REPLY or COUNTER, only when the sender isn't among its answers (or
 * may not act): an assignee's own answer with stale entries of others in
 * the .ics is not "from the wrong person".
 */
export const taskSenderBlocked = (input: TaskCardInput): boolean => {
  if (
    taskActionFor(input, input.sender) !== "none" ||
    taskActionFor(input, "any") === "none"
  ) {
    return false;
  }
  if (input.method !== "reply" && input.method !== "counter") {
    return true;
  }
  const { sender } = input;
  return (
    !mayAct(sender) ||
    !(input.assignment?.assignees ?? []).some(
      (assignee) => assignee.email === sender.email
    )
  );
};

/** A start after the new due, or of the other kind, moves with it. */
const startMoves = (stored: RawTask, due: TaskDue, value: string): boolean => {
  if (typeof stored.start !== "string") {
    return false;
  }
  const before = dueOf(stored);
  const start = parseLocalDateTime(stored.start);
  const end = parseLocalDateTime(value);
  return (
    (before !== null && before.kind !== due.kind) ||
    (start !== null && end !== null && start.compare(end) > 0)
  );
};

/**
 * The patch that moves the user's task to the due an assignee proposed
 * (the organizer accepts a COUNTER). A time is written in the task's own
 * zone (the proposal comes in UTC); a start after the new due, or of the
 * other kind (day / time), moves with it. Null if the due can't be read.
 */
export const counterDuePatch = (
  stored: RawTask,
  proposed: TaskDue,
  display: string
): Patch | null => {
  let due: TaskDue = proposed;
  if (proposed.kind === "datetime") {
    const at = dueInstant(proposed, display);
    if (at === null) {
      return null;
    }
    const zone = stored.timeZone ?? display;
    due = {
      kind: "datetime",
      local: formatLocalDateTime(fromAbsolute(at, zone)),
      timeZone: zone,
    };
  }
  const value = due.kind === "date" ? `${due.date}T00:00:00` : due.local;
  const patch: Record<string, unknown> =
    due.kind === "date"
      ? { due: value, showWithoutTime: true }
      : { due: value, showWithoutTime: false, timeZone: due.timeZone };
  if (
    due.kind === "date" &&
    stored.timeZone !== null &&
    stored.timeZone !== undefined
  ) {
    patch.timeZone = null;
  }
  if (startMoves(stored, due, value)) {
    patch.start = value;
  }
  return patch;
};
