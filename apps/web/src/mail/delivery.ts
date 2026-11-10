// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What happened to the mail we sent (RFC 8621 §7 `deliveryStatus`): per
// recipient, delivered, bounced (with the server's reply), still queued or
// read (an MDN arrived, `displayed: yes`). Stalwart fills it in as its
// queue delivers; it has no MDN. The bridge from the DSN and MDN reports that come back.
//
// Submissions are found with `EmailSubmission/query` by `emailIds`. The
// bridge doesn't have that method (only `/get`, with the submissions of
// the last day), so on `unknownMethod` all of them are fetched and
// filtered here. No submission capability, no submissions or any error:
// nothing is shown.

import type {
  AccountId,
  EmailId,
  EmailSubmission,
  JmapClient,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  hasCapability,
  isMethodError,
} from "@nebulamail/jmap-client";

import { getClient } from "./client";
import { chunk } from "./thread-ids";

export type DeliveryState =
  /** The receiving server accepted it. */
  | "delivered"
  /** It bounced (`delivered: no`). */
  | "failed"
  /** Not delivered yet: the server keeps trying. */
  | "queued"
  /** The recipient's client sent a read receipt. */
  | "read"
  /** Sent, nothing known yet (the default without DSN). */
  | "unknown";

export interface RecipientStatus {
  readonly email: string;
  readonly state: DeliveryState;
  /** The SMTP reply, e.g. "550 5.1.1 user unknown" (bounces mostly). */
  readonly reply: string | null;
}

type StatusMap = NonNullable<EmailSubmission["deliveryStatus"]>;

/**
 * A reply worth showing: one line, without controls, not too long. A 2xx
 * reply is the server taking the message, not why it bounced: the bridge
 * keeps that one when the report gives no SMTP reason (Postfix says
 * "X-Postfix; unknown user"), and "250 2.0.0 Ok: queued" under "Not
 * delivered" would only confuse.
 */
const MAX_REPLY = 300;
const ACCEPTED = /^2\d\d(?:[\s-]|$)/u;
/**
 * Stalwart 1.0 writes some replies as "Code: 250, Enhanced code: 2.1.5,
 * Message: OK" (checked live 2026-10-30): read back as "250 2.1.5 OK".
 */
const SPELLED_OUT =
  /^Code: (?<code>\d{3}), Enhanced code: (?<enhanced>\d\.\d{1,3}\.\d{1,3}), Message: (?<message>.*)$/u;
const cleanReply = (reply: string): string | null => {
  const line = reply
    .replaceAll(/\p{Cc}+/gu, " ")
    .replaceAll(/\s+/gu, " ")
    .trim();
  const spelled = SPELLED_OUT.exec(line)?.groups;
  const text =
    spelled === undefined
      ? line
      : `${spelled.code ?? ""} ${spelled.enhanced ?? ""} ${spelled.message ?? ""}`.trim();
  if (text === "" || ACCEPTED.test(text)) {
    return null;
  }
  return text.length > MAX_REPLY ? `${text.slice(0, MAX_REPLY)}…` : text;
};

export const recipientState = (status: StatusMap[string]): DeliveryState => {
  if (status.delivered === "no") {
    return "failed";
  }
  if (status.displayed === "yes") {
    return "read";
  }
  if (status.delivered === "queued") {
    return "queued";
  }
  return status.delivered === "yes" ? "delivered" : "unknown";
};

/** One entry per recipient, in the server's order. */
export const recipientStatuses = (
  delivery: StatusMap | null | undefined
): RecipientStatus[] =>
  Object.entries(delivery ?? {}).map(([email, status]) => {
    const state = recipientState(status);
    return {
      email,
      // A "250 OK" for a delivered message says nothing new.
      reply:
        state === "failed" || state === "queued"
          ? cleanReply(status.smtpReply)
          : null,
      state,
    };
  });

/**
 * The state of the whole message, for a list row: any bounce first, then
 * anything still on its way; "read" only if everyone read it; "delivered"
 * if it reached everyone. Otherwise (or without recipients) nothing.
 */
export const overallState = (
  recipients: readonly RecipientStatus[]
): DeliveryState => {
  const states = new Set(recipients.map((recipient) => recipient.state));
  if (states.size === 0) {
    return "unknown";
  }
  if (states.has("failed")) {
    return "failed";
  }
  if (states.has("queued")) {
    return "queued";
  }
  if (states.has("unknown")) {
    return "unknown";
  }
  return states.size === 1 && states.has("read") ? "read" : "delivered";
};

type SubmissionFields = Pick<
  EmailSubmission,
  "emailId" | "deliveryStatus" | "sendAt" | "undoStatus"
>;

