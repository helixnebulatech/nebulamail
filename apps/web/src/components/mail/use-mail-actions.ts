// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What you can do to emails from the list, the reader and the palette:
// archive, delete, spam, move, read/unread, star. Optimistic (hooks.ts);
// failures roll back and show a toast. Callers pass the emails a
// conversation action changes (threads.ts `actionTargets`).
//
// In a folder someone shares with us, its rights (`myRights`, rights.ts)
// decide what is offered: the UI hides the rest, and these functions do
// nothing for it (shortcuts and ⌘K land here too).

import type { EmailId, MailboxId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";

import type { MailboxRole, MailboxSummary } from "../../mail/api";
import {
  NOT_SPAM_KEYWORDS,
  SPAM_KEYWORDS,
  createMailbox,
} from "../../mail/api";
import {
  findRole,
  useAccountMailboxes,
  useMoveTo,
  useSetFlagged,
  useSetSeen,
} from "../../mail/hooks";
import { mailKeys } from "../../mail/queries";
import type { MailPermissions } from "../../mail/rights";
import { mailPermissions } from "../../mail/rights";
import { resolveView } from "../../mail/views";
import { m } from "../../paraglide/messages.js";

/** Folder names for roles the server may not have created. */
const DEFAULT_NAMES: Partial<Record<MailboxRole, string>> = {
  archive: "Archive",
  junk: "Junk Mail",
};

/**
 * Moves take email ids (every email of the conversations, threads.ts);
 * `conversations` is the number the toast reports (default: the ids).
 */
type MoveAction = (
  ids: readonly EmailId[],
  conversations?: number
) => Promise<void>;

export interface MailActions {
  readonly archive: MoveAction;
  readonly trash: MoveAction;
  /** To Junk with `$junk` (and without `$notjunk`). */
  readonly spam: MoveAction;
  /** Back to the Inbox with `$notjunk` (and without `$junk`). */
  readonly notSpam: MoveAction;
  readonly move: (
    ids: readonly EmailId[],
    mailbox: MailboxSummary,
    label: string
  ) => Promise<void>;
  readonly setSeen: (ids: readonly EmailId[], seen: boolean) => void;
  readonly setFlagged: (ids: readonly EmailId[], flagged: boolean) => void;
  readonly mailboxes: readonly MailboxSummary[];
  /** What the open folder's rights allow (rights.ts). */
  readonly can: MailPermissions;
}

const roleAllowed = (can: MailPermissions, role: MailboxRole): boolean => {
  switch (role) {
    case "archive": {
      return can.archive;
    }
    case "trash": {
      return can.trash;
    }
    case "junk": {
      return can.spam;
    }
    case "inbox": {
      return can.notSpam;
    }
    case "drafts":
    case "sent": {
      return can.move;
    }
    default: {
      return role satisfies never;
    }
  }
};

export const useMailActions = (): MailActions => {
  // The folders of the account in the URL: own, or a shared one.
  const { data: mailboxes = [] } = useAccountMailboxes();
  const { mailbox: segment } = useParams({ strict: false });
  const view = segment === undefined ? null : resolveView(segment, mailboxes);
  const can = mailPermissions(
    mailboxes,
    view?.kind === "mailbox" ? view.mailbox : null
  );
  const queryClient = useQueryClient();
  const toast = useToast();
  const moveTo = useMoveTo();
  const seen = useSetSeen();
  const flagged = useSetFlagged();

  const failed = (): void => {
    toast.show({
      priority: "high",
      title: m.toast_action_failed(),
      tone: "danger",
    });
  };

  const mailboxFor = async (role: MailboxRole): Promise<MailboxId> => {
    const existing = findRole(mailboxes, role);
    if (existing !== undefined) {
      return existing.id;
    }
    const name = DEFAULT_NAMES[role];
    // Never create folders in someone else's (shared) account.
    if (name === undefined || mailboxes.some((box) => box.accountId !== null)) {
      throw new Error(`The account has no ${role} mailbox`);
    }
    const id = await createMailbox(name, role);
    await queryClient.invalidateQueries({ queryKey: mailKeys.mailboxes() });
    return id;
  };

  const moveToRole = async (
    ids: readonly EmailId[],
    role: MailboxRole,
    done: (count: number) => string,
    conversations: number | undefined,
    keywords?: Readonly<Record<string, boolean>>
  ): Promise<void> => {
    if (ids.length === 0 || !roleAllowed(can, role)) {
      return;
    }
    try {
      const mailboxId = await mailboxFor(role);
      await moveTo.mutateAsync({
        ids,
        mailboxId,
        ...(keywords === undefined ? {} : { keywords }),
      });
      toast.show({ title: done(conversations ?? ids.length) });
    } catch {
      failed();
    }
  };

  return {
    archive: async (ids, conversations) => {
      await moveToRole(
        ids,
        "archive",
        (count) => m.toast_archived({ count }),
        conversations
      );
    },
    can,
    mailboxes,
    move: async (ids, mailbox, label) => {
      if (!can.moveTargets.some((target) => target.id === mailbox.id)) {
        return;
      }
      try {
        await moveTo.mutateAsync({ ids, mailboxId: mailbox.id });
        toast.show({ title: m.toast_moved({ folder: label }) });
      } catch {
        failed();
      }
    },
    setFlagged: (ids, value) => {
      if (can.setFlagged) {
        flagged.mutate({ flagged: value, ids }, { onError: failed });
      }
    },
    setSeen: (ids, value) => {
      if (can.setSeen) {
        seen.mutate({ ids, seen: value }, { onError: failed });
      }
    },
    notSpam: async (ids, conversations) => {
      await moveToRole(
        ids,
        "inbox",
        (count) => m.toast_not_spam({ count }),
        conversations,
        NOT_SPAM_KEYWORDS
      );
    },
    spam: async (ids, conversations) => {
      await moveToRole(
        ids,
        "junk",
        (count) => m.toast_spam({ count }),
        conversations,
        SPAM_KEYWORDS
      );
    },
    trash: async (ids, conversations) => {
      await moveToRole(
        ids,
        "trash",
        (count) => m.toast_trashed({ count }),
        conversations
      );
    },
  };
};
