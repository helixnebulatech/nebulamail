// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Scheduled send: RFC 4865
// FUTURERELEASE, `HOLDUNTIL` in the EmailSubmission envelope. The server
// keeps the message (pending submission) until then: Stalwart, or the
// bridge, which holds it itself. Without FUTURERELEASE it
// isn't offered.
//
// The limit is the server's: `maxDelayedSend` (RFC 8621 §1.3.2) and the
// FUTURERELEASE interval argument (RFC 4865 §3, seconds), whichever is
// lower, and its latest date-time argument. Stalwart 1.0.1 advertises 30
// days but its queue refuses more than 7 (checked 2026-11-02)
// the refusal says "exceeds maximum of <date>", which is
// remembered for this session so the UI stops offering later times.
// Times are checked here and again by the server; the client's clock is
// never the last word.

import type { CalendarDate, Time } from "@internationalized/date";
import {
  CalendarDateTime,
  fromAbsolute,
  getDayOfWeek,
  toCalendarDate,
  toZoned,
} from "@internationalized/date";
import type {
  AccountId,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  JmapClient,
  SubmissionAccountCapability,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  SEND_WHEN_SIGNED_OUT,
  submissionCapability,
} from "@nebulamail/jmap-client";

import { getClient } from "./client";
import { ScheduleError } from "./schedule-errors";
import type { ScheduleProblem } from "./schedule-errors";
import { canSendSignedOut } from "./send-signed-out";

export { ScheduleError, scheduleFailure } from "./schedule-errors";
export type { ScheduleFailure, ScheduleProblem } from "./schedule-errors";

/** How far ahead the server lets us schedule. */
export interface ScheduleLimit {
  /** Seconds from now (the lower of the advertised ones). */
  readonly maxSeconds: number;
  /** An absolute latest time, if the server gave one. */
  readonly latest: Date | null;
}

/** A scheduled time must be at least this far ahead. */
export const MIN_LEAD_MS = 60_000;
/** Quick times closer than this aren't offered ("this afternoon" at 13:58). */
const QUICK_LEAD_MS = 10 * 60_000;
/**
 * Kept below the server's maximum: our clock and the server's differ a
 * little, and a time right at the edge would be refused.
 */
const EDGE_MS = 5 * 60_000;

const SECONDS = /^\d{1,10}$/u;
/** RFC 3339 in UTC, as Stalwart writes it ("…maximum of 2026-11-09T20:06:18Z"). */
const UTC_DATE_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/u;

