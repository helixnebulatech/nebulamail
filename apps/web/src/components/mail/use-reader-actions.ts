// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the reader can do with the open conversation, shared by its
// toolbar, its keyboard shortcuts and the ⌘K palette. Archive, delete,
// spam, move, read/unread and star act on the whole conversation (the
// emails threads.ts picks for this view); reply, forward, print and the
// raw message act on one email, the newest unless a card says otherwise.
// Actions that take the conversation out of the list move on to the next
// one (Superhuman's "Done").

import type { EmailId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import {
  Archive,
  ChevronsDownUp,
  ChevronsUpDown,
  Download,
  FileCode,
  ListTodo,
  Forward,
  Mail,
  MailCheck,
  OctagonAlert,
  Printer,
  Reply,
  ReplyAll,
  Star,
  Trash2,
} from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";

import { formatFullDate } from "../../lib/format";
import { useShortcut } from "../../lib/shortcuts";
import type { EmailDetail, EmailSummary, MailboxSummary } from "../../mail/api";
import { copyToOwnAccount } from "../../mail/attachments";
import { downloadEml, emlFileName } from "../../mail/download";
import { useOwnAddresses } from "../../mail/hooks";
import type { ReplyMode } from "../../mail/reply";
import { buildReply } from "../../mail/reply";
import type { MailPermissions } from "../../mail/rights";
import type { ThreadAction } from "../../mail/threads";
import { actionTargets } from "../../mail/threads";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { useTaskFromEmail } from "../tasks/use-task-from-email";
import type { Command } from "./commands";
import { useCommands } from "./commands";
import { useComposer } from "./compose-context";
import { rowIndex, useListContent } from "./list-state";
import { isInOverlay } from "./mail-ui";
import { mailboxLabel } from "./mailbox-label";
import { useMailActions } from "./use-mail-actions";

const MARK_READ_DELAY_MS = 600;

/** Runs `action` unless the key press happened in a dialog or composer. */
const outsideOverlays =
  (action: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      action();
    }
  };

/** Print runs, unique in the page (each gets its own frame). */
let printRuns = 0;
const nextPrintRun = (): number => {
  printRuns += 1;
  return printRuns;
};

export interface ReaderActions {
  readonly own: readonly string[];
  readonly next: EmailId | undefined;
  readonly previous: EmailId | undefined;
  /** 1-based position in the list, or null if it isn't in the loaded part. */
  readonly position: number | null;
  readonly mailboxes: readonly MailboxSummary[];
  /** What the open folder allows (rights.ts); the rest is hidden. */
  readonly can: MailPermissions;
  /** Any shown email of the conversation is unread / starred. */
  readonly unread: boolean;
  readonly flagged: boolean;
  readonly archive: () => void;
  readonly trash: () => void;
  readonly spam: () => void;
  readonly notSpam: () => void;
  /** The open list is the Junk folder: offer "Not spam" instead of "Report spam". */
  readonly inJunk: boolean;
  readonly move: (mailbox: MailboxSummary) => void;
  readonly markRead: () => void;
  readonly markUnread: () => void;
  readonly toggleFlag: () => void;
  /**
   * Reply to `email`, or to the newest one. If that one couldn't be
   * loaded, a toast says so and offers to try again.
   */
  readonly reply: (mode: ReplyMode, email?: EmailDetail) => void;
  /** "Create task" from the newest email: Shift+T. */
  readonly createTask: () => void;
  /** The server keeps tasks and the email is loaded. */
  readonly canCreateTask: boolean;
  /** The newest email is still loading: reply and forward wait for it. */
  readonly replyPending: boolean;
  readonly moveMenuOpen: boolean;
  readonly setMoveMenuOpen: (open: boolean) => void;
  /** Saves the raw message as `<subject>.eml`. */
  readonly downloadEml: (email?: EmailDetail) => void;
  /** The email whose "View original" dialog is open. */
  readonly originalId: EmailId | null;
  readonly showOriginal: (id: EmailId | null) => void;
  /** The email being printed; its card mounts a print frame until `printDone`. */
  readonly printingId: EmailId | null;
  /**
   * Which print request this is: a new one on every `print`, even of the
   * email already printing (Firefox's preview still open), so its frame is
   * made again (`key`) and prints again instead of nothing happening.
   */
  readonly printRun: number;
  readonly print: (id?: EmailId) => void;
  /** That run is over; an older run's frame finishing changes nothing. */
  readonly printDone: (run: number) => void;
}