/**
 * Per email, its recipients' status. An email sent twice (rare) shows the
 * newest submission; cancelled ones (undo send) don't count.
 */
export const statusesByEmail = (
  submissions: readonly SubmissionFields[],
  wanted: ReadonlySet<EmailId>
): Map<EmailId, RecipientStatus[]> => {
  const newest = new Map<EmailId, SubmissionFields>();
  for (const submission of submissions) {
    if (
      !wanted.has(submission.emailId) ||
      submission.undoStatus === "canceled"
    ) {
      continue;
    }
    const seen = newest.get(submission.emailId);
    if (seen === undefined || seen.sendAt < submission.sendAt) {
      newest.set(submission.emailId, submission);
    }
  }
  const result = new Map<EmailId, RecipientStatus[]>();
  for (const [id, submission] of newest) {
    const recipients = recipientStatuses(submission.deliveryStatus);
    if (recipients.length > 0) {
      result.set(id, recipients);
    }
  }
  return result;
};

const PROPERTIES = [
  "emailId",
  "deliveryStatus",
  "sendAt",
  "undoStatus",
] as const;

const byQuery = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly EmailId[]
): Promise<SubmissionFields[]> => {
  const responses = await client.request((b) =>
    chunk(ids, client.session.core.maxObjectsInGet).map((part) => {
      const query = b.call("EmailSubmission/query", {
        accountId,
        filter: { emailIds: part },
      });
      return b.call("EmailSubmission/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: [...PROPERTIES],
      });
    })
  );
  return responses.flatMap((response) => response.list);
};

/** Without `/query` (the bridge): every submission the server keeps. */
const fetchAll = async (
  client: JmapClient,
  accountId: AccountId
): Promise<SubmissionFields[]> => {
  const [response] = await client.request((b) => [
    b.call("EmailSubmission/get", {
      accountId,
      ids: null,
      properties: [...PROPERTIES],
    }),
  ]);
  return response.list;
};

/**
 * The same answer for calls made at the same time (the list's pages all
 * ask at once after a push): one EmailSubmission/get, not one per page.
 */
const allInFlight = new WeakMap<
  JmapClient,
  Map<AccountId, Promise<SubmissionFields[]>>
>();

const byGetAll = async (
  client: JmapClient,
  accountId: AccountId
): Promise<SubmissionFields[]> => {
  let byAccount = allInFlight.get(client);
  if (byAccount === undefined) {
    byAccount = new Map();
    allInFlight.set(client, byAccount);
  }
  const running = byAccount.get(accountId);
  if (running !== undefined) {
    return await running;
  }
  const map = byAccount;
  const pending = fetchAll(client, accountId);
  map.set(accountId, pending);
  try {
    return await pending;
  } finally {
    map.delete(accountId);
  }
};

/**
 * Servers that answered `unknownMethod` to EmailSubmission/query (RFC 8620
 * §3.6.2; the bridge has no /query): not asked again with this client.
 */
const withoutQuery = new WeakSet<JmapClient>();

/**
 * The submissions of these emails (any undo status). Without
 * `EmailSubmission/query` (the bridge) it reads all of them and keeps only
 * these emails': the account's other sends never count.
 */
export const submissionsOf = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly EmailId[]
): Promise<SubmissionFields[]> => {
  let submissions: SubmissionFields[];
  if (withoutQuery.has(client)) {
    submissions = await byGetAll(client, accountId);
  } else {
    try {
      submissions = await byQuery(client, accountId, ids);
    } catch (error) {
      if (!isMethodError(error, "unknownMethod")) {
        throw error;
      }
      withoutQuery.add(client);
      submissions = await byGetAll(client, accountId);
    }
  }
  const wanted = new Set(ids);
  return submissions.filter((submission) => wanted.has(submission.emailId));
};

/** Whether this session can tell us about deliveries at all. */
export const hasSubmissions = (client: JmapClient): boolean =>
  hasCapability(
    client.session,
    CAPABILITIES.submission,
    client.primaryAccountId()
  );

/**
 * The delivery status of these emails of the user's own account (only
 * there can we have sent them). Emails without one are left out.
 */
/** Each email's recipients and how delivery went for them. */
export type DeliveryStatuses = ReadonlyMap<EmailId, readonly RecipientStatus[]>;

export const getDeliveryStatuses = async (
  ids: readonly EmailId[]
): Promise<DeliveryStatuses> => {
  if (ids.length === 0) {
    return new Map();
  }
  const client = await getClient();
  if (!hasSubmissions(client)) {
    return new Map();
  }
  const submissions = await submissionsOf(
    client,
    client.primaryAccountId(),
    ids
  );
  return statusesByEmail(submissions, new Set(ids));
};
