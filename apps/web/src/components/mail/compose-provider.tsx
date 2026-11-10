// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Floating compose windows (Gmail/Proton): up to three at once, each one a
// draft with autosave. Send closes the window and offers "Undo" while the
// server holds the message (FUTURERELEASE); undo brings the window back.

import type { EmailId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useEffectEvent, useState } from "react";
import type { ReactNode } from "react";

import type { Draft, SendOutcome, SentEmail } from "../../mail/compose";
import { listBackups, newBackupId } from "../../mail/draft-backup";
import type { DraftBackup } from "../../mail/draft-backup";
import { emailQuery, mailKeys } from "../../mail/queries";
import type { ComposeInit } from "../../mail/reply";
import { reopenDraft } from "../../mail/reply";
import { formatSendAt } from "../../mail/schedule";
import { undoSent } from "../../mail/use-compose";
import { scheduleKeys } from "../../mail/use-schedule";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { ComposerContext } from "./compose-context";
import type { Composer, ComposeWindowState } from "./compose-context";

export const MAX_WINDOWS = 3;
/**
 * How long "Undo" stays after scheduling: the message waits on the server
 * for hours, and the Scheduled view can cancel it later anyway.
 */
const SCHEDULED_UNDO_MS = 10_000;

export const draftToInit = (draft: Draft): ComposeInit => ({
  bcc: draft.bcc,
  cc: draft.cc,
  subject: draft.subject,
  ...(draft.identity === null ? {} : { identityEmail: draft.identity.email }),
  attachments: draft.attachments,
  ...(draft.plainText === undefined
    ? { html: draft.html }
    : { plainText: true, text: draft.plainText }),
  restored: true,
  to: draft.to,
  ...(draft.inReplyTo === undefined ? {} : { inReplyTo: draft.inReplyTo }),
  ...(draft.references === undefined ? {} : { references: draft.references }),
  ...(draft.readReceipt === true ? { readReceipt: true } : {}),
});

let counter = 0;
const nextKey = (): string => {
  counter += 1;
  return `compose-${counter}`;
};

const backupWindow = (backup: DraftBackup): ComposeWindowState => ({
  backupId: backup.id,
  fromBackup: true,
  init: draftToInit(backup.draft),
  key: nextKey(),
  maximized: false,
  minimized: false,
  ...(backup.draftId === undefined ? {} : { draftId: backup.draftId }),
});