const parseUtc = (value: string | undefined): Date | null => {
  if (value === undefined || !UTC_DATE_TIME.test(value)) {
    return null;
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
};

/**
 * The limit from the account's submission capability, or null when it
 * can't schedule (no FUTURERELEASE, or `maxDelayedSend` 0).
 */
export const scheduleLimit = (
  capability: SubmissionAccountCapability | undefined
): ScheduleLimit | null => {
  const args = capability?.submissionExtensions.FUTURERELEASE;
  if (capability === undefined || args === undefined) {
    return null;
  }
  const interval =
    args[0] !== undefined && SECONDS.test(args[0]) ? Number(args[0]) : 0;
  const candidates = [capability.maxDelayedSend, interval].filter(
    (value) => value > 0
  );
  // RFC 8621: maxDelayedSend 0 means no delayed send at all.
  if (capability.maxDelayedSend <= 0 || candidates.length === 0) {
    return null;
  }
  return { latest: parseUtc(args[1]), maxSeconds: Math.min(...candidates) };
};

/**
 * How far ahead (seconds) a refusal said the server really takes, per
 * account. Kept as an interval, not a date: Stalwart's maximum is "now +
 * 7 days" written as a date, so a date learned once would shrink the
 * window every day a tab stays open.
 */
const learned = new Map<string, number>();

/**
 * The maximum in a refusal ("…exceeds maximum of 2026-11-09T20:06:18Z."),
 * Stalwart's wording; null for any other text.
 */
export const maximumInRefusal = (
  description: string | null | undefined
): Date | null => {
  const match =
    /exceeds maximum of (?<at>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z)\.?$/u.exec(
      description ?? ""
    );
  return parseUtc(match?.groups?.at);
};

/**
 * Remembers the server's real maximum (from a refusal at `now`) for this
 * session, as seconds from then.
 */
export const learnMaximum = (
  accountId: string,
  at: Date,
  now: Date = new Date()
): void => {
  const seconds = Math.floor((at.getTime() - now.getTime()) / 1000);
  if (seconds > 0) {
    learned.set(accountId, seconds);
  }
};

/** Only for tests. */
export const forgetLearned = (): void => {
  learned.clear();
};

/** The account's limit, with what a refusal taught us (see top). */
export const limitFor = (client: JmapClient): ScheduleLimit | null => {
  const accountId = client.primaryAccountId();
  const limit = scheduleLimit(submissionCapability(client.session, accountId));
  const known = learned.get(accountId);
  if (limit === null || known === undefined) {
    return limit;
  }
  return { ...limit, maxSeconds: Math.min(limit.maxSeconds, known) };
};

/** "lun, 3 nov, 8:00": weekday, day, month and time, in the user's zone. */
export const formatSendAt = (at: Date | string, locale: string): string => {
  const date = typeof at === "string" ? new Date(at) : at;
  return Number.isNaN(date.getTime())
    ? ""
    : new Intl.DateTimeFormat(locale, {
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
        month: "short",
        weekday: "short",
      }).format(date);
};

/** The latest time that can be chosen now. */
export const latestAllowed = (now: Date, limit: ScheduleLimit): Date => {
  const byInterval = now.getTime() + limit.maxSeconds * 1000 - EDGE_MS;
  const byDate =
    limit.latest === null
      ? Number.POSITIVE_INFINITY
      : limit.latest.getTime() - EDGE_MS;
  return new Date(Math.min(byInterval, byDate));
};

/** Whether `at` can be scheduled now; the problem otherwise. */
export const checkSendAt = (
  at: Date,
  now: Date,
  limit: ScheduleLimit
): ScheduleProblem | null => {
  const time = at.getTime();
  if (!Number.isFinite(time)) {
    return "invalid";
  }
  if (time < now.getTime() + MIN_LEAD_MS) {
    return "past";
  }
  return time > latestAllowed(now, limit).getTime() ? "tooFar" : null;
};

/**
 * `HOLDUNTIL`'s value (RFC 4865 §3: RFC 3339 date-time), in UTC and whole
 * seconds: "2026-11-03T07:00:00Z".
 */
export const holdUntil = (at: Date): string => {
  const seconds = new Date(Math.floor(at.getTime() / 1000) * 1000);
  return seconds.toISOString().replace(/\.\d{3}Z$/u, "Z");
};

export type QuickTimeId =
  | "thisAfternoon"
  | "tomorrowMorning"
  | "tomorrowAfternoon"
  | "monday";

export interface QuickTime {
  readonly id: QuickTimeId;
  readonly at: Date;
}

const MORNING = 8;
const AFTERNOON = 14;
/** getDayOfWeek with an en-US locale: Sunday is 0, Monday 1. */
const MONDAY = 1;

/**
 * The quick choices, in the user's time zone: this afternoon (14:00),
 * tomorrow morning (8:00), tomorrow afternoon (14:00) and next Monday
 * morning (8:00). Ones too close or past are left out, and Monday when it
 * is tomorrow (it would repeat "tomorrow morning").
 */
export const quickTimes = (now: Date, zone: string): QuickTime[] => {
  const today = toCalendarDate(fromAbsolute(now.getTime(), zone));
  const at = (days: number, hour: number): Date => {
    const day = today.add({ days });
    return toZoned(
      new CalendarDateTime(day.year, day.month, day.day, hour, 0),
      zone
    ).toDate();
  };
  const weekday = getDayOfWeek(today, "en-US");
  // Days to the next Monday after today (1–7).
  const toMonday = ((MONDAY - weekday + 7) % 7) + (weekday === MONDAY ? 7 : 0);
  const times: QuickTime[] = [
    { at: at(0, AFTERNOON), id: "thisAfternoon" },
    { at: at(1, MORNING), id: "tomorrowMorning" },
    { at: at(1, AFTERNOON), id: "tomorrowAfternoon" },
    ...(toMonday === 1
      ? []
      : [{ at: at(toMonday, MORNING), id: "monday" as const }]),
  ];
  return times.filter(
    (time) => time.at.getTime() >= now.getTime() + QUICK_LEAD_MS
  );
};

/**
 * The instant of a date and time picked in `zone` (both needed). Times
 * that don't exist (a DST gap) move forward, like the calendar's.
 */
export const pickedTime = (
  date: CalendarDate | null,
  time: Time | null,
  zone: string
): Date | null =>
  date === null || time === null
    ? null
    : toZoned(
        new CalendarDateTime(
          date.year,
          date.month,
          date.day,
          time.hour,
          time.minute
        ),
        zone
      ).toDate();

// ---------------------------------------------------------------- server

/** A message waiting on the server to go out. */
export interface ScheduledEmail {
  readonly submissionId: EmailSubmissionId;
  readonly emailId: EmailId;
  readonly identityId: IdentityId;
  readonly sendAt: string;
  readonly subject: string;
  /** The envelope's recipients (Bcc included), for the list. */
  readonly recipients: readonly string[];
  /** The envelope sender; null if the server derived the envelope. */
  readonly mailFrom: string | null;
  /**
   * Sent with `HOLDUNTIL` (a scheduled send), not `HOLDFOR` (undo send's
   * few seconds). Stalwart keeps the parameters; a server that drops them
   * leaves this false.
   */
  readonly holdUntil: boolean;
  /**
   * It goes out even if the user signs out first (the bridge's
   * `sendWhenSignedOut`, send-signed-out.ts); false elsewhere.
   */
  readonly sendWhenSignedOut: boolean;
}

/** At most this many in the list (Stalwart's `maxObjectsInGet` is 500). */
const MAX_SCHEDULED = 200;

const accountOf = (client: JmapClient): AccountId => client.primaryAccountId();

/** Whether the account can schedule messages at all. */
export const canSchedule = (client: JmapClient): boolean =>
  limitFor(client) !== null;

/**
 * The account's pending submissions, soonest first, with their email's
 * subject. One request: query (`undoStatus: pending`), get, Email/get.
 */
export const listScheduled = async (): Promise<ScheduledEmail[]> => {
  const client = await getClient();
  if (!canSchedule(client)) {
    return [];
  }
  const accountId = accountOf(client);
  // Only the bridge knows the property (another server would refuse it).
  const signedOut = canSendSignedOut(client);
  const [, submissions, emails] = await client.request((b) => {
    if (signedOut) {
      b.using(CAPABILITIES.bridge);
    }
    const query = b.call("EmailSubmission/query", {
      accountId,
      filter: { undoStatus: "pending" },
      limit: MAX_SCHEDULED,
      sort: [{ isAscending: true, property: "sentAt" }],
    });
    const get = b.call("EmailSubmission/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: [
        "emailId",
        "envelope",
        "identityId",
        "sendAt",
        "undoStatus",
        ...(signedOut ? [SEND_WHEN_SIGNED_OUT] : []),
      ],
    });
    const email = b.call("Email/get", {
      "#ids": get.ref("/list/*/emailId"),
      accountId,
      properties: ["subject"],
    });
    return [query, get, email] as const;
  });
  const subjects = new Map(
    emails.list.map((email) => [email.id, email.subject ?? ""])
  );
  return submissions.list
    .filter((submission) => submission.undoStatus === "pending")
    .map((submission) => ({
      emailId: submission.emailId,
      identityId: submission.identityId,
      holdUntil:
        submission.envelope?.mailFrom.parameters?.HOLDUNTIL !== undefined,
      mailFrom: submission.envelope?.mailFrom.email ?? null,
      recipients: (submission.envelope?.rcptTo ?? []).map((rcpt) => rcpt.email),
      sendAt: submission.sendAt,
      sendWhenSignedOut: submission[SEND_WHEN_SIGNED_OUT] === true,
      subject: subjects.get(submission.emailId) ?? "",
      submissionId: submission.id,
    }))
    .toSorted((a, b) => Date.parse(a.sendAt) - Date.parse(b.sendAt));
};

