// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Folders for the "Move to" actions: the id (what the script uses) and the
// full path: Stalwart's fallback name ("Parent/Child") if the id is gone,
// or on the bridge the name the IMAP server knows it by, the only one its
// Sieve takes ("INBOX.Parent.Child").

import type { MailboxSummary } from "../../mail/api";
import type { FilterSet } from "./model";

export interface FolderOption {
  readonly id: string;
  /** The name a `fileinto` on this server uses. */
  readonly path: string;
  /** Depth, for indenting the option. */
  readonly depth: number;
  readonly mailbox: MailboxSummary;
}

export type ImapNames = ReadonlyMap<string, string> | null;

const MAX_DEPTH = 20;

export const folderOptions = (
  mailboxes: readonly MailboxSummary[],
  /** Mailbox id → IMAP name, on the bridge. */
  imapNames: ImapNames = null
): FolderOption[] => {
  const byId = new Map(mailboxes.map((box) => [box.id, box]));
  return mailboxes
    .filter((box) => box.role !== "drafts")
    .map((box) => {
      const names = [box.name];
      let parent = box.parentId === null ? undefined : byId.get(box.parentId);
      while (parent !== undefined && names.length < MAX_DEPTH) {
        names.unshift(parent.name);
        parent =
          parent.parentId === null ? undefined : byId.get(parent.parentId);
      }
      return {
        depth: names.length - 1,
        id: box.id,
        mailbox: box,
        path: imapNames?.get(box.id) ?? names.join("/"),
      };
    });
};

/**
 * Fills in the folder id of actions read by path only (`fileinto "A/B"`,
 * written for servers without `mailboxid`); unknown paths stay without id.
 */
export const resolveFolders = (
  set: FilterSet,
  folders: readonly Pick<FolderOption, "id" | "path">[]
): FilterSet => ({
  ...set,
  rules: set.rules.map((rule) => ({
    ...rule,
    actions: rule.actions.map((action) => {
      if (
        (action.kind !== "move" && action.kind !== "copy") ||
        action.mailboxId !== ""
      ) {
        return action;
      }
      const folder = folders.find((item) => item.path === action.mailboxName);
      return folder === undefined
        ? action
        : { ...action, mailboxId: folder.id };
    }),
  })),
});
