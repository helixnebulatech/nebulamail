// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One compose window's form state, lifted into a provider (composition
// rules "state-lift-state" and "state-context-interface"):
// ComposerFormProvider alone knows the draft comes from useCompose, the
// uploads, the body's images and the scheduled send; the parts in
// composer.tsx only read { state, actions, meta } through useComposerForm.

import { normalizeCid, normalizeContentId } from "@nebulamail/mail-render";
import { useToast } from "@nebulamail/ui";
import type { InsertedImage } from "@nebulamail/ui";
import { useQueryClient } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import {
  createContext,
  use,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
} from "react";
import type { DragEvent, KeyboardEvent, ReactNode, RefObject } from "react";

import { formatBytes } from "../../lib/format";
import { isApplePlatform } from "../../lib/shortcuts";
import type { AttachmentLimits, RejectReason } from "../../mail/attachments";
import {
  htmlToPlainText,
  plainSignature,
  plainTextToHtml,
  swapTextSignature,
} from "../../mail/body";
import type { Draft, DraftProblem, Identity } from "../../mail/compose";
import { useInlineImages } from "../../mail/hooks";
import { canInsertImage, toDataUrl } from "../../mail/images";
import type { ServerRefusal } from "../../mail/refusal";
import { DraftRefusedError, serverRefusalOf } from "../../mail/refusal";
import {
  formatSendAt,
  latestAllowed,
  maximumInRefusal,
  scheduleFailure,
} from "../../mail/schedule";
import type { ScheduleLimit } from "../../mail/schedule";
import { SendFailedError } from "../../mail/send-failed";
import { useCompose } from "../../mail/use-compose";
import { useReceiptsSupported } from "../../mail/use-read-receipts";
import { scheduleKeys, useScheduleLimit } from "../../mail/use-schedule";
import { useSendAfterUploads, useUploads } from "../../mail/use-uploads";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import type { ComposeWindowState } from "./compose-context";
import { useComposer } from "./compose-context";
import { useKeys } from "./use-apple";

/** What blocks sending: the draft's problems, or uploads still running. */
export type Problem = DraftProblem | "uploading";

/** "Mon 9 Nov, 20:01": the latest time that can be chosen now. */
const latestText = (limit: ScheduleLimit): string =>
  formatSendAt(latestAllowed(new Date(), limit), getLocale());

/**
 * Why a scheduled send failed, in words; null for other failures. A
 * refusal may teach the server's real limit (schedule.ts): the cached
 * limit is refreshed so the menu stops offering those times.
 */
const scheduleErrorText = async (
  error: unknown,
  limit: ScheduleLimit | null,
  queryClient: QueryClient
): Promise<string | null> => {
  const failure = scheduleFailure(error);
  if (failure?.kind === "time") {
    if (failure.problem === "tooFar" && limit !== null) {
      return m.schedule_too_far({ date: latestText(limit) });
    }
    return failure.problem === "past"
      ? m.schedule_past()
      : m.schedule_invalid();
  }
  const cause = error instanceof SendFailedError ? error.cause : null;
  // Only a refusal that names the server's maximum is about the time; any
  // other forbiddenMailFrom (identity, permissions) gets the usual text.
  if (
    !(cause instanceof DraftRefusedError) ||
    cause.type !== "forbiddenMailFrom" ||
    maximumInRefusal(cause.message) === null
  ) {
    return null;
  }
  await queryClient.invalidateQueries({ queryKey: scheduleKeys.limit() });
  const fresh = queryClient.getQueryData<ScheduleLimit | null>(
    scheduleKeys.limit()
  );
  // The refusal taught the real maximum (schedule.ts learnMaximum).
  return fresh === null || fresh === undefined
    ? m.toast_schedule_refused()
    : m.toast_schedule_refused_max({ date: latestText(fresh) });
};

/** Why the server refused to save or send (retrying alone won't help). */
const REFUSAL_TEXT: Record<ServerRefusal, () => string> = {
  tooLarge: m.compose_refused_too_large,
  tooManyKeywords: m.compose_refused_too_many_keywords,
  tooManyMailboxes: m.compose_refused_too_many_mailboxes,
};

export const refusalText = (refusal: ServerRefusal): string =>
  REFUSAL_TEXT[refusal]();

