// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The list's actions on whole conversations (rows): each row stands for
// the emails of its thread that the action changes in this view
// (threads.ts `rowTargets`), so archiving a row archives the conversation.

import type { EmailId } from "@nebulamail/jmap-client";

import type { ConversationRow, MailboxSummary } from "../../mail/api";
import type { MailPermissions } from "../../mail/rights";
import type { ConversationView } from "../../mail/threads";
import { conversationView, rowTargets } from "../../mail/threads";
import { useListContent } from "./list-state";
import { useMailActions } from "./use-mail-actions";

type RowIds = readonly EmailId[];

export interface ConversationActions {
  readonly archive: (rows: RowIds) => void;
  readonly trash: (rows: RowIds) => void;
  readonly spam: (rows: RowIds) => void;
  readonly notSpam: (rows: RowIds) => void;
  readonly move: (rows: RowIds, mailbox: MailboxSummary, label: string) => void;
  readonly setSeen: (rows: RowIds, seen: boolean) => void;
  readonly setFlagged: (rows: RowIds, flagged: boolean) => void;
  readonly mailboxes: readonly MailboxSummary[];
  /** What the open folder allows (rights.ts). */
  readonly can: MailPermissions;
  /** What a row shows about its conversation in this view. */
  readonly view: (row: ConversationRow) => ConversationView;
}

export const useConversationActions = (): ConversationActions => {
  const list = useListContent();
  const actions = useMailActions();
  const { context } = list.state;

  const rows = (ids: RowIds): ConversationRow[] =>
    list.state.emails.filter((row) => ids.includes(row.id));

  return {
    archive: (ids) => {
      void actions.archive(rowTargets(rows(ids), context, "move"), ids.length);
    },
    can: actions.can,
    mailboxes: actions.mailboxes,
    move: (ids, mailbox, label) => {
      void actions.move(rowTargets(rows(ids), context, "move"), mailbox, label);
    },
    notSpam: (ids) => {
      void actions.notSpam(rowTargets(rows(ids), context, "move"), ids.length);
    },
    setFlagged: (ids, flagged) => {
      actions.setFlagged(
        rowTargets(rows(ids), context, flagged ? "flag" : "unflag"),
        flagged
      );
    },
    setSeen: (ids, seen) => {
      actions.setSeen(
        rowTargets(rows(ids), context, seen ? "read" : "unread"),
        seen
      );
    },
    spam: (ids) => {
      void actions.spam(rowTargets(rows(ids), context, "move"), ids.length);
    },
    trash: (ids) => {
      void actions.trash(rowTargets(rows(ids), context, "move"), ids.length);
    },
    view: (row) => conversationView(row, context),
  };
};
