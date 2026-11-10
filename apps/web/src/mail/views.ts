// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which emails a list shows: the folder in the URL (`/mail/inbox`,
// `/mail/<mailbox id>`, `/mail/starred`), the split-inbox tab and the
// read/unread filter, combined into one JMAP filter. Splits are data so
// user-defined ones can be added later (Superhuman).

import type {
  EmailFilter,
  EmailFilterCondition,
  MailboxId,
} from "@nebulamail/jmap-client";

import { mailboxParamFor, parseMailboxParam } from "./account-scope";
import type { MailboxRole, MailboxSummary } from "./api";

export const ROLE_ALIASES = [
  "inbox",
  "drafts",
  "sent",
  "archive",
  "junk",
  "trash",
] as const satisfies readonly MailboxRole[];

export const STARRED = "starred";

export type MailboxView =
  | { readonly kind: "mailbox"; readonly mailbox: MailboxSummary }
  | { readonly kind: "starred" }
  /** A role alias the account has no folder for yet (e.g. archive). */
  | { readonly kind: "missing"; readonly role: MailboxRole };

export const isRoleAlias = (value: string): value is MailboxRole =>
  ROLE_ALIASES.some((role) => role === value);

/**
 * The URL segment for a mailbox: its role if it has one, else its id; with
 * the account in front for shared ones (account-scope.ts).
 */
export const mailboxParam = (
  mailbox: Pick<MailboxSummary, "accountId" | "id" | "role">
): string => mailboxParamFor(mailbox.accountId, mailbox.role ?? mailbox.id);

/**
 * null: no such mailbox (or the mailboxes aren't loaded). `mailboxes` are
 * those of the account in `segment`.
 */
export const resolveView = (
  segment: string,
  mailboxes: readonly MailboxSummary[] | undefined
): MailboxView | null => {
  const param = parseMailboxParam(segment).mailbox;
  if (param === STARRED) {
    return { kind: "starred" };
  }
  if (mailboxes === undefined) {
    return null;
  }
  if (isRoleAlias(param)) {
    const mailbox = mailboxes.find((box) => box.role === param);
    return mailbox === undefined
      ? { kind: "missing", role: param }
      : { kind: "mailbox", mailbox };
  }
  const mailbox = mailboxes.find((box) => box.id === param);
  return mailbox === undefined ? null : { kind: "mailbox", mailbox };
};

// --- Split inbox ------------------------------------------------------------

export interface Split {
  readonly id: string;
  /** Extra condition on top of the inbox; null = the whole inbox. */
  readonly condition: EmailFilterCondition | null;
  /** Show the number of unread emails in the tab. */
  readonly countUnread: boolean;
}

const IMPORTANT: Split = {
  condition: null,
  countUnread: false,
  id: "important",
};

/** Phase 1 splits; the labels live in the UI (i18n by id). */
export const DEFAULT_SPLITS: readonly Split[] = [
  IMPORTANT,
  // The unread count goes on "Unread", where you'd look for it. While
  // "Important" is the whole inbox, both would show the same number.
  { condition: { notKeyword: "$seen" }, countUnread: true, id: "unread" },
  { condition: { hasAttachment: true }, countUnread: false, id: "attachments" },
];

export const findSplit = (
  splits: readonly Split[],
  id: string | undefined
): Split => splits.find((split) => split.id === id) ?? splits[0] ?? IMPORTANT;

// --- Read filter ------------------------------------------------------------

export const READ_FILTERS = ["all", "unread", "read", "attachment"] as const;
export type ReadFilter = (typeof READ_FILTERS)[number];

const READ_CONDITIONS: Readonly<
  Record<ReadFilter, EmailFilterCondition | null>
> = {
  all: null,
  attachment: { hasAttachment: true },
  read: { hasKeyword: "$seen" },
  unread: { notKeyword: "$seen" },
};

/** All conditions together (AND); a single one stays as is. */
export const combine = (
  conditions: readonly (EmailFilterCondition | null)[]
): EmailFilter => {
  const present = conditions.filter(
    (condition): condition is EmailFilterCondition => condition !== null
  );
  const [only] = present;
  if (present.length === 1 && only !== undefined) {
    return only;
  }
  return present.length === 0 ? {} : { conditions: present, operator: "AND" };
};

/**
 * The filter for a list. Starred leaves out Trash and Spam, like other
 * nebulamails. Splits only apply to the inbox.
 */
export const listFilter = (
  view: Exclude<MailboxView, { readonly kind: "missing" }>,
  {
    excluded = [],
    readFilter = "all",
    split = null,
  }: {
    readonly excluded?: readonly MailboxId[];
    readonly readFilter?: ReadFilter;
    readonly split?: Split | null;
  } = {}
): EmailFilter => {
  const base: EmailFilterCondition =
    view.kind === "starred"
      ? {
          hasKeyword: "$flagged",
          ...(excluded.length > 0 ? { inMailboxOtherThan: excluded } : {}),
        }
      : { inMailbox: view.mailbox.id };
  const splitCondition =
    view.kind === "mailbox" && view.mailbox.role === "inbox"
      ? (split?.condition ?? null)
      : null;
  return combine([base, splitCondition, READ_CONDITIONS[readFilter]]);
};