const rejectText = (
  { name, reason }: { readonly name: string; readonly reason: RejectReason },
  limits: AttachmentLimits
): string => {
  const locale = getLocale();
  switch (reason) {
    case "too-large": {
      return m.compose_file_too_large({
        max: formatBytes(limits.maxFile, locale),
        name,
      });
    }
    case "total-too-large": {
      return m.compose_total_too_large({
        max: formatBytes(limits.maxTotal ?? 0, locale),
        name,
      });
    }
    case "empty": {
      return m.compose_file_empty({ name });
    }
    default: {
      return reason satisfies never;
    }
  }
};

const hasFiles = (event: DragEvent): boolean =>
  event.dataTransfer.types.includes("Files");

/** What the body needs besides the draft: images and the text format. */
const useComposeBody = (
  compose: ReturnType<typeof useCompose>,
  uploads: ReturnType<typeof useUploads>
) => {
  const toast = useToast();
  const { draft } = compose;
  const sender = draft.identity;
  // Images inserted from this device: shown from their local copy.
  const [localImages, setLocalImages] = useState<ReadonlyMap<string, string>>(
    () => new Map()
  );
  const inlineImages = useInlineImages(
    draft.attachments.filter(
      (attachment) =>
        attachment.cid === undefined ||
        !localImages.has(normalizeContentId(attachment.cid))
    )
  );
  const inlineImage = (cid: string): string | undefined =>
    localImages.get(normalizeCid(cid)) ?? inlineImages.get(normalizeCid(cid));
  const imageInput = useRef<HTMLInputElement>(null);
  // Where the image being picked goes (the editor's insert at the cursor).
  const insertImage = useRef<((image: InsertedImage) => void) | null>(null);
  const [confirmPlain, setConfirmPlain] = useState(false);
  const textSignature = sender === null ? null : plainSignature(sender);
  // The text signature now in a plain-text body, to swap it when the
  // sender changes (the rich editor swaps its own block).
  const appliedTextSignature = useRef(textSignature);

  /** A local image for the body: checked, shown, uploaded, inserted. */
  const addImage = async (file: File): Promise<void> => {
    const insert = insertImage.current;
    insertImage.current = null;
    if (insert === null) {
      return;
    }
    if (!canInsertImage(file)) {
      toast.show({
        priority: "high",
        title: m.compose_image_invalid({ name: file.name }),
        tone: "danger",
      });
      return;
    }
    const cid = `${crypto.randomUUID()}@nebula-mail`;
    try {
      const url = await toDataUrl(file);
      setLocalImages(
        (current) => new Map([...current, [normalizeContentId(cid), url]])
      );
      const { limits, rejected } = await uploads.add([file], {
        inline: {
          cid,
          onUploaded: (attachment) => {
            insert({ alt: attachment.name, cid });
          },
        },
      });
      for (const item of rejected) {
        toast.show({
          priority: "high",
          title: rejectText(item, limits),
          tone: "danger",
        });
      }
    } catch {
      toast.show({
        priority: "high",
        title: m.compose_upload_failed(),
        tone: "danger",
      });
    }
  };

  const toPlainText = (): void => {
    compose.setBody({
      html: "",
      // The text as typed up to now (the draft's html may lag behind).
      plainText: htmlToPlainText(compose.readDraft().html, textSignature),
    });
    appliedTextSignature.current = textSignature;
  };

  const toHtml = (): void => {
    compose.setBody({
      html: plainTextToHtml(draft.plainText ?? "", sender?.signature ?? null),
      plainText: null,
    });
  };

  // The sender changed in plain-text mode: swap the signature in the text.
  // Only when the signature or the mode changes (not on every keystroke):
  // the applied signature changes nowhere else but in toPlainText, which
  // also changes the mode.
  const plainMode = draft.plainText !== undefined;
  const swapSignature = useEffectEvent((signature: string | null) => {
    const text = draft.plainText;
    if (text === undefined || signature === appliedTextSignature.current) {
      return;
    }
    const swapped = swapTextSignature(
      text,
      appliedTextSignature.current,
      signature
    );
    appliedTextSignature.current = signature;
    if (swapped !== text) {
      compose.update({ plainText: swapped });
    }
  });
  useEffect(() => {
    if (plainMode) {
      swapSignature(textSignature);
    }
  }, [textSignature, plainMode]);

  return {
    addImage,
    confirmPlain,
    imageInput,
    plain: draft.plainText !== undefined,
    richProps: {
      inlineImage,
      onInsertImage: (insert: (image: InsertedImage) => void): void => {
        insertImage.current = insert;
        imageInput.current?.click();
      },
      ...(draft.bodyStyle === undefined
        ? {}
        : {
            defaultFont: draft.bodyStyle.font,
            defaultFontSize: draft.bodyStyle.size,
          }),
    },
    /** HTML → text asks first (formatting is lost); text → HTML doesn't. */
    setConfirmPlain,
    setPlain: (checked: boolean): void => {
      if (checked) {
        setConfirmPlain(true);
      } else {
        toHtml();
      }
    },
    toPlainText,
  };
};

