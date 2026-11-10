// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A copy of a draft on this device when the server can't save it (offline)
// kept in IndexedDB until a later save, a send or a discard
// works, and offered again when the app opens. Per account; forgotten on
// logout (device-db.ts).

import { BlobId, EmailId } from "@nebulamail/jmap-client";
import { COMPOSE_FONT_IDS, COMPOSE_FONT_SIZES } from "@nebulamail/mail-render";
import { z } from "zod";

import { accountKey } from "./account-key";
import type { Draft } from "./compose";
import { withStore } from "./device-db";
import { IdentitySchema } from "./identity-cache";

const STORE = "backups";

/**
 * Written with no account known at all (opened offline before anything
 * was saved on this device): offered to whoever connects next in this
 * browser, and re-labelled when saved again. Logout still wipes it.
 */
const UNKNOWN_ACCOUNT = "";

const currentAccount = async (): Promise<string> =>
  await accountKey().catch(() => UNKNOWN_ACCOUNT);

export interface DraftBackup {
  /** The compose window's backup id (stable across reopenings). */
  readonly id: string;
  readonly account: string;
  /** When it was written (ms since epoch). */
  readonly savedAt: number;
  readonly draft: Draft;
  /** The server draft it continues, if one was saved before. */
  readonly draftId?: EmailId;
}

const RecipientSchema = z.object({
  email: z.string(),
  name: z.string().exactOptional(),
});

const DraftSchema = z.object({
  attachments: z.array(
    z.object({
      blobId: BlobId,
      cid: z.string().min(1).exactOptional(),
      name: z.string(),
      size: z.number().nonnegative(),
      type: z.string(),
    })
  ),
  bcc: z.array(RecipientSchema),
  cc: z.array(RecipientSchema),
  html: z.string(),
  // Plain-text mode and the default font and size (2026-11-01).
  plainText: z.string().exactOptional(),
  bodyStyle: z
    .object({
      font: z.enum(COMPOSE_FONT_IDS),
      size: z.enum(COMPOSE_FONT_SIZES),
    })
    .exactOptional(),
  // null: written before the identities were known (opened offline).
  identity: IdentitySchema.nullable(),
  inReplyTo: z.array(z.string()).exactOptional(),
  readReceipt: z.boolean().exactOptional(),
  references: z.array(z.string()).exactOptional(),
  subject: z.string(),
  to: z.array(RecipientSchema),
});

const BackupSchema = z.object({
  account: z.string(),
  draft: DraftSchema,
  draftId: EmailId.exactOptional(),
  id: z.string().min(1),
  savedAt: z.number(),
});

/**
 * A new id for a compose window's copy. Not `crypto.randomUUID`: that one
 * only exists on https/localhost, and a self-hosted install may be on http.
 */
export const newBackupId = (): string =>
  Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
    byte.toString(16).padStart(2, "0")
  ).join("");

/** A stored record → a backup, or null if it isn't one we can use. */
export const parseBackup = (value: unknown): DraftBackup | null => {
  const parsed = BackupSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
};

export const saveBackup = async (backup: {
  readonly id: string;
  readonly draft: Draft;
  readonly draftId?: EmailId | undefined;
}): Promise<void> => {
  const record: DraftBackup = {
    account: await currentAccount(),
    draft: backup.draft,
    id: backup.id,
    savedAt: Date.now(),
    ...(backup.draftId === undefined ? {} : { draftId: backup.draftId }),
  };
  await withStore(STORE, "readwrite", (store) => store.put(record));
};

export const deleteBackup = async (id: string): Promise<void> => {
  await withStore(STORE, "readwrite", (store) => store.delete(id));
};

/** This account's backups, newest first. */
export const listBackups = async (): Promise<DraftBackup[]> => {
  const account = await currentAccount();
  const records: unknown[] = await withStore(STORE, "readonly", (store) =>
    store.getAll()
  );
  return records
    .map(parseBackup)
    .filter(
      (backup): backup is DraftBackup =>
        backup !== null &&
        (backup.account === account || backup.account === UNKNOWN_ACCOUNT)
    )
    .toSorted((a, b) => b.savedAt - a.savedAt);
};

/** Every account's backups go (logout on this device), with the rest. */
export { forgetDeviceDb as forgetBackups } from "./device-db";