export const ComposeProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const [windows, setWindows] = useState<readonly ComposeWindowState[]>([]);
  const toast = useToast();
  const queryClient = useQueryClient();

  /** Opens the device copies (as many as fit); the rest wait for later. */
  const restore = (backups: readonly DraftBackup[]): void => {
    setWindows((current) => {
      const fresh = backups
        .filter(
          (backup) => !current.some((window) => window.backupId === backup.id)
        )
        .slice(0, Math.max(0, MAX_WINDOWS - current.length))
        .map((backup) => backupWindow(backup));
      return fresh.length === 0
        ? current
        : [
            ...current.map((window) => ({ ...window, minimized: true })),
            ...fresh,
          ];
    });
  };

  // Messages the server couldn't save last time (offline): offer them.
  const offer = useEffectEvent((backups: readonly DraftBackup[]) => {
    toast.show({
      action: {
        label: m.compose_backups_open(),
        onClick: () => {
          restore(backups);
        },
      },
      priority: "high",
      closeLabel: m.close(),
      // Stays until opened or closed: it's text the user would lose.
      timeout: 0,
      title: m.compose_backups_found(),
      tone: "info",
    });
  });

  useEffect(() => {
    let active = true;
    const load = async (): Promise<void> => {
      const backups = await listBackups().catch(() => []);
      if (active && backups.length > 0) {
        offer(backups);
      }
    };
    void load();
    return () => {
      active = false;
    };
  }, []);

  const open = (
    init: ComposeInit = {},
    draftId?: EmailId,
    backup?: { readonly backupId: string }
  ): void => {
    const existing =
      draftId === undefined
        ? undefined
        : windows.find((window) => window.draftId === draftId);
    if (existing !== undefined) {
      setWindows((current) =>
        current.map((window) =>
          window.key === existing.key ? { ...window, minimized: false } : window
        )
      );
      return;
    }
    if (windows.length >= MAX_WINDOWS) {
      toast.show({ title: m.toast_too_many_windows(), tone: "info" });
      return;
    }
    setWindows((current) => [
      // A new window minimizes the others, like Gmail.
      ...current.map((window) => ({ ...window, minimized: true })),
      {
        backupId: backup?.backupId ?? newBackupId(),
        fromBackup: backup !== undefined,
        init,
        key: nextKey(),
        maximized: false,
        minimized: false,
        ...(draftId === undefined ? {} : { draftId }),
      },
    ]);
  };

  const openDraft = async (id: EmailId): Promise<"opened" | "too-large"> => {
    // A device copy is newer than the server's version.
    const backups = await listBackups().catch(() => []);
    const copy = backups.find((backup) => backup.draftId === id);
    if (copy !== undefined) {
      open(draftToInit(copy.draft), id, { backupId: copy.id });
      return "opened";
    }
    const email = await queryClient.query(emailQuery(id));
    if (email === null) {
      return "opened";
    }
    const reopened = reopenDraft(email);
    if (!reopened.ok) {
      // Never in the editor: saving would replace it with a cut copy.
      toast.show({ title: m.draft_too_large(), tone: "info" });
      return "too-large";
    }
    open(reopened.init, id);
    return "opened";
  };

  const close = (key: string): void => {
    setWindows((current) => current.filter((window) => window.key !== key));
    // The draft may have changed: refresh Drafts.
    void queryClient.invalidateQueries({ queryKey: mailKeys.lists() });
  };

  const update: Composer["update"] = (key, patch) => {
    setWindows((current) =>
      current.map((window) =>
        window.key === key ? { ...window, ...patch } : window
      )
    );
  };

  /** Undo while the server holds it; `title` says what happened. */
  const offerUndo = (draft: Draft, message: SentEmail, title: string) => {
    const undo = async (): Promise<void> => {
      try {
        await undoSent(message);
        open(draftToInit(draft), message.emailId);
        toast.show({ title: m.toast_send_undone(), tone: "info" });
      } catch {
        toast.show({
          priority: "high",
          title: m.toast_undo_too_late(),
          tone: "danger",
        });
      }
      await queryClient.invalidateQueries({ queryKey: mailKeys.all });
    };
    const remaining = Date.parse(message.sendAt) - Date.now();
    if (!message.undoable || Number.isNaN(remaining) || remaining < 1000) {
      return false;
    }
    const scheduled = message.scheduled === true;
    toast.show({
      action: {
        label: m.toast_undo(),
        onClick: () => {
          void undo();
        },
      },
      countdown: true,
      // A little shorter than the hold, so "Undo" never arrives too late.
      timeout: scheduled
        ? Math.min(SCHEDULED_UNDO_MS, remaining - 500)
        : Math.max(1000, remaining - 500),
      title,
    });
    return true;
  };

  const sent: Composer["sent"] = (key, draft, outcome: SendOutcome) => {
    // Always closed: a "Send" button left at hand invites a duplicate.
    close(key);
    void queryClient.invalidateQueries({ queryKey: scheduleKeys.list() });
    switch (outcome.kind) {
      case "sent": {
        const title =
          outcome.sent.scheduled === true
            ? m.toast_scheduled({
                date: formatSendAt(outcome.sent.sendAt, getLocale()),
              })
            : m.toast_sent();
        if (!offerUndo(draft, outcome.sent, title)) {
          toast.show({ title, tone: "success" });
        }
        break;
      }
      case "sentNotFiled": {
        if (outcome.sent.scheduled === true) {
          // Still an editable draft for days: say so and keep it on screen.
          toast.show({
            closeLabel: m.close(),
            priority: "high",
            timeout: 0,
            title: m.toast_scheduled_not_filed({
              date: formatSendAt(outcome.sent.sendAt, getLocale()),
            }),
            tone: "info",
          });
          break;
        }
        if (!offerUndo(draft, outcome.sent, m.toast_sent_not_filed())) {
          toast.show({
            closeLabel: m.close(),
            priority: "high",
            timeout: 0,
            title: m.toast_sent_not_filed(),
            tone: "info",
          });
        }
        break;
      }
      case "unknown": {
        toast.show({
          closeLabel: m.close(),
          priority: "high",
          timeout: 0,
          title: m.toast_send_unknown(),
          tone: "danger",
        });
        void queryClient.invalidateQueries({ queryKey: mailKeys.all });
        break;
      }
      default: {
        outcome satisfies never;
      }
    }
  };

  return (
    <ComposerContext
      // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object and its functions.
      value={{ close, open, openDraft, sent, update, windows }}
    >
      {children}
    </ComposerContext>
  );
};
