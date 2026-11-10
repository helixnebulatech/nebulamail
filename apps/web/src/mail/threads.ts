// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Conversations: which emails of a thread a view shows, what
// a list row says about them, which emails each action changes, and which
// cards of an open conversation start expanded. Pure, so it is tested
// without a server.

import type { EmailId, MailboxId } from "@nebulamail/jmap-client";

import type { ConversationRow, Person, ThreadMember } from "./api";

/** The folders a view needs to know to decide what a thread shows. */
export interface ThreadContext {
  /** The open folder; null for Starred and search results. */
  readonly mailboxId: MailboxId | null;
  readonly trashId: MailboxId | null;
  readonly junkId: MailboxId | null;
  readonly sentId: MailboxId | null;
  readonly draftsId: MailboxId | null;
}

export const NO_CONTEXT: ThreadContext = {
  draftsId: null,
  junkId: null,
  mailboxId: null,
  sentId: null,
  trashId: null,
};

type Member = Pick<
  ThreadMember,
  "id" | "mailboxIds" | "draft" | "unread" | "flagged" | "from"
>;

const isIn = (email: Member, mailboxId: MailboxId | null): boolean =>
  mailboxId !== null && email.mailboxIds.includes(mailboxId);

const viewing = (context: ThreadContext, mailboxId: MailboxId | null) =>
  context.mailboxId !== null && context.mailboxId === mailboxId;

/**
 * Whether a view shows this email of a conversation. Trash shows only what
 * is in Trash, Spam only what is in Spam; every other view (folders,
 * Starred, search) hides what is in Trash or Spam. Your own replies in
 * Sent show everywhere: that's the point of a conversation.
 */
export const isShown = (email: Member, context: ThreadContext): boolean => {
  if (viewing(context, context.trashId)) {
    return isIn(email, context.trashId);
  }
  if (viewing(context, context.junkId)) {
    return isIn(email, context.junkId);
  }
  return !isIn(email, context.trashId) && !isIn(email, context.junkId);
};

/** The emails a view shows, in thread order; `keep` (the opened one) always. */
export const shownEmails = <T extends Member>(
  emails: readonly T[],
  context: ThreadContext,
  keep?: EmailId
): T[] =>
  emails.filter((email) => email.id === keep || isShown(email, context));

/** The newest sent or received email (drafts don't count). */
export const latestEmail = <T extends Member>(
  emails: readonly T[]
): T | undefined => emails.findLast((email) => !email.draft) ?? emails.at(-1);

const ids = (emails: readonly Member[]): EmailId[] =>
  emails.map((email) => email.id);

// --- List rows --------------------------------------------------------------

export interface ConversationView {
  /** Emails shown in the conversation (the "3" next to the sender). */
  readonly count: number;
  /** Any shown email unread / starred / with attachments (like Gmail). */
  readonly unread: boolean;
  readonly flagged: boolean;
  readonly hasAttachment: boolean;
  /** Distinct senders, oldest first. */
  readonly participants: readonly Person[];
}

/** The emails of a row, or the row alone when its thread isn't known. */
const rowEmails = (row: ConversationRow): readonly ThreadMember[] =>
  row.members.some((member) => member.id === row.id) ? row.members : [row];

/** The shown, non-draft emails of a row (drafts count in Drafts). */
const rowShown = (
  row: ConversationRow,
  context: ThreadContext
): ThreadMember[] => {
  const inDrafts = viewing(context, context.draftsId);
  return shownEmails(rowEmails(row), context, row.id).filter(
    (email) => inDrafts || !email.draft || email.id === row.id
  );
};