type Compose = ReturnType<typeof useCompose>;
type Uploads = ReturnType<typeof useUploads>;

/** What the parts show. */
export interface ComposerFormState {
  readonly window: ComposeWindowState;
  readonly identities: readonly Identity[];
  readonly draft: Draft;
  /** The subject, or "(untitled)". */
  readonly title: string;
  /** Replies start in the body, above the (collapsed) quote. */
  readonly isReply: boolean;
  readonly showCc: boolean;
  readonly showBcc: boolean;
  /** What blocks sending, once the user tried to send; else null. */
  readonly shownProblem: Problem | null;
  /** «Send» was pressed during uploads and waits for them. */
  readonly sendWaiting: boolean;
  /** The send button's spinner (sending, or waiting for uploads). */
  readonly sendBusy: boolean;
  readonly sending: boolean;
  readonly closing: boolean;
  /** Files are being dragged over the window. */
  readonly dragging: boolean;
  /** The body is plain text (no formatting). */
  readonly plain: boolean;
  /** "Switch to plain text?" is open. */
  readonly confirmPlain: boolean;
  readonly save: {
    readonly state: Compose["saveState"];
    readonly savedAt: Compose["savedAt"];
    readonly refusal: ServerRefusal | null;
    readonly backedUp: boolean;
  };
  readonly uploads: Uploads["uploads"];
  /** The server can send read receipts (the menu item is offered). */
  readonly receipts: boolean;
  /** Scheduled send: null when the server can't hold messages. */
  readonly scheduleLimit: ScheduleLimit | null;
  /** When the send menu opened (its quick times count from then). */
  readonly menuOpenedAt: Date | null;
  /** When "Schedule send" opened; null: closed. */
  readonly scheduleOpenedAt: Date | null;
}

/** What the parts can do. */
export interface ComposerFormActions {
  readonly update: Compose["update"];
  /** The HTML editor changed (use-compose.ts `editBody`). */
  readonly editBody: Compose["editBody"];
  readonly setIdentity: Compose["setIdentity"];
  readonly revealCc: () => void;
  readonly revealBcc: () => void;
  readonly addFiles: (files: readonly File[], pasted?: boolean) => void;
  /** Sends now, or at `sendAt` (scheduled). */
  readonly send: (sendAt?: Date) => void;
  readonly openSchedule: () => void;
  readonly closeSchedule: () => void;
  readonly setMenuOpenedAt: (at: Date | null) => void;
  readonly saveDraft: () => void;
  readonly close: () => void;
  readonly discard: () => void;
  readonly minimize: () => void;
  readonly restore: () => void;
  readonly toggleMaximized: () => void;
  readonly setPlain: (checked: boolean) => void;
  readonly setConfirmPlain: (open: boolean) => void;
  /** Turns the body into plain text (after the confirmation). */
  readonly toPlainText: () => void;
  readonly cancelUpload: (key: string) => void;
  readonly removeAttachment: Compose["removeAttachment"];
  /** A local image picked for the body (see meta.imageInputRef). */
  readonly addImage: (file: File) => void;
}

/** Refs, handlers and labels the parts wire up; not state. */
export interface ComposerFormMeta {
  readonly keys: ReturnType<typeof useKeys>;
  /** The hidden "Attach" file input. */
  readonly fileInputRef: RefObject<HTMLInputElement | null>;
  /** The hidden "Insert image" file input. */
  readonly imageInputRef: RefObject<HTMLInputElement | null>;
  /** What the rich editor needs for images and the default font. */
  readonly richProps: ReturnType<typeof useComposeBody>["richProps"];
  /** ⌘↵ in a field: send. */
  readonly onFieldKeyDown: (event: KeyboardEvent<HTMLElement>) => void;
  /** ⌘S / ⌘⇧↵ anywhere in the window. */
  readonly onWindowKeyDown: (event: KeyboardEvent<HTMLElement>) => void;
  readonly onDragOver: (event: DragEvent<HTMLElement>) => void;
  readonly onDragLeave: (event: DragEvent<HTMLElement>) => void;
  readonly onDrop: (event: DragEvent<HTMLElement>) => void;
}

export interface ComposerForm {
  readonly state: ComposerFormState;
  readonly actions: ComposerFormActions;
  readonly meta: ComposerFormMeta;
}

