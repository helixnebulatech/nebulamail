// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The identities on this device, so a compose window opens without a
// connection: id, name, address, signature, Reply-To and
// Bcc of each identity the server listed last time. In IndexedDB
// (device-db.ts), not
// localStorage, and wiped on logout. One record, for the account
// that connected last; its key is also what account-key.ts falls back to
// when the JMAP session can't be reached.

import { IdentityId } from "@nebulamail/jmap-client";
import { sanitizeComposeHtml } from "@nebulamail/mail-render";
import { z } from "zod";

import type { Identity } from "./compose";
import { withStore } from "./device-db";

const STORE = "identities";
const RECORD = "current";

const RecipientSchema = z.object({
  email: z.string(),
  name: z.string().exactOptional(),
});

export const IdentitySchema = z.object({
  // RFC 8621 §6 Reply-To and Bcc (2026-11-08); older copies don't have them.
  bcc: z.array(RecipientSchema).exactOptional(),
  email: z.string(),
  id: IdentityId,
  name: z.string(),
  replyTo: z.array(RecipientSchema).exactOptional(),
  signature: z.string().nullable(),
  // Added 2026-11-01 (plain-text mode); older copies don't have it.
  textSignature: z.string().nullable().exactOptional(),
});

const RecordSchema = z.object({
  account: z.string(),
  id: z.literal(RECORD),
  identities: z.array(IdentitySchema),
  savedAt: z.number(),
});

export interface SavedIdentities {
  /** account-key.ts: the JMAP server and username. */
  readonly account: string;
  readonly identities: readonly Identity[];
}

export const saveIdentities = async (
  account: string,
  identities: readonly Identity[]
): Promise<void> => {
  const record: z.infer<typeof RecordSchema> = {
    account,
    id: RECORD,
    identities: identities.map(({ bcc, replyTo, ...identity }) => ({
      ...identity,
      ...(bcc === undefined ? {} : { bcc: [...bcc] }),
      ...(replyTo === undefined ? {} : { replyTo: [...replyTo] }),
    })),
    savedAt: Date.now(),
  };
  await withStore(STORE, "readwrite", (store) => store.put(record));
};

/** What was saved, or null. Signatures are sanitized again on the way in. */
export const loadIdentities = async (): Promise<SavedIdentities | null> => {
  const value: unknown = await withStore(STORE, "readonly", (store) =>
    store.get(RECORD)
  );
  const parsed = RecordSchema.safeParse(value);
  if (!parsed.success) {
    return null;
  }
  return {
    account: parsed.data.account,
    identities: parsed.data.identities.map((identity) => ({
      ...identity,
      signature:
        identity.signature === null
          ? null
          : sanitizeComposeHtml(identity.signature),
    })),
  };
};
