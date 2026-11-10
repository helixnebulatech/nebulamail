// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Attachment uploads of one compose window: one at a time, with progress,
// cancellable. A finished upload becomes a DraftAttachment of the draft.

import { isJmapError } from "@nebulamail/jmap-client";
import { useEffect, useEffectEvent, useRef, useState } from "react";

import type {
  AttachmentLimits,
  DraftAttachment,
  FileCheck,
} from "./attachments";
import {
  attachmentLimits,
  checkFiles,
  pastedFileName,
  uploadAttachment,
} from "./attachments";

export interface Upload {
  readonly key: string;
  readonly name: string;
  readonly size: number;
  /** 0–1, or `null` while the transport can't tell (indeterminate bar). */
  readonly progress: number | null;
  readonly failed: boolean;
}

export interface AddOptions {
  /** Clipboard images: named by date. */
  readonly pasted?: boolean;
  /**
   * An image for the body (one file): uploaded as an inline part with
   * this Content-ID; `onUploaded` gets it once uploaded, to show it.
   */
  readonly inline?: {
    readonly cid: string;
    readonly onUploaded: (attachment: DraftAttachment) => void;
  };
}

export interface Uploads {
  readonly uploads: readonly Upload[];
  /** Still uploading (sending must wait). */
  readonly busy: boolean;
  /**
   * Checks the limits and queues the files that fit; returns the ones
   * refused. `pasted` names clipboard images ("image.png") by date.
   */
  readonly add: (
    files: readonly File[],
    options?: AddOptions
  ) => Promise<{
    readonly rejected: FileCheck["rejected"];
    readonly limits: AttachmentLimits;
  }>;
  /** Stops an upload, or forgets a failed one. */
  readonly cancel: (key: string) => void;
}

/** The progress as a whole percent (0–100), or null when unknown. */
export const progressStep = (progress: number | null): number | null =>
  progress === null
    ? null
    : Math.min(100, Math.max(0, Math.floor(progress * 100)));

let counter = 0;
const nextKey = (): string => {
  counter += 1;
  return `upload-${counter}`;
};

export const useUploads = (
  attached: readonly { readonly size: number }[],
  onUploaded: (attachment: DraftAttachment) => void
): Uploads => {
  const [uploads, setUploads] = useState<readonly Upload[]>([]);
  // Not state: they never change what is drawn.
  const controllers = useRef(new Map<string, AbortController>());
  const queue = useRef<Promise<void>>(Promise.resolve());
  const cancelled = useRef(new Set<string>());

  // Closing the window stops what is still uploading.
  useEffect(() => {
    const running = controllers.current;
    return () => {
      for (const controller of running.values()) {
        controller.abort();
      }
    };
  }, []);

  const patch = (key: string, change: Partial<Upload>): void => {
    setUploads((current) =>
      current.map((upload) =>
        upload.key === key ? { ...upload, ...change } : upload
      )
    );
  };

  const run = async (
    file: File,
    key: string,
    name: string,
    inline: AddOptions["inline"]
  ): Promise<void> => {
    const controller = new AbortController();
    controllers.current.set(key, controller);
    const forget = (): void => {
      setUploads((current) => current.filter((upload) => upload.key !== key));
    };
    // XHR fires progress many times a second; each state change redraws
    // the composer, so only a new whole percent (or a switch to/from
    // "can't tell") is drawn.
    let shown: number | null = 0;
    const attachment = await uploadAttachment(file, {
      name,
      onProgress: (progress) => {
        const step = progressStep(progress);
        if (step !== shown) {
          shown = step;
          patch(key, { progress: step === null ? null : step / 100 });
        }
      },
      signal: controller.signal,
    }).catch((error: unknown) => {
      if (isJmapError(error) && error.detail.kind === "aborted") {
        forget();
      } else {
        patch(key, { failed: true });
      }
      return null;
    });
    controllers.current.delete(key);
    if (attachment !== null) {
      forget();
      const uploaded =
        inline === undefined ? attachment : { ...attachment, cid: inline.cid };
      onUploaded(uploaded);
      inline?.onUploaded(uploaded);
    }
  };

  const add: Uploads["add"] = async (files, options: AddOptions = {}) => {
    const { inline, pasted = false } = options;
    const limits = await attachmentLimits();
    const pending = uploads.filter((upload) => !upload.failed);
    const { accepted, rejected } = checkFiles(
      files,
      [...attached, ...pending],
      limits
    );
    const now = new Date();
    for (const file of accepted) {
      const key = nextKey();
      const name = pasted ? pastedFileName(file, now) : file.name;
      setUploads((current) => [
        ...current,
        { failed: false, key, name, progress: 0, size: file.size },
      ]);
      const previous = queue.current;
      const turn = async (): Promise<void> => {
        await previous;
        // Cancelled while waiting in the queue.
        if (!cancelled.current.has(key)) {
          await run(file, key, name, inline);
        }
      };
      queue.current = turn();
    }
    return { limits, rejected };
  };

  const cancel = (key: string): void => {
    const controller = controllers.current.get(key);
    if (controller === undefined) {
      // Queued or failed: just forget it (and skip it when its turn comes).
      cancelled.current.add(key);
      setUploads((current) => current.filter((upload) => upload.key !== key));
    } else {
      controller.abort();
    }
  };

  return {
    add,
    busy: uploads.some((upload) => !upload.failed),
    cancel,
    uploads,
  };
};

export interface SendAfterUploads<T> {
  /** A send is waiting for the uploads. */
  readonly waiting: boolean;
  /** Sending, or waiting to send: the Send button is busy. */
  readonly busy: boolean;
  /** Runs `value` once the uploads end (replaces a waiting one). */
  readonly queue: (value: T) => void;
  /** Forgets the waiting send. */
  readonly drop: () => void;
}

/**
 * «Send» pressed while an upload is still running, often with a full bar:
 * the file is out but the server's answer (its blob id) isn't, which on a
 * slow path (the full BFF mode on serverless) takes a while.
 * Refusing that click looked like a dead button (2026-11-08); now it is
 * kept and `run` gets it once the uploads end, with the draft that has
 * the files. Not if an upload failed meanwhile: the user decides whether
 * to send without it.
 */
export const useSendAfterUploads = <T>(
  uploads: Uploads,
  sending: boolean,
  run: (value: T) => void
): SendAfterUploads<T> => {
  const failed = uploads.uploads.filter((upload) => upload.failed).length;
  const [waiting, setWaiting] = useState<{
    readonly value: T;
    readonly failed: number;
  } | null>(null);
  const [due, setDue] = useState<{ readonly value: T } | null>(null);
  // Adjusted during render (not in an effect): the uploads just ended.
  if (waiting !== null && !uploads.busy) {
    setWaiting(null);
    if (failed <= waiting.failed) {
      setDue({ value: waiting.value });
    }
  }
  const fire = useEffectEvent((value: T) => {
    run(value);
  });
  // Sending is a side effect: after the render with the files in the draft.
  useEffect(() => {
    if (due !== null) {
      fire(due.value);
    }
  }, [due]);
  return {
    busy: sending || waiting !== null,
    drop: () => {
      setWaiting(null);
    },
    queue: (value) => {
      setWaiting({ failed, value });
    },
    waiting: waiting !== null,
  };
};