/**
 * Per email of the «Programados» list, when it goes out: what marks it
 * «Programado» in Sent from the list already loaded, with
 * no request per row. Undo send's held messages (`HOLDFOR`, a few
 * seconds) aren't scheduled: without `HOLDUNTIL` only a time more than a
 * minute ahead counts. An email submitted twice shows the soonest.
 */
export const scheduledByEmail = (
  list: readonly ScheduledEmail[],
  now: Date
): ReadonlyMap<EmailId, string> => {
  const result = new Map<EmailId, string>();
  const soonest = new Map<EmailId, number>();
  for (const item of list) {
    const at = Date.parse(item.sendAt);
    if (!Number.isFinite(at) || at <= now.getTime()) {
      continue;
    }
    if (!item.holdUntil && at < now.getTime() + MIN_LEAD_MS) {
      continue;
    }
    const seen = soonest.get(item.emailId);
    if (seen === undefined || at < seen) {
      soonest.set(item.emailId, at);
      result.set(item.emailId, item.sendAt);
    }
  }
  return result;
};

/**
 * Cancels a pending submission. If the server won't, says whether it was
 * already cancelled (another tab, the undo toast) or already sent.
 */
export const cancelPending = async (
  client: JmapClient,
  submissionId: EmailSubmissionId
): Promise<void> => {
  const accountId = accountOf(client);
  const [result] = await client.request((b) => [
    b.call("EmailSubmission/set", {
      accountId,
      update: { [submissionId]: { undoStatus: "canceled" } },
    }),
  ]);
  if (result.updated[submissionId] !== undefined) {
    return;
  }
  const [got] = await client.request((b) => [
    b.call("EmailSubmission/get", {
      accountId,
      ids: [submissionId],
      properties: ["undoStatus"],
    }),
  ]);
  throw new ScheduleError({
    kind:
      got.list[0]?.undoStatus === "canceled"
        ? "alreadyCanceled"
        : "alreadySent",
  });
};

