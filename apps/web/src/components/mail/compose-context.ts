// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The compose windows' context, on its own: only React and types here, so
// hot reload never runs this module again. A new context object while the
// app still renders the old provider would break every compose window
// ("useComposer must be used inside ComposeProvider"). ComposeProvider
// (compose-provider.tsx) provides it.

import type { EmailId } from "@nebulamail/jmap-client";
import { createContext, use } from "react";

import type { Draft, SendOutcome } from "../../mail/compose";
import type { ComposeInit } from "../../mail/reply";

export interface ComposeWindowState {
  readonly key: string;
  readonly init: ComposeInit;
  /** The saved draft this window continues (reopened draft, undone send). */
  readonly draftId?: EmailId;
  /** Where its copy on this device goes if the server can't save it. */
  readonly backupId: string;
  /** It was restored from that copy (not yet on the server). */
  readonly fromBackup: boolean;
  readonly minimized: boolean;
  readonly maximized: boolean;
}

export interface Composer {
  readonly windows: readonly ComposeWindowState[];
  /** Opens a new window (or focuses the existing one for the same draft). */
  readonly open: (
    init?: ComposeInit,
    draftId?: EmailId,
    backup?: { readonly backupId: string }
  ) => void;
  /**
   * Opens a saved draft from the Drafts folder. "too-large": it can't be
   * edited here (the user was told); show it in the reader instead.
   */
  readonly openDraft: (id: EmailId) => Promise<"opened" | "too-large">;
  readonly close: (key: string) => void;
  readonly update: (
    key: string,
    patch: Partial<Pick<ComposeWindowState, "minimized" | "maximized">>
  ) => void;
  /**
   * The window sent its message (or may have): close it, then offer undo,
   * or say it wasn't filed or that we don't know if it went out.
   */
  readonly sent: (key: string, draft: Draft, outcome: SendOutcome) => void;
}

export const ComposerContext = createContext<Composer | null>(null);

export const useComposer = (): Composer => {
  const value = use(ComposerContext);
  if (value === null) {
    throw new Error("useComposer must be used inside ComposeProvider");
  }
  return value;
};
