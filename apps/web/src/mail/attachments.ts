// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Attachments of a message being written: uploaded as blobs
// (with progress) and referenced by blobId in the draft. Sizes are checked
// against the session's `maxSizeUpload` (per file) and the account's
// `maxSizeAttachmentsPerEmail` (all together) before uploading.

import type { AccountId, BlobId } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  hasCapability,
  mailCapability,
} from "@nebulamail/jmap-client";
import { stripBidiControls, stripInvisible } from "@nebulamail/mail-render";

import { getClient } from "./client";

/** An attachment the server already has. */
export interface DraftAttachment {
  readonly blobId: BlobId;
  readonly name: string;
  readonly type: string;
  readonly size: number;
  /**
   * An inline image of the body (`<img src="cid:…">`, from a quoted or
   * forwarded email): sent as an inline part with this Content-ID, and
   * only while the body still shows it.
   */
  readonly cid?: string;
  /**
   * The blob is in another account (an email of a folder shared with us).
   * A draft can only use blobs of our own account: it is copied into ours
   * first (`copyToOwnAccount`, Blob/copy).
   */
  readonly accountId?: AccountId;
}

/** The attachments after copying, and how many couldn't be copied. */
export interface CopiedAttachments {
  readonly attachments: DraftAttachment[];
  readonly failed: number;
}

const withoutAccount = ({
  accountId: _accountId,
  ...attachment
}: DraftAttachment): DraftAttachment => attachment;

/**
 * Points the attachments of other accounts at their copies in `primary`
 * (`copied`: account → original blob id → copy). Those without a copy are
 * left out and counted.
 */
export const applyBlobCopies = (
  attachments: readonly DraftAttachment[],
  primary: AccountId,
  copied: ReadonlyMap<AccountId, Partial<Record<string, BlobId>>>
): CopiedAttachments => {
  let failed = 0;
  const result = attachments.flatMap((attachment) => {
    const from = attachment.accountId;
    if (from === undefined || from === primary) {
      return [withoutAccount(attachment)];
    }
    const blobId = copied.get(from)?.[attachment.blobId];
    if (blobId === undefined) {
      failed += 1;
      return [];
    }
    return [{ ...withoutAccount(attachment), blobId }];
  });
  return { attachments: result, failed };
};

/**
 * Copies the blobs of other accounts into ours (RFC 8620 §6.3), one
 * `Blob/copy` per account in one request, so a forward or a reply from a
 * shared folder can carry its files and inline images.
 */
export const copyToOwnAccount = async (
  attachments: readonly DraftAttachment[]
): Promise<CopiedAttachments> => {
  const client = await getClient();
  const primary = client.primaryAccountId();
  const byAccount = new Map<AccountId, BlobId[]>();
  for (const attachment of attachments) {
    const from = attachment.accountId;
    if (from !== undefined && from !== primary) {
      byAccount.set(from, [...(byAccount.get(from) ?? []), attachment.blobId]);
    }
  }
  const copied = new Map<AccountId, Partial<Record<string, BlobId>>>();
  if (byAccount.size > 0) {
    const accounts = [...byAccount];
    const responses = await client.request((b) => {
      // Stalwart wants the Blob capability (RFC 9404) for Blob/copy,
      // although RFC 8620 puts the method in core (checked live).
      if (hasCapability(client.session, CAPABILITIES.blob)) {
        b.using(CAPABILITIES.blob);
      }
      return accounts.map(([fromAccountId, blobIds]) =>
        b.call("Blob/copy", {
          accountId: primary,
          blobIds: [...new Set(blobIds)],
          fromAccountId,
        })
      );
    });
    for (const response of responses) {
      copied.set(response.fromAccountId, response.copied);
    }
  }
  return applyBlobCopies(attachments, primary, copied);
};

export interface AttachmentLimits {
  /** Largest file the server accepts (session `maxSizeUpload`). */
  readonly maxFile: number;
  /** All attachments of one email together, if the server says. */
  readonly maxTotal: number | null;
}

export type RejectReason = "too-large" | "total-too-large" | "empty";