export interface ReaderInput {
  /** The email in the URL. */
  readonly emailId: EmailId;
  /** The shown emails of its conversation, oldest first (threads.ts). */
  readonly emails: readonly EmailSummary[];
  /** The newest email with its body, once loaded (null: not found). */
  readonly latest: EmailDetail | null | undefined;
  /** Loading it failed (offline, server error). */
  readonly latestFailed: boolean;
  /** Loads it again. */
  readonly retryLatest: () => void;
  readonly expandAll: () => void;
  readonly collapseAll: () => void;
}

export const useReaderActions = ({
  collapseAll,
  emailId,
  emails,
  expandAll,
  latest,
  latestFailed,
  retryLatest,
}: ReaderInput): ReaderActions => {
  const list = useListContent();
  const composer = useComposer();
  const actions = useMailActions();
  const own = useOwnAddresses();
  const [marked, setMarked] = useState<ReadonlySet<EmailId>>(new Set());
  const [moveMenuOpen, setMoveMenuOpen] = useState(false);
  const [originalId, setOriginalId] = useState<EmailId | null>(null);
  const [printing, setPrinting] = useState<{
    readonly id: EmailId;
    readonly run: number;
  } | null>(null);
  const toast = useToast();
  const { context } = list.state;

  const targets = (action: ThreadAction): EmailId[] =>
    actionTargets(emails, context, action, emailId);

  // Mark as read after a moment (not while skimming with j/k), each email
  // once: the whole conversation, and replies that arrive while it's open.
  const toMark = emails
    .filter((email) => email.unread && !email.draft && !marked.has(email.id))
    .map((email) => email.id);
  const toMarkKey = toMark.join(" ");
  const { setSeen } = actions;
  const markPending = useEffectEvent(() => {
    if (toMark.length > 0) {
      setMarked((current) => new Set([...current, ...toMark]));
      setSeen(toMark, true);
    }
  });
  useEffect(() => {
    const timer =
      toMarkKey === ""
        ? undefined
        : setTimeout(() => {
            markPending();
          }, MARK_READ_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [toMarkKey]);

  const latestId = latest === undefined || latest === null ? null : latest.id;
  const unread = emails.some((email) => email.unread && !email.draft);
  const flagged = emails.some((email) => email.flagged);
  const next = list.meta.neighbor(emailId, 1);
  const previous = list.meta.neighbor(emailId, -1);
  const index = rowIndex(list.state.emails, emailId);
  const inJunk =
    list.state.view?.kind === "mailbox" &&
    list.state.view.mailbox.role === "junk";

  /** After the conversation leaves this list: the next one, else close. */
  const moveOn = (): void => {
    const target = next ?? previous;
    if (target === undefined) {
      list.actions.close();
    } else {
      list.actions.open(target);
    }
  };

  const replyPending = latest === undefined && !latestFailed;

  const reply = (mode: ReplyMode, email = latest): void => {
    if (email === undefined && !latestFailed) {
      // Still loading: the buttons are disabled meanwhile.
      return;
    }
    if (email === undefined || email === null) {
      toast.show({
        action: { label: m.retry(), onClick: retryLatest },
        closeLabel: m.close(),
        priority: "high",
        // Stays until retried or closed: an error with an action.
        timeout: 0,
        title: m.reply_load_failed(),
        tone: "danger",
      });
      return;
    }
    const locale = getLocale();
    const init = buildReply(email, mode, own, {
      formatDate: (iso) => formatFullDate(iso, locale),
      forwarded: (fields) => m.quote_forwarded(fields),
      forwardedAttached: m.quote_forwarded_attached(),
      quoteTruncated: m.quote_truncated(),
      wrote: (date, sender) => m.quote_wrote({ date, sender }),
    });
    const files = init.attachments ?? [];
    if (!files.some((file) => file.accountId !== undefined)) {
      composer.open(init);
      return;
    }
    // From a shared folder: a draft can only use blobs of our account, so
    // the files and inline images are copied into it first (Blob/copy).
    void (async () => {
      let attachments = files.filter((file) => file.accountId === undefined);
      let failed = files.length - attachments.length;
      try {
        ({ attachments, failed } = await copyToOwnAccount(files));
      } catch {
        // Opened without them; the toast says so.
      }
      if (failed > 0) {
        toast.show({
          priority: "high",
          title: m.attachments_copy_failed(),
          tone: "danger",
        });
      }
      composer.open({ ...init, attachments });
    })();
  };

  const taskFromEmail = useTaskFromEmail();
  const canCreateTask =
    taskFromEmail.available && latest !== undefined && latest !== null;
  const createTask = (): void => {
    if (canCreateTask) {
      void taskFromEmail.create(latest, list.state.mailbox);
    }
  };

  const saveEml = (email = latest): void => {
    if (email === undefined || email === null) {
      return;
    }
    downloadEml(email).catch(() => {
      toast.show({
        priority: "high",
        title: m.toast_download_failed({ name: emlFileName(email.subject) }),
        tone: "danger",
      });
    });
  };

  const markRead = (): void => {
    const ids = targets("read");
    setMarked((current) => new Set([...current, ...ids]));
    setSeen(ids, true);
  };

  const { can } = actions;
  const result: ReaderActions = {
    archive: () => {
      if (!can.archive) {
        return;
      }
      moveOn();
      void actions.archive(targets("move"), 1);
    },
    can,
    downloadEml: saveEml,
    flagged,
    inJunk,
    mailboxes: actions.mailboxes,
    markRead,
    markUnread: () => {
      if (!can.setSeen) {
        return;
      }
      // Everything shown counts as handled: don't mark it read again.
      setMarked(new Set(emails.map((email) => email.id)));
      setSeen(targets("unread"), false);
      list.actions.close();
    },
    canCreateTask,
    createTask,
    move: (mailbox) => {
      if (!can.moveTargets.some((target) => target.id === mailbox.id)) {
        return;
      }
      moveOn();
      void actions.move(targets("move"), mailbox, mailboxLabel(mailbox));
    },
    moveMenuOpen,
    next,
    notSpam: () => {
      if (!can.notSpam) {
        return;
      }
      moveOn();
      void actions.notSpam(targets("move"), 1);
    },
    originalId,
    own,
    position: index === -1 ? null : index + 1,
    previous,
    print: (id) => {
      const target = id ?? latestId;
      if (target === null) {
        return;
      }
      setPrinting({ id: target, run: nextPrintRun() });
    },
    printDone: (run) => {
      setPrinting((current) => (current?.run === run ? null : current));
    },
    printRun: printing?.run ?? 0,
    printingId: printing?.id ?? null,
    reply,
    replyPending,
    setMoveMenuOpen,
    showOriginal: setOriginalId,
    spam: () => {
      if (!can.spam) {
        return;
      }
      moveOn();
      void actions.spam(targets("move"), 1);
    },
    toggleFlag: () => {
      if (emails.length > 0) {
        actions.setFlagged(targets(flagged ? "unflag" : "flag"), !flagged);
      }
    },
    trash: () => {
      if (!can.trash) {
        return;
      }
      moveOn();
      void actions.trash(targets("move"), 1);
    },
    unread,
  };

  // With a selection in the list, its shortcuts act on the selection.
  const enabled = { enabled: !list.state.hasSelection };
  useShortcut("e", outsideOverlays(result.archive), enabled);
  useShortcut("#", outsideOverlays(result.trash), enabled);
  useShortcut(
    "!",
    outsideOverlays(inJunk ? result.notSpam : result.spam),
    enabled
  );
  useShortcut("shift+u", outsideOverlays(result.markUnread), enabled);
  useShortcut("shift+i", outsideOverlays(result.markRead), enabled);
  useShortcut("s", outsideOverlays(result.toggleFlag), enabled);
  useShortcut(
    "v",
    outsideOverlays(() => {
      if (can.move) {
        setMoveMenuOpen(true);
      }
    }),
    enabled
  );
  useShortcut(
    "r",
    outsideOverlays(() => {
      reply("reply");
    })
  );
  useShortcut(
    "a",
    outsideOverlays(() => {
      reply("replyAll");
    })
  );
  useShortcut(
    "f",
    outsideOverlays(() => {
      reply("forward");
    })
  );
  useShortcut("escape", outsideOverlays(list.actions.close));
  // "t" alone would be close to "g t" (Sent): Shift+T, like Shift+U/I.
  useShortcut("shift+t", outsideOverlays(createTask), enabled);
  // The browser's Ctrl/⌘+P would print the whole app: print the open
  // email like «Print» does (header and body only). Until it has loaded,
  // the browser prints (styles/app.css keeps just the conversation).
  useShortcut(
    "mod+p",
    () => {
      result.print();
    },
    { enabled: latestId !== null }
  );

  const several: Command[] =
    emails.length > 1
      ? [
          {
            group: "actions",
            icon: ChevronsUpDown,
            id: "thread-expand-all",
            label: m.thread_expand_all(),
            run: expandAll,
          },
          {
            group: "actions",
            icon: ChevronsDownUp,
            id: "thread-collapse-all",
            label: m.thread_collapse_all(),
            run: collapseAll,
          },
        ]
      : [];

  // What the folder's rights don't allow isn't offered (rights.ts).
  const allowed: Partial<Record<string, boolean>> = {
    "email-archive": can.archive,
    "email-flag": can.setFlagged,
    "email-not-spam": can.notSpam,
    "email-spam": can.spam,
    "email-trash": can.trash,
    "email-unread": can.setSeen,
  };

  useCommands(
    latest === undefined || latest === null
      ? []
      : (
          [
            {
              group: "actions",
              icon: Archive,
              id: "email-archive",
              label: m.action_archive(),
              run: result.archive,
              shortcut: "e",
            },
            {
              group: "actions",
              icon: Trash2,
              id: "email-trash",
              label: m.action_trash(),
              run: result.trash,
              shortcut: "#",
            },
            {
              group: "actions",
              icon: Mail,
              id: "email-unread",
              label: m.action_mark_unread(),
              run: result.markUnread,
              shortcut: "shift+u",
            },
            {
              group: "actions",
              icon: Star,
              id: "email-flag",
              label: flagged ? m.action_unflag() : m.action_flag(),
              run: result.toggleFlag,
              shortcut: "s",
            },
            inJunk
              ? {
                  group: "actions",
                  icon: MailCheck,
                  id: "email-not-spam",
                  label: m.action_not_spam(),
                  run: result.notSpam,
                  shortcut: "!",
                }
              : {
                  group: "actions",
                  icon: OctagonAlert,
                  id: "email-spam",
                  label: m.action_spam(),
                  run: result.spam,
                  shortcut: "!",
                },
            {
              group: "actions",
              icon: Printer,
              id: "email-print",
              label: m.action_print(),
              run: () => {
                result.print();
              },
            },
            {
              group: "actions",
              icon: Download,
              id: "email-download-eml",
              label: m.action_download_eml(),
              run: () => {
                saveEml();
              },
            },
            ...(canCreateTask
              ? [
                  {
                    group: "actions" as const,
                    icon: ListTodo,
                    id: "email-create-task",
                    keywords: "task tarea to-do",
                    label: m.tasks_create_from_email(),
                    run: createTask,
                    shortcut: "shift+t",
                  },
                ]
              : []),
            {
              group: "actions",
              icon: FileCode,
              id: "email-view-original",
              label: m.action_view_original(),
              run: () => {
                setOriginalId(latest.id);
              },
            },
            {
              group: "actions",
              icon: Reply,
              id: "email-reply",
              label: m.reply(),
              run: () => {
                reply("reply");
              },
              shortcut: "r",
            },
            {
              group: "actions",
              icon: ReplyAll,
              id: "email-reply-all",
              label: m.reply_all(),
              run: () => {
                reply("replyAll");
              },
              shortcut: "a",
            },
            {
              group: "actions",
              icon: Forward,
              id: "email-forward",
              label: m.forward(),
              run: () => {
                reply("forward");
              },
              shortcut: "f",
            },
            ...several,
          ] satisfies Command[]
        ).filter((command) => allowed[command.id] !== false)
  );

  return result;
};
