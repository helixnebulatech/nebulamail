// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the user may do with the emails of a folder (RFC 8621 §2.4,
// `Mailbox.myRights`). Our own folders allow everything; a folder someone
// shares with us may be read-only, or let us mark but not move. The UI
// hides what the server would reject instead of showing an error after.
//
// Moving takes `mayRemoveItems` on the folder it leaves and `mayAddItems`
// on the one it enters; `$seen` needs `maySetSeen`, any other keyword
// (`$flagged`, `$junk`…) `maySetKeywords`.

import type { MailboxRole, MailboxSummary } from "./api";

/** The rights of RFC 8621 the mail UI looks at. */
export interface MailRights {
  readonly mayReadItems: boolean;
  readonly mayAddItems: boolean;
  readonly mayRemoveItems: boolean;
  readonly maySetSeen: boolean;
  readonly maySetKeywords: boolean;
  readonly mayCreateChild: boolean;
  readonly mayRename: boolean;
  readonly mayDelete: boolean;
}

export const ALL_RIGHTS: MailRights = {
  mayAddItems: true,
  mayCreateChild: true,
  mayDelete: true,
  mayReadItems: true,
  mayRemoveItems: true,
  mayRename: true,
  maySetKeywords: true,
  maySetSeen: true,
};

/**
 * From the server's `myRights`. Missing (a server that leaves it out):
 * everything, and the server still has the last word.
 */
export const toMailRights = (
  rights: Partial<Record<keyof MailRights, boolean>> | null | undefined
): MailRights =>
  rights === null || rights === undefined
    ? ALL_RIGHTS
    : {
        mayAddItems: rights.mayAddItems !== false,
        mayCreateChild: rights.mayCreateChild !== false,
        mayDelete: rights.mayDelete !== false,
        mayReadItems: rights.mayReadItems !== false,
        mayRemoveItems: rights.mayRemoveItems !== false,
        mayRename: rights.mayRename !== false,
        maySetKeywords: rights.maySetKeywords !== false,
        maySetSeen: rights.maySetSeen !== false,
      };

/** What the email actions of a list or the reader may offer. */
export interface MailPermissions {
  readonly archive: boolean;
  readonly trash: boolean;
  readonly spam: boolean;
  readonly notSpam: boolean;
  /** Move to another folder (there is at least one it may go to). */
  readonly move: boolean;
  readonly setSeen: boolean;
  readonly setFlagged: boolean;
  /** Folders emails of this one may be moved to (never Drafts). */
  readonly moveTargets: readonly MailboxSummary[];
}

const accepts = (mailbox: MailboxSummary | undefined): boolean =>
  mailbox !== undefined && mailbox.rights.mayAddItems;

/**
 * The permissions for emails in `source` (the open folder; null: a view
 * like "Starred" or a search, whose emails can be anywhere: the rights of
 * the account's folders are only checked on the target).
 *
 * In our own account a missing Archive or Junk folder is created when
 * needed (use-mail-actions.ts); in a shared one it never is.
 */
export const mailPermissions = (
  mailboxes: readonly MailboxSummary[],
  source: MailboxSummary | null
): MailPermissions => {
  const from = source?.rights ?? ALL_RIGHTS;
  const own = mailboxes.every((box) => box.accountId === null);
  const role = (name: MailboxRole): MailboxSummary | undefined =>
    mailboxes.find((box) => box.role === name);
  const reachable = (name: MailboxRole, creatable: boolean): boolean => {
    const target = role(name);
    if (target === undefined) {
      return own && creatable;
    }
    return accepts(target);
  };
  const moveTargets = from.mayRemoveItems
    ? mailboxes.filter(
        (box) => box.id !== source?.id && box.role !== "drafts" && accepts(box)
      )
    : [];
  const leave = from.mayRemoveItems;
  // Spam and "not spam" also change the $junk / $notjunk keywords.
  const keywords = from.maySetKeywords;
  return {
    archive: leave && reachable("archive", true),
    move: moveTargets.length > 0,
    moveTargets,
    notSpam: leave && keywords && reachable("inbox", false),
    setFlagged: from.maySetKeywords,
    setSeen: from.maySetSeen,
    spam: leave && keywords && reachable("junk", true),
    trash: leave && reachable("trash", false),
  };
};
