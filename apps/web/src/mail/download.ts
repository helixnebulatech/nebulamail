// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Downloads: fetched with the token, saved with
// `<a download>` from a `blob:` URL. Always labelled as opaque bytes so the
// browser never renders it (an HTML or SVG attachment stays a file).
// "View original" reads the raw message as text and is never rendered.

import type { AttachmentAnalysis } from "@nebulamail/mail-render";
import { BIDI_CONTROLS, cleanDisplayText } from "@nebulamail/mail-render";

import type { Attachment, EmailDetail } from "./api";
import { getClient } from "./client";

/** Matches the proxy's per-email budget; bigger files need streaming (TODO). */
const MAX_DOWNLOAD_BYTES = 100 * 1024 * 1024;

/** "View original" shows at most this much; the .eml download has it all. */
export const MAX_ORIGINAL_BYTES = 512 * 1024;

export const saveBlob = (blob: Blob, fileName: string): void => {
  const url = URL.createObjectURL(
    new Blob([blob], { type: "application/octet-stream" })
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  link.rel = "noopener";
  document.body.append(link);
  link.click();
  link.remove();
  // Give the browser time to start the download before freeing the bytes.
  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 60_000);
};

/**
 * The name an attachment is saved as: the one its warnings are about
 * (without the trailing dots and spaces Windows drops), or `unnamed` when
 * nothing is left ("...").
 */
export const attachmentFileName = (
  analysis: Pick<AttachmentAnalysis, "fileName">,
  unnamed: string
): string => (analysis.fileName === "" ? unnamed : analysis.fileName);

export const downloadAttachment = async (
  attachment: Attachment,
  fileName: string
): Promise<void> => {
  const client = await getClient();
  const blob = await client.downloadBlob({
    accountId: attachment.accountId ?? client.primaryAccountId(),
    blobId: attachment.blobId,
    maxBytes: MAX_DOWNLOAD_BYTES,
    name: fileName,
    type: "application/octet-stream",
  });
  saveBlob(blob, fileName);
};

// oxlint-disable-next-line no-control-regex -- control characters are what we remove
const UNSAFE_IN_NAMES = /[\u0000-\u001F\u007F"*/:<>?\\|]/gu;
const RESERVED_NAMES = /^(?:con|prn|aux|nul|com\d|lpt\d)$/iu;
const MAX_NAME_LENGTH = 80;

/**
 * `<subject>.eml`, safe on every OS: no bidi or invisible characters
 * ("factura\u202Efdp.eml"), no path separators or characters Windows refuses,
 * no leading dots, no reserved device names, at most 80 characters
 * (graphemes).
 */
export const emlFileName = (subject: string): string => {
  const cleaned = cleanDisplayText(subject)
    .replaceAll(UNSAFE_IN_NAMES, "_")
    .replaceAll(/\s+/gu, " ")
    .replace(/^[\s.]+/u, "");
  // Cut by user-perceived characters, so an emoji is never split.
  const base = Array.from(
    new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(cleaned),
    (part) => part.segment
  )
    .slice(0, MAX_NAME_LENGTH)
    .join("")
    .replace(/[\s.]+$/u, "");
  if (base === "") {
    return "message.eml";
  }
  return `${RESERVED_NAMES.test(base) ? `_${base}` : base}.eml`;
};

/** The raw message (RFC 5322) as `<subject>.eml`. */
export const downloadEml = async (email: EmailDetail): Promise<void> => {
  const client = await getClient();
  const fileName = emlFileName(email.subject);
  const blob = await client.downloadBlob({
    accountId: email.accountId ?? client.primaryAccountId(),
    blobId: email.blobId,
    maxBytes: MAX_DOWNLOAD_BYTES,
    name: fileName,
    type: "application/octet-stream",
  });
  saveBlob(blob, fileName);
};

export interface OriginalSource {
  /** Decoded as UTF-8 (invalid bytes become U+FFFD). Show as text only. */
  readonly text: string;
  /** The message is bigger than what was read. */
  readonly truncated: boolean;
}

/** Bidi controls made visible, so the source can't reorder itself on screen. */
export const showControls = (text: string): string =>
  text.replaceAll(
    BIDI_CONTROLS,
    (char) =>
      `[U+${(char.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}]`
  );

/** Reads at most `max` bytes of a stream, then cancels the rest. */
export const readPrefix = async (
  body: ReadableStream<Uint8Array> | null,
  max: number
): Promise<{ readonly bytes: Uint8Array; readonly truncated: boolean }> => {
  if (body === null) {
    return { bytes: new Uint8Array(0), truncated: false };
  }
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  let truncated = false;
  for (;;) {
    // oxlint-disable-next-line no-await-in-loop -- a stream is read in order.
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    const room = max - total;
    if (value.byteLength > room) {
      chunks.push(value.subarray(0, room));
      total = max;
      truncated = true;
      break;
    }
    chunks.push(value);
    total += value.byteLength;
  }
  if (truncated) {
    await reader.cancel();
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return { bytes, truncated };
};

/** The start of the raw message, for "View original". */
export const readOriginal = async (
  email: EmailDetail,
  max = MAX_ORIGINAL_BYTES
): Promise<OriginalSource> => {
  const client = await getClient();
  const response = await client.download({
    accountId: email.accountId ?? client.primaryAccountId(),
    blobId: email.blobId,
    name: "message.eml",
    type: "application/octet-stream",
  });
  const { bytes, truncated } = await readPrefix(response.body, max);
  return {
    // `stream: true` keeps a multi-byte character cut at the end from
    // turning into garbage in the middle of the text.
    text: showControls(
      new TextDecoder("utf-8").decode(bytes, { stream: truncated })
    ),
    truncated: truncated || email.size > max,
  };
};