const ComposerFormContext = createContext<ComposerForm | null>(null);

export const useComposerForm = (): ComposerForm => {
  const value = use(ComposerFormContext);
  if (value === null) {
    throw new Error("useComposerForm must be used inside ComposerFormProvider");
  }
  return value;
};

export interface ComposerFormProviderProps {
  readonly window: ComposeWindowState;
  readonly initial: Draft;
  readonly identities: readonly Identity[];
  readonly children: ReactNode;
}

export const ComposerFormProvider = ({
  children,
  identities,
  initial,
  window,
}: ComposerFormProviderProps) => {
  const composer = useComposer();
  const toast = useToast();
  const keys = useKeys();
  const compose = useCompose(initial, {
    backupId: window.backupId,
    fromBackup: window.fromBackup,
    identities,
    identityEmail: window.init.identityEmail,
    restored: window.init.restored === true,
    ...(window.draftId === undefined ? {} : { draftId: window.draftId }),
  });
  const { draft } = compose;
  const receipts = useReceiptsSupported();
  const [showCc, setShowCc] = useState(initial.cc.length > 0);
  const [showBcc, setShowBcc] = useState(initial.bcc.length > 0);
  const [attempted, setAttempted] = useState(false);
  const [closing, setClosing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const uploads = useUploads(draft.attachments, compose.addAttachment);
  const body = useComposeBody(compose, uploads);
  const problem: Problem | null =
    compose.problem ?? (uploads.busy ? "uploading" : null);
  // Replies start in the body, above the quote.
  const isReply = initial.to.length > 0;

  const addFiles = async (
    files: readonly File[],
    pasted = false
  ): Promise<void> => {
    if (files.length === 0) {
      return;
    }
    try {
      const { limits, rejected } = await uploads.add(files, { pasted });
      for (const item of rejected) {
        toast.show({
          priority: "high",
          title: rejectText(item, limits),
          tone: "danger",
        });
      }
    } catch {
      toast.show({
        priority: "high",
        title: m.compose_upload_failed(),
        tone: "danger",
      });
    }
  };

  const onDragOver = (event: DragEvent<HTMLElement>): void => {
    if (hasFiles(event)) {
      event.preventDefault();
      setDragging(true);
    }
  };

  const onDragLeave = (event: DragEvent<HTMLElement>): void => {
    // Only when leaving the window, not when moving between its children.
    if (
      !(event.relatedTarget instanceof Node) ||
      !event.currentTarget.contains(event.relatedTarget)
    ) {
      setDragging(false);
    }
  };

  const onDrop = (event: DragEvent<HTMLElement>): void => {
    setDragging(false);
    if (hasFiles(event)) {
      event.preventDefault();
      void addFiles([...event.dataTransfer.files]);
    }
  };

  const title =
    draft.subject.trim() === "" ? m.composer_untitled() : draft.subject;

  const close = async (): Promise<void> => {
    setClosing(true);
    // Closing never loses the window: if the last save fails, the draft
    // from the previous autosave stays in Drafts.
    await compose.flush().catch(() => null);
    composer.close(window.key);
  };

  const discard = async (): Promise<void> => {
    composer.close(window.key);
    await compose.discard().catch(() => null);
    toast.show({ title: m.toast_draft_discarded() });
  };

  // Scheduled send: only when the server can hold it.
  const scheduleLimit = useScheduleLimit();
  const queryClient = useQueryClient();
  const [scheduleOpenedAt, setScheduleOpenedAt] = useState<Date | null>(null);
  const [menuOpenedAt, setMenuOpenedAt] = useState<Date | null>(null);

  const sendNow = async (sendAt?: Date): Promise<void> => {
    setAttempted(true);
    if (problem !== null || compose.sending) {
      return;
    }
    try {
      const outcome = await compose.send(
        sendAt === undefined ? {} : { sendAt }
      );
      // What was sent (with the last keystrokes), for Undo to reopen.
      composer.sent(window.key, compose.readDraft(), outcome);
    } catch (error) {
      if (sendAt !== undefined) {
        const text = await scheduleErrorText(error, scheduleLimit, queryClient);
        if (text !== null) {
          toast.show({ priority: "high", title: text, tone: "danger" });
          return;
        }
      }
      const refusal = serverRefusalOf(error);
      toast.show({
        priority: "high",
        title: refusal === null ? m.toast_send_failed() : refusalText(refusal),
        tone: "danger",
      });
    }
  };

  // «Send» while uploading waits for the uploads (use-uploads.ts).
  const afterUploads = useSendAfterUploads<Date | undefined>(
    uploads,
    compose.sending,
    (sendAt) => {
      void sendNow(sendAt);
    }
  );

  const send = async (sendAt?: Date): Promise<void> => {
    if (problem === "uploading") {
      setAttempted(true);
      afterUploads.queue(sendAt);
      return;
    }
    await sendNow(sendAt);
  };

  const openSchedule = (): void => {
    setAttempted(true);
    if (problem === null && !compose.sending && scheduleLimit !== null) {
      setScheduleOpenedAt(new Date());
    }
  };

  // Saves now (autosave does it anyway every few seconds) and says so.
  const saveDraft = async (): Promise<void> => {
    // Never edited and never saved: nothing to keep yet.
    if (compose.saveState === "idle" && compose.savedAt === null) {
      toast.show({ title: m.toast_draft_nothing() });
      return;
    }
    await compose.flush().catch(() => null);
    toast.show({ title: m.toast_draft_saved() });
  };

  // ⌘S / Ctrl+S anywhere in the window: save the draft, not the page.
  // ⌘⇧↵: schedule send (pick the date and time).
  const handleWindowKeyDown = (event: KeyboardEvent<HTMLElement>): void => {
    const mod = isApplePlatform() ? event.metaKey : event.ctrlKey;
    if (mod && !event.shiftKey && event.key.toLowerCase() === "s") {
      event.preventDefault();
      void saveDraft();
    }
    if (mod && event.shiftKey && event.key === "Enter") {
      event.preventDefault();
      openSchedule();
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>): void => {
    const mod = isApplePlatform() ? event.metaKey : event.ctrlKey;
    // With Shift it's "schedule" (handleWindowKeyDown), never "send now".
    if (mod && !event.shiftKey && event.key === "Enter") {
      event.preventDefault();
      void send();
    }
  };

  const value: ComposerForm = {
    actions: {
      addFiles: (files, pasted = false) => {
        void addFiles(files, pasted);
      },
      addImage: (file) => {
        void body.addImage(file);
      },
      cancelUpload: (key) => {
        afterUploads.drop();
        uploads.cancel(key);
      },
      close: () => {
        void close();
      },
      closeSchedule: () => {
        setScheduleOpenedAt(null);
      },
      discard: () => {
        void discard();
      },
      minimize: () => {
        // The editor unmounts and later starts again from the draft's html:
        // bring it up to date first.
        compose.readDraft();
        composer.update(window.key, { maximized: false, minimized: true });
      },
      openSchedule,
      removeAttachment: compose.removeAttachment,
      restore: () => {
        composer.update(window.key, { minimized: false });
      },
      revealBcc: () => {
        setShowBcc(true);
      },
      revealCc: () => {
        setShowCc(true);
      },
      saveDraft: () => {
        void saveDraft();
      },
      send: (sendAt) => {
        void send(sendAt);
      },
      setConfirmPlain: body.setConfirmPlain,
      setIdentity: compose.setIdentity,
      setMenuOpenedAt,
      setPlain: body.setPlain,
      toPlainText: body.toPlainText,
      toggleMaximized: () => {
        composer.update(window.key, { maximized: !window.maximized });
      },
      update: compose.update,
      editBody: compose.editBody,
    },
    meta: {
      fileInputRef: fileInput,
      imageInputRef: body.imageInput,
      keys,
      onDragLeave,
      onDragOver,
      onDrop,
      onFieldKeyDown: handleKeyDown,
      onWindowKeyDown: handleWindowKeyDown,
      richProps: body.richProps,
    },
    state: {
      closing,
      confirmPlain: body.confirmPlain,
      draft,
      dragging,
      identities,
      isReply,
      menuOpenedAt,
      plain: body.plain,
      receipts,
      save: {
        backedUp: compose.backedUp,
        refusal: compose.saveRefusal,
        savedAt: compose.savedAt,
        state: compose.saveState,
      },
      scheduleLimit,
      scheduleOpenedAt,
      sendBusy: afterUploads.busy,
      sendWaiting: afterUploads.waiting,
      sending: compose.sending,
      // The sender's own Bcc (RFC 8621 §6) is never hidden.
      showBcc: showBcc || draft.bcc.length > 0,
      showCc,
      shownProblem: attempted ? problem : null,
      title,
      uploads: uploads.uploads,
      window,
    },
  };

  return (
    <ComposerFormContext
      // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object and its functions.
      value={value}
    >
      {children}
    </ComposerFormContext>
  );
};
