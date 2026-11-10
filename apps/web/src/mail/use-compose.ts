// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// State of one compose window: edits, autosave to Drafts
// every few seconds (never two saves at once; a copy in IndexedDB while the
// server can't save), send with undo.

import type { BlobId, EmailId } from "@nebulamail/jmap-client";
import { useEffect, useEffectEvent, useRef, useState } from "react";

import type { DraftAttachment } from "./attachments";
import type {
  Draft,
  DraftProblem,
  Identity,
  SendOutcome,
  SentEmail,
} from "./compose";
import {
  DEFAULT_UNDO_SECONDS,
  allRecipients,
  checkDraft,
  discardDraft,
  saveDraft,
  sendEmail,
  undoSend,
} from "./compose";
import { deleteBackup, saveBackup } from "./draft-backup";
import { sameAddress, swapIdentityBcc } from "./recipients";
import type { ServerRefusal } from "./refusal";
import { serverRefusalOf } from "./refusal";
import { SendFailedError } from "./send-failed";
import { rememberSent } from "./suggestions";

export const AUTOSAVE_MS = 3000;

/** How long after the last keystroke the draft's `html` catches up. */
const BODY_COMMIT_MS = 300;

export type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

export interface ComposeState {
  /**
   * The draft as of the last render. Its `html` catches up with the editor
   * shortly after typing stops (`editBody`): handlers that need the text as
   * it is now (send, minimize, a switch to plain text) use `readDraft`.
   */
  readonly draft: Draft;
  /** The draft with the editor's latest text, at once. */
  readonly readDraft: () => Draft;
  /**
   * The HTML editor changed: `read` returns that version's HTML. It is
   * serialized only when needed (a save, `readDraft`, or once typing
   * pauses), not on every keystroke: with a long quote that was most of the
   * cost of a key press.
   */
  readonly editBody: (read: () => string) => void;
  readonly update: (patch: Partial<Omit<Draft, "identity">>) => void;
  /**
   * Switches the body between HTML (`plainText` null) and plain text: the
   * caller converts it (body.ts) and gives both forms.
   */
  readonly setBody: (body: {
    readonly html: string;
    readonly plainText: string | null;
  }) => void;
  readonly setIdentity: (identity: Identity) => void;
  readonly addAttachment: (attachment: DraftAttachment) => void;
  readonly removeAttachment: (blobId: BlobId) => void;
  readonly saveState: SaveState;
  /** When the last save finished (for "Guardado a las 9:41"). */
  readonly savedAt: Date | null;
  /** The server couldn't save it, so a copy is kept on this device. */
  readonly backedUp: boolean;
  /** Why the last save failed, when retrying alone won't help. */
  readonly saveRefusal: ServerRefusal | null;
  readonly problem: DraftProblem | null;
  /**
   * Sends it. Resolves with the outcome (sent, sent but not filed, or
   * unknown: never to be sent again); throws only if nothing went out.
   * `sendAt`: schedule it for then instead (schedule.ts).
   */
  readonly send: (options?: { readonly sendAt?: Date }) => Promise<SendOutcome>;
  readonly sending: boolean;
  /** Saves now if there are unsaved edits (before closing the window). */
  readonly flush: () => Promise<void>;
  /** Deletes the saved draft, if any (stops autosave first). */
  readonly discard: () => Promise<void>;
}

export const emptyDraft = (identity: Identity | null): Draft => ({
  bcc: [],
  cc: [],
  identity,
  subject: "",
  attachments: [],
  html: "<p></p>",
  to: [],
});

export interface ComposeOptions {
  /** The saved draft this continues (reopened draft, undone send). */
  readonly draftId?: EmailId;
  /**
   * Where the device copy of this window lives (draft-backup.ts). Without
   * it nothing is kept on the device.
   */
  readonly backupId?: string;
  /** The draft comes from a device copy: save it to the server soon. */
  readonly fromBackup?: boolean;
  /**
   * The user's identities as known now (the server's, or the copy on this
   * device). When the draft's sender is unknown (null) or isn't one of
   * them, it becomes the one for `identityEmail`, else the first.
   */
  readonly identities?: readonly Identity[];
  readonly identityEmail?: string | undefined;
  /**
   * A reopened email (a draft, a device copy, an undone send): its Bcc is
   * as it was, the sender's is only swapped if the sender changes.
   */
  readonly restored?: boolean;
}

/** The draft's sender among `identities` (see ComposeOptions). */
export const resolveIdentity = (
  identity: Identity | null,
  identities: readonly Identity[],
  preferredEmail?: string
): Identity | null => {
  if (identities.length === 0) {
    return identity;
  }
  const byEmail = (email: string | undefined): Identity | undefined =>
    email === undefined
      ? undefined
      : identities.find((candidate) => sameAddress(candidate.email, email));
  return (
    identities.find((candidate) => candidate.id === identity?.id) ??
    byEmail(identity?.email) ??
    byEmail(preferredEmail) ??
    identities[0] ??
    null
  );
};