/** Whether the email has a submission still waiting to go out. */
export const hasPending = async (
  client: JmapClient,
  emailId: EmailId
): Promise<boolean> => {
  const [query] = await client.request((b) => [
    b.call("EmailSubmission/query", {
      accountId: accountOf(client),
      filter: { emailIds: [emailId], undoStatus: "pending" },
    }),
  ]);
  return query.ids.length > 0;
};

/**
 * Moves a scheduled message to another time: cancels the pending
 * submission, then submits the same email with the new `HOLDUNTIL`. Never
 * the other way round: if the cancel came too late, a second submission
 * would send it twice. If the new submission is refused, the email is put
 * back in Drafts (by the caller, compose.ts `rescheduleEmail`) so it
 * isn't lost.
 */
export const reschedule = async (
  scheduled: ScheduledEmail,
  at: Date
): Promise<{
  readonly submissionId: EmailSubmissionId;
  readonly sendAt: string;
}> => {
  const client = await getClient();
  const limit = limitFor(client);
  if (limit === null) {
    throw new ScheduleError({ kind: "time", problem: "invalid" });
  }
  const problem = checkSendAt(at, new Date(), limit);
  if (problem !== null) {
    throw new ScheduleError({ kind: "time", problem });
  }
  if (scheduled.mailFrom === null || scheduled.recipients.length === 0) {
    // Without the envelope we can't say who gets it again.
    throw new ScheduleError({ kind: "time", problem: "invalid" });
  }
  await cancelPending(client, scheduled.submissionId);
  const accountId = accountOf(client);
  // The new submission goes out after sign-out exactly as the old one.
  const signedOut = canSendSignedOut(client);
  const submitAgain = async () => {
    const [answer] = await client.request((b) => {
      if (signedOut) {
        b.using(CAPABILITIES.bridge);
      }
      return [
        b.call("EmailSubmission/set", {
          accountId,
          create: {
            again: {
              ...(signedOut
                ? { [SEND_WHEN_SIGNED_OUT]: scheduled.sendWhenSignedOut }
                : {}),
              emailId: scheduled.emailId,
              envelope: {
                mailFrom: {
                  email: scheduled.mailFrom ?? "",
                  parameters: { HOLDUNTIL: holdUntil(at) },
                },
                rcptTo: scheduled.recipients.map((email) => ({ email })),
              },
              identityId: scheduled.identityId,
            },
          },
        }),
      ] as const;
    });
    return answer;
  };
  const result = await submitAgain().catch((error: unknown): never => {
    // The old time is gone: the caller must not leave it in Sent.
    throw new ScheduleError({ kind: "interrupted" }, null, { cause: error });
  });
  const created = result.created.again;
  if (created === undefined) {
    const refusal = result.notCreated.again;
    const maximum = maximumInRefusal(refusal?.description);
    if (maximum !== null) {
      learnMaximum(accountId, maximum);
    }
    throw new ScheduleError({ kind: "refused", maximum }, refusal?.description);
  }
  return {
    sendAt: created.sendAt ?? holdUntil(at),
    submissionId: created.id,
  };
};
