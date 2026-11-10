// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The synced preferences document: one small JSON file the
// devices of an account share. Each option carries the time it was
// changed, and when two devices disagree the newest change of each option
// wins (not the newest file). Options this version doesn't know are kept
// as they are, so an older app never deletes a newer one's settings.
//
// {"format":"nebula-mail-preferences","version":1,
//  "values":{"theme":{"value":"dark","at":1793000000000}, …}}

import { COMPOSE_FONT_IDS, COMPOSE_FONT_SIZES } from "@nebulamail/mail-render";
import { z } from "zod";

import {
  ImageLoadModeSchema,
  ImageRouteSchema,
  MAX_TRUSTED_SENDERS,
} from "../mail/image-policy";

export const SYNC_FORMAT = "nebula-mail-preferences";

/** What is synced, and what each value may be. */
export const SYNCED = {
  /** Default font, size and format of new messages (mail/compose-prefs.ts). */
  composeFont: z.enum(COMPOSE_FONT_IDS),
  composeFontSize: z.enum(COMPOSE_FONT_SIZES),
  composeFormat: z.enum(["html", "text"]),
  density: z.enum(["compact", "comfortable"]),
  language: z.enum(["en", "es"]),
  /** Static build: one-click unsubscribe (mail/one-click-unsubscribe.ts). */
  /** «Primera vez que te escribe» in the reader (mail/sender-new.ts). */
  newSenderHint: z.boolean(),
  oneClickUnsubscribe: z.boolean(),
  pane: z.enum(["side", "full"]),
  /** Read receipts (mail/read-receipts.ts). */
  readReceipts: z.enum(["never", "ask", "always"]),
  /**
   * Senders whose remote images load at once with "trusted-senders"
   * ("Always show from this sender", mail/image-policy.ts).
   */
  imageTrustedSenders: z
    .array(z.string().min(3).max(320))
    .max(MAX_TRUSTED_SENDERS),
  /** Remote images in the static build (mail/remote-images.ts). */
  remoteImages: z.enum(["blocked", "direct", "proxy"]),
  /** Build with our server: when remote images load (image-policy.ts). */
  remoteImagesLoad: ImageLoadModeSchema,
  /** Build with our server: how they load (proxy or from the sender). */
  remoteImagesVia: ImageRouteSchema,
  /**
   * Calendars the user took out of Tasks ("Remove from Tasks",
   * tasks/lists.ts): they don't show even if they hold tasks.
   */
  removedTaskLists: z.array(z.string().min(1).max(512)).max(200),
  /**
   * Calendars the user made task lists (`calendarKey`s, tasks/lists.ts):
   * Stalwart can't tell an empty list from a calendar.
   */
  taskLists: z.array(z.string().min(1).max(512)).max(200),
  /**
   * When the reminders of a task due on a day ring (minutes since
   * midnight; tasks/day-reminders.ts): the last time the user chose.
   */
  taskReminderTime: z.number().int().min(0).max(1439),
  theme: z.enum(["light", "dark", "system"]),
  /** The script the vacation response paused (to switch it back on). */
  vacationPausedScript: z
    .object({ id: z.string().min(1).max(255), name: z.string().max(512) })
    .nullable(),
  verifiedBadge: z.boolean(),
} as const;

export type SyncKey = keyof typeof SYNCED;
export type SyncValue<K extends SyncKey> = z.infer<(typeof SYNCED)[K]>;
export type SyncValues = { readonly [K in SyncKey]?: SyncValue<K> };

const isSyncKey = (key: string): key is SyncKey => Object.hasOwn(SYNCED, key);

export const SYNC_KEYS: readonly SyncKey[] =
  Object.keys(SYNCED).filter(isSyncKey);

export interface SyncEntry {
  readonly value: unknown;
  /** When it was changed (ms since 1970); 0: never changed here. */
  readonly at: number;
}

export interface SyncDoc {
  readonly values: Readonly<Record<string, SyncEntry>>;
}

export const EMPTY_DOC: SyncDoc = { values: {} };

/** A synced file is small; anything bigger isn't ours. */
export const MAX_DOC_BYTES = 64 * 1024;

const Entry = z.object({
  at: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
  value: z.unknown(),
});

/**
 * JSON text → document. Known options with a wrong value are dropped;
 * unknown ones are kept. Not our format (or not JSON): `null`.
 */
export const parseDoc = (text: string): SyncDoc | null => {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  const doc = z
    .object({
      format: z.literal(SYNC_FORMAT),
      values: z.record(z.string(), z.unknown()),
    })
    .safeParse(raw);
  if (!doc.success) {
    return null;
  }
  const values: Record<string, SyncEntry> = {};
  for (const [key, rawEntry] of Object.entries(doc.data.values)) {
    const entry = Entry.safeParse(rawEntry);
    if (!entry.success || key === "__proto__") {
      continue;
    }
    if (isSyncKey(key) && !SYNCED[key].safeParse(entry.data.value).success) {
      continue;
    }
    values[key] = { at: entry.data.at, value: entry.data.value };
  }
  return { values };
};

export const serializeDoc = (doc: SyncDoc): string =>
  JSON.stringify({ format: SYNC_FORMAT, values: doc.values, version: 1 });

/** Per option, the newest change wins; on a tie, `remote`. */
export const mergeDocs = (local: SyncDoc, remote: SyncDoc): SyncDoc => {
  const values: Record<string, SyncEntry> = { ...remote.values };
  for (const [key, entry] of Object.entries(local.values)) {
    const other = remote.values[key];
    if (other === undefined || entry.at > other.at) {
      values[key] = entry;
    }
  }
  return { values };
};

const sortDoc = (doc: SyncDoc): SyncDoc => ({
  values: Object.fromEntries(
    Object.entries(doc.values).toSorted(([a], [b]) => a.localeCompare(b))
  ),
});

export const sameDoc = (a: SyncDoc, b: SyncDoc): boolean =>
  serializeDoc(sortDoc(a)) === serializeDoc(sortDoc(b));

/** The known options of a document, with their checked values. */
export const valuesOf = (doc: SyncDoc): SyncValues => {
  const values: Record<string, unknown> = {};
  for (const key of SYNC_KEYS) {
    const entry = doc.values[key];
    const parsed =
      entry === undefined ? undefined : SYNCED[key].safeParse(entry.value);
    if (parsed?.success === true) {
      values[key] = parsed.data;
    }
  }
  return values;
};

/** The document with one option changed at `at`. */
export const withValue = <K extends SyncKey>(
  doc: SyncDoc,
  key: K,
  value: SyncValue<K>,
  at: number
): SyncDoc => ({ values: { ...doc.values, [key]: { at, value } } });