export interface FileCheck {
  readonly accepted: readonly File[];
  readonly rejected: readonly {
    readonly name: string;
    readonly size: number;
    readonly reason: RejectReason;
  }[];
}

/**
 * Which files can be added: each within `maxFile`, and all of them (with
 * the ones already attached) within `maxTotal`. In order: once the total is
 * reached, later files are refused.
 */
export const checkFiles = (
  files: readonly File[],
  current: readonly { readonly size: number }[],
  limits: AttachmentLimits
): FileCheck => {
  const accepted: File[] = [];
  const rejected: { name: string; size: number; reason: RejectReason }[] = [];
  let total = current.reduce((sum, item) => sum + item.size, 0);
  for (const file of files) {
    let reason: RejectReason | null = null;
    if (file.size === 0) {
      reason = "empty";
    } else if (file.size > limits.maxFile) {
      reason = "too-large";
    } else if (
      limits.maxTotal !== null &&
      total + file.size > limits.maxTotal
    ) {
      reason = "total-too-large";
    }
    if (reason === null) {
      accepted.push(file);
      total += file.size;
    } else {
      rejected.push({ name: file.name, reason, size: file.size });
    }
  }
  return { accepted, rejected };
};

const CONTROL = /\p{Cc}/gu;

/**
 * The file name sent to the recipient: no control, bidi or invisible
 * characters (an RLO in "factura<RLO>fdp.exe" makes it read "facturaexe.pdf").
 */
export const attachmentName = (name: string): string => {
  const clean = stripInvisible(stripBidiControls(name))
    .replaceAll(CONTROL, "")
    .trim();
  return clean === "" ? "attachment" : clean;
};

/** A pasted image has no useful name ("image.png"); give it a dated one. */
export const pastedFileName = (file: File, now: Date): string => {
  if (file.name !== "" && file.name !== "image.png") {
    return file.name;
  }
  const extension = /^image\/(?<ext>[a-z]+)$/u.exec(file.type)?.groups?.ext;
  const stamp = now.toISOString().slice(0, 19).replaceAll(":", "-");
  return `image-${stamp}.${extension ?? "png"}`;
};

export const attachmentLimits = async (): Promise<AttachmentLimits> => {
  const client = await getClient();
  const mail = mailCapability(client.session, client.primaryAccountId());
  return {
    maxFile: client.session.core.maxSizeUpload,
    maxTotal: mail?.maxSizeAttachmentsPerEmail ?? null,
  };
};

export interface UploadOptions {
  /**
   * 0–1, or `null` when the transport can't report progress (same-origin
   * uploads go through fetch): show an indeterminate bar.
   */
  readonly onProgress?: (fraction: number | null) => void;
  readonly signal?: AbortSignal;
  /** Name to use instead of the file's (pasted images). */
  readonly name?: string;
}

/**
 * Starts reporting an upload of `size` bytes: `null` right away when the
 * transport can't report progress (its only call, at the end, is ignored:
 * the upload is done then), else the fraction sent on each call.
 */
export const startProgress = (
  reportsProgress: boolean,
  size: number,
  onProgress: UploadOptions["onProgress"]
): ((sent: number, total: number | undefined) => void) => {
  if (!reportsProgress) {
    onProgress?.(null);
    return () => {
      // Indeterminate until the upload ends.
    };
  }
  return (sent, total) => {
    const of = total ?? size;
    onProgress?.(of > 0 ? Math.min(1, sent / of) : 0);
  };
};

/** Uploads one file as a blob; the draft then refers to its blobId. */
export const uploadAttachment = async (
  file: File,
  { name, onProgress, signal }: UploadOptions = {}
): Promise<DraftAttachment> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const type = file.type === "" ? "application/octet-stream" : file.type;
  const result = await client.upload(accountId, file, {
    onProgress: startProgress(
      client.uploadReportsProgress(accountId, file),
      file.size,
      onProgress
    ),
    type,
    ...(signal === undefined ? {} : { signal }),
  });
  return {
    blobId: result.blobId,
    name: attachmentName(name ?? file.name),
    size: result.size,
    type,
  };
};