/**
 * `initial` may be an existing draft (reopened) with its id, or a reply.
 * After `send`, the window should close; `undo` reopens it (see useUndoSend).
 * If the server can't save, a copy is kept on the device until it can.
 */
export const useCompose = (
  initial: Draft,
  {
    backupId,
    draftId: initialDraftId,
    fromBackup = false,
    identities = [],
    identityEmail,
    restored = false,
  }: ComposeOptions = {}
): ComposeState => {
  const [edited, setEdited] = useState(initial);
  const [saveState, setSaveState] = useState<SaveState>(
    fromBackup ? "dirty" : "idle"
  );
  // The sender follows the identities as they arrive (computed, not
  // stored): from the device copy to the server's, or from unknown.
  const sender = resolveIdentity(edited.identity, identities, identityEmail);
  const draft: Draft =
    sender === edited.identity ? edited : { ...edited, identity: sender };
  // Once the sender is known, a draft that could only stay on the device
  // tries the server again (state adjusted during render, not in an effect).
  const senderId = sender?.id ?? null;
  // The sender whose Bcc (RFC 8621 §6 Identity `bcc`) the draft has: a new
  // email gets it once its sender is known; a reopened one (undefined
  // until its sender is known) already has it. Then each change of sender
  // swaps it (recipients.ts `swapIdentityBcc`). Adjusted during render.
  const reopened = restored || fromBackup || initialDraftId !== undefined;
  const [bccSender, setBccSender] = useState<Identity | null | undefined>(() =>
    reopened ? (sender ?? undefined) : null
  );
  if (sender !== null && bccSender?.id !== sender.id) {
    setBccSender(sender);
    if (bccSender !== undefined) {
      setEdited((current) => ({
        ...current,
        bcc: swapIdentityBcc(current.bcc, bccSender, sender),
      }));
    }
  }
  const [knownSender, setKnownSender] = useState(senderId);
  if (knownSender !== senderId) {
    setKnownSender(senderId);
    if (senderId !== null && saveState === "error") {
      setSaveState("dirty");
    }
  }
  const [savedAt, setSavedAt] = useState<Date | null>(null);
  const [sending, setSending] = useState(false);
  const [backedUp, setBackedUp] = useState(fromBackup);
  const [saveRefusal, setSaveRefusal] = useState<ServerRefusal | null>(null);
  // Mutable bookkeeping that must not re-render: the server id of the
  // current draft version and whether a save is in flight.
  const draftId = useRef<EmailId | undefined>(initialDraftId);
  const saving = useRef<Promise<void> | null>(null);
  const version = useRef(0);
  // Whether a device copy may exist (so it is deleted once not needed).
  const hasBackup = useRef(fromBackup);
  // The body as the editor has it now: its HTML (`latestHtml`), or a
  // version not serialized yet (`pendingBody`, see `editBody`). The state
  // (`edited.html`) follows after BODY_COMMIT_MS.
  const latestHtml = useRef(initial.html);
  const pendingBody = useRef<(() => string) | null>(null);
  const bodyTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  );

  /** Serializes a pending body version into the draft (state and ref). */
  const commitBody = (): void => {
    const read = pendingBody.current;
    if (read === null) {
      return;
    }
    pendingBody.current = null;
    clearTimeout(bodyTimer.current);
    const html = read();
    latestHtml.current = html;
    setEdited((current) => ({ ...current, html }));
  };

  const readDraft = (): Draft => {
    commitBody();
    return { ...draft, html: latestHtml.current };
  };

  // A closed window doesn't commit later (its draft was flushed on close).
  useEffect(
    () => () => {
      clearTimeout(bodyTimer.current);
    },
    []
  );

  const keepOnDevice = async (current: Draft): Promise<void> => {
    if (backupId === undefined) {
      return;
    }
    hasBackup.current = true;
    await saveBackup({
      draft: current,
      draftId: draftId.current,
      id: backupId,
    });
    setBackedUp(true);
  };

  const dropDeviceCopy = async (): Promise<void> => {
    if (backupId === undefined || !hasBackup.current) {
      return;
    }
    hasBackup.current = false;
    setBackedUp(false);
    await deleteBackup(backupId);
  };

  // Back online: try the server again.
  useEffect(() => {
    const retry = (): void => {
      setSaveState((current) => (current === "error" ? "dirty" : current));
    };
    globalThis.addEventListener("online", retry);
    return () => {
      globalThis.removeEventListener("online", retry);
    };
  }, []);

  const saveNow = async (): Promise<void> => {
    if (saving.current !== null) {
      await saving.current;
    }
    const savedVersion = version.current;
    const current = readDraft();
    setSaveState("saving");
    const run = async (): Promise<void> => {
      try {
        draftId.current = await saveDraft(current, draftId.current);
        setSaveRefusal(null);
        setSavedAt(new Date());
        setSaveState(version.current === savedVersion ? "saved" : "dirty");
        await dropDeviceCopy().catch(() => null);
      } catch (error) {
        setSaveRefusal(serverRefusalOf(error));
        setSaveState("error");
        // Offline or the server said no: don't lose the text.
        await keepOnDevice(current).catch(() => null);
      }
    };
    saving.current = run();
    await saving.current;
    saving.current = null;
  };
  const save = useEffectEvent(saveNow);

  // Throttled, not debounced: while the user keeps typing it still saves
  // every AUTOSAVE_MS (a save leaves the state "dirty" if edits came in).
  useEffect(() => {
    const timer =
      saveState === "dirty"
        ? setTimeout(() => {
            void save();
          }, AUTOSAVE_MS)
        : undefined;
    return () => {
      clearTimeout(timer);
    };
  }, [saveState]);

  const update = (patch: Partial<Omit<Draft, "identity">>): void => {
    if (patch.html !== undefined) {
      pendingBody.current = null;
      latestHtml.current = patch.html;
    }
    version.current += 1;
    setEdited((current) => ({ ...current, ...patch }));
    setSaveState("dirty");
  };

  const editBody = (read: () => string): void => {
    pendingBody.current = read;
    version.current += 1;
    // Already "dirty" while typing: no render per keystroke.
    setSaveState("dirty");
    clearTimeout(bodyTimer.current);
    bodyTimer.current = setTimeout(commitBody, BODY_COMMIT_MS);
  };

  const setBody: ComposeState["setBody"] = ({ html, plainText }) => {
    pendingBody.current = null;
    clearTimeout(bodyTimer.current);
    latestHtml.current = html;
    version.current += 1;
    setEdited(({ plainText: _old, ...current }) => ({
      ...current,
      html,
      ...(plainText === null ? {} : { plainText }),
    }));
    setSaveState("dirty");
  };

  const setIdentity = (identity: Identity): void => {
    version.current += 1;
    setEdited((current) => ({ ...current, identity }));
    setSaveState("dirty");
  };

  // Uploads finish at any time: these work on the latest draft.
  const addAttachment = (attachment: DraftAttachment): void => {
    version.current += 1;
    setEdited((current) => ({
      ...current,
      attachments: [...current.attachments, attachment],
    }));
    setSaveState("dirty");
  };

  const removeAttachment = (blobId: BlobId): void => {
    version.current += 1;
    setEdited((current) => ({
      ...current,
      attachments: current.attachments.filter(
        (attachment) => attachment.blobId !== blobId
      ),
    }));
    setSaveState("dirty");
  };

  const send = async ({
    sendAt,
  }: { readonly sendAt?: Date } = {}): Promise<SendOutcome> => {
    // The text as it is when Send is pressed.
    const current = readDraft();
    setSending(true);
    // Let a running autosave finish so we replace the latest draft version.
    if (saving.current !== null) {
      await saving.current;
    }
    const outcome = await sendEmail(current, {
      undoSeconds: DEFAULT_UNDO_SECONDS,
      ...(sendAt === undefined ? {} : { sendAt }),
      ...(draftId.current === undefined
        ? {}
        : { previousDraft: draftId.current }),
    }).catch((error: unknown) => {
      setSending(false);
      // The attempt saved a new draft and replaced the old one: keep
      // editing that one, or the next save leaves an orphan copy.
      if (error instanceof SendFailedError && error.draftId !== null) {
        draftId.current = error.draftId;
      }
      throw error;
    });
    setSending(false);
    const serverDraft =
      outcome.kind === "unknown" ? outcome.emailId : outcome.sent.emailId;
    // Suggestions learn from what was sent (on this device only).
    rememberSent(allRecipients(current)).catch(() => null);
    if (serverDraft === null) {
      // Can't tell whether the server has it: the device copy stays.
      return outcome;
    }
    draftId.current = serverDraft;
    dropDeviceCopy().catch(() => null);
    return outcome;
  };

  const flush = async (): Promise<void> => {
    if (saving.current !== null) {
      await saving.current;
    }
    if (saveState === "dirty" || saveState === "error") {
      await saveNow();
    }
  };

  const discard = async (): Promise<void> => {
    // Edits after this must not resurrect the draft.
    pendingBody.current = null;
    clearTimeout(bodyTimer.current);
    setSaveState("idle");
    if (saving.current !== null) {
      await saving.current;
    }
    const id = draftId.current;
    draftId.current = undefined;
    // The device copy goes first: discarding works offline too.
    await dropDeviceCopy().catch(() => null);
    if (id !== undefined) {
      await discardDraft(id);
    }
  };

  return {
    addAttachment,
    backedUp,
    discard,
    draft,
    editBody,
    flush,
    problem: checkDraft(draft),
    readDraft,
    removeAttachment,
    saveRefusal,
    saveState,
    savedAt,
    send,
    sending,
    setBody,
    setIdentity,
    update,
  };
};

/** Undo for a sent message, while the server still holds it. */
export const undoSent = async (sent: SentEmail): Promise<void> => {
  await undoSend(sent);
};