export const conversationView = (
  row: ConversationRow,
  context: ThreadContext
): ConversationView => {
  const emails = rowShown(row, context);
  const participants = new Map<string, Person>();
  for (const email of emails) {
    const [sender] = email.from;
    const key = sender?.email.toLowerCase();
    if (sender !== undefined && key !== undefined && !participants.has(key)) {
      participants.set(key, sender);
    }
  }
  return {
    count: emails.length,
    flagged: emails.some((email) => email.flagged),
    hasAttachment: emails.some((email) => email.hasAttachment),
    participants: [...participants.values()],
    unread: emails.some((email) => email.unread),
  };
};

/**
 * Up to three names, like Gmail: "Ana, Bob, me"; with more, the first and
 * the last two: "Ana … Carla, me".
 */
export const participantNames = (names: readonly string[]): string => {
  if (names.length <= 3) {
    return names.join(", ");
  }
  return `${names[0] ?? ""} … ${names.slice(-2).join(", ")}`;
};

// --- What actions change ----------------------------------------------------

export type ThreadAction = "move" | "read" | "unread" | "flag" | "unflag";

/**
 * The emails an action on a whole conversation changes:
 *
 * - move (archive, trash, spam, move to): the ones in the open folder; in
 *   Starred and search, the shown ones outside Sent and Drafts. Your own
 *   replies stay in Sent when you archive a conversation (like Gmail).
 * - read: every shown unread one. unread: the newest (like Gmail).
 * - flag: the newest. unflag: every shown starred one.
 *
 * Never empty: falls back to `fallback` (the row or the opened email).
 */
export const actionTargets = (
  emails: readonly Member[],
  context: ThreadContext,
  action: ThreadAction,
  fallback: EmailId
): EmailId[] => {
  const shown = shownEmails(emails, context, fallback);
  const latest = latestEmail(shown);
  const picked = (() => {
    switch (action) {
      case "move": {
        return context.mailboxId === null
          ? shown.filter(
              (email) =>
                !isIn(email, context.sentId) && !isIn(email, context.draftsId)
            )
          : shown.filter((email) => isIn(email, context.mailboxId));
      }
      case "read": {
        return shown.filter((email) => email.unread);
      }
      case "unflag": {
        return shown.filter((email) => email.flagged);
      }
      case "unread":
      case "flag": {
        return latest === undefined ? [] : [latest];
      }
      default: {
        return action satisfies never;
      }
    }
  })();
  return picked.length > 0 ? ids(picked) : [fallback];
};

/** `actionTargets` for list rows (bulk selection, hover actions). */
export const rowTargets = (
  rows: readonly ConversationRow[],
  context: ThreadContext,
  action: ThreadAction
): EmailId[] => [
  ...new Set(
    rows.flatMap((row) =>
      actionTargets(rowShown(row, context), context, action, row.id)
    )
  ),
];

// --- Open conversation ------------------------------------------------------

/** Which cards start expanded, fixed when the conversation first loads. */
export interface Expansion {
  readonly known: ReadonlySet<EmailId>;
  readonly open: ReadonlySet<EmailId>;
}

/** Unread ones, the newest and the opened one; drafts stay one line. */
export const initialExpansion = (
  emails: readonly Member[],
  target: EmailId
): Expansion => {
  const latest = latestEmail(emails);
  const open = emails.filter(
    (email) =>
      !email.draft &&
      (email.unread || email.id === target || email.id === latest?.id)
  );
  return { known: new Set(ids(emails)), open: new Set(ids(open)) };
};

/**
 * Whether a card is expanded: the user's choice, else the initial one. An
 * email that arrives while the conversation is open (push) expands if it
 * is unread or the newest. Marking as read doesn't collapse anything.
 */
export const isExpanded = (
  email: Member,
  latestId: EmailId | undefined,
  expansion: Expansion,
  overrides: ReadonlyMap<EmailId, boolean>
): boolean => {
  if (email.draft) {
    return false;
  }
  const chosen = overrides.get(email.id);
  if (chosen !== undefined) {
    return chosen;
  }
  if (expansion.known.has(email.id)) {
    return expansion.open.has(email.id);
  }
  return email.unread || email.id === latestId;
};
