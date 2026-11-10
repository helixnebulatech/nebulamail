// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Keeps this device's preferences and the account's synced copy in step.
// A change is saved here at once (with its time) and
// sent a moment later; opening the app, or coming back to the tab, merges
// the server's copy in: per option, the newest change wins. Without a
// place on the server (remote.ts) everything simply stays on the device.
//
// The device copy is in localStorage: these are display choices and the
// id of a paused Sieve script, nothing sensitive. It is
// forgotten on logout like the rest of the device data.

import { readLocalFlag } from "../lib/use-local-flag";
import { getClient } from "../mail/client";
import {
  EMPTY_DOC,
  SYNC_KEYS,
  mergeDocs,
  parseDoc,
  sameDoc,
  serializeDoc,
  valuesOf,
  withValue,
} from "./model";
import type { SyncDoc, SyncKey, SyncValue, SyncValues } from "./model";
import { backendFor, remoteFor } from "./remote";
import type { BackendKind } from "./remote";

const STORE_KEY = "wm-prefs-sync";
/** The user allowed the mail-folder fallback (remote.ts, option 2). */
export const MAIL_FALLBACK_FLAG = "wm-prefs-sync-mail";
const EVENT = "wm-prefs-sync";
const PUSH_DELAY_MS = 1500;

/** What this device has and how to change it (set by the app shell). */
export interface Device {
  /** Current values of the options this device keeps itself. */
  readonly read: () => SyncValues;
  /** Applies options that changed elsewhere. */
  readonly apply: (values: SyncValues, keys: readonly SyncKey[]) => void;
}

export type SyncStatus =
  | { readonly state: "idle" }
  | { readonly state: "syncing" }
  | { readonly state: "local" }
  | {
      readonly state: "synced";
      readonly backend: BackendKind;
      readonly at: number;
    }
  | { readonly state: "error"; readonly backend: BackendKind };

interface Stored {
  /** Server and user the copy belongs to; `null` before the first sync. */
  readonly account: string | null;
  readonly doc: SyncDoc;
}

let status: SyncStatus = { state: "idle" };
let device: Device | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let running: Promise<void> | null = null;
let again = false;

const notify = (): void => {
  globalThis.dispatchEvent(new Event(EVENT));
};

const setStatus = (next: SyncStatus): void => {
  status = next;
  notify();
};

const EMPTY_STORED: Stored = { account: null, doc: EMPTY_DOC };

const parseStored = (raw: string | null): Stored => {
  if (raw === null) {
    return EMPTY_STORED;
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return EMPTY_STORED;
    }
    const account =
      "account" in parsed && typeof parsed.account === "string"
        ? parsed.account
        : null;
    const doc =
      "doc" in parsed && typeof parsed.doc === "string"
        ? parseDoc(parsed.doc)
        : null;
    return { account, doc: doc ?? EMPTY_DOC };
  } catch {
    return EMPTY_STORED;
  }
};

/**
 * The device copy as last read or written, and its checked values: read
 * once, not on every `syncedValue` (each React render asks). This tab
 * writes it only through `writeStored`; another tab's write arrives as a
 * `storage` event, which drops it.
 */
let cache: { readonly stored: Stored; values: SyncValues | null } | null = null;
let listening = false;

/** Whether another tab's `storage` event touched the device copy. */
const otherTabWrote = (event: StorageEvent): boolean =>
  // `key` is null when another tab cleared the whole storage.
  event.key === STORE_KEY || event.key === null;

const onStorage = (event: StorageEvent): void => {
  if (otherTabWrote(event)) {
    cache = null;
  }
};

const listenToOtherTabs = (): void => {
  if (listening || typeof globalThis.addEventListener !== "function") {
    return;
  }
  listening = true;
  globalThis.addEventListener("storage", onStorage);
};

const readStored = (): Stored => {
  if (cache !== null) {
    return cache.stored;
  }
  let raw: string | null = null;
  try {
    raw = globalThis.localStorage.getItem(STORE_KEY);
  } catch {
    // No storage (server rendering, storage off): nothing to keep either.
    return EMPTY_STORED;
  }
  listenToOtherTabs();
  const stored = parseStored(raw);
  cache = { stored, values: null };
  return stored;
};

const writeStored = (stored: Stored): void => {
  // Also when storage is off (private mode): the change applies until reload.
  cache = { stored, values: null };
  try {
    globalThis.localStorage.setItem(
      STORE_KEY,
      JSON.stringify({ account: stored.account, doc: serializeDoc(stored.doc) })
    );
  } catch {
    // Storage off: only this page keeps it.
  }
  // useSyncedValue readers see the new values.
  notify();
};

/** Values read before the last change of the device copy. */
let lastValues: SyncValues = {};

/**
 * `next`, with the earlier object of every option that didn't change: a
 * change of one option doesn't make the others look new to React.
 */
const keepSame = (previous: SyncValues, next: SyncValues): SyncValues => {
  const kept: Record<string, unknown> = { ...next };
  for (const key of SYNC_KEYS) {
    const before = previous[key];
    if (
      before !== undefined &&
      typeof before === "object" &&
      JSON.stringify(before) === JSON.stringify(next[key])
    ) {
      kept[key] = before;
    }
  }
  // Same keys and checked values as `next`, some by their earlier object.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return kept;
};

/**
 * A synced option as this device last saw it. The same value (by
 * reference) until the device copy changes, as `useSyncExternalStore`
 * wants of a snapshot.
 */
export const syncedValue = <K extends SyncKey>(
  key: K
): SyncValue<K> | undefined => {
  const stored = readStored();
  if (cache === null || cache.stored !== stored) {
    return valuesOf(stored.doc)[key];
  }
  cache.values ??= keepSame(lastValues, valuesOf(stored.doc));
  lastValues = cache.values;
  return cache.values[key];
};

/** Merges the server's copy in and sends ours if it differs. */
const syncOnce = async (): Promise<void> => {
  const client = await getClient();
  const account = `${client.session.apiUrl.origin} ${client.session.username}`;
  const stored = readStored();
  // Another user's leftovers (no logout): start from this device's values.
  const local =
    stored.account === null || stored.account === account
      ? stored.doc
      : EMPTY_DOC;
  // Only what the user chose is synced: a device's defaults (a language
  // taken from its browser, say) never overwrite another device.
  const kind = backendFor(client, readLocalFlag(MAIL_FALLBACK_FLAG, false));
  if (kind === null) {
    writeStored({ account, doc: local });
    setStatus({ state: "local" });
    return;
  }
  const remote = remoteFor(client, kind);
  try {
    const saved = (await remote.read()) ?? EMPTY_DOC;
    const merged = mergeDocs(local, saved);
    // A change made while the server answered stays (it is newer).
    const now = readStored();
    const latest =
      now.account === stored.account ? mergeDocs(merged, now.doc) : merged;
    writeStored({ account, doc: latest });
    // Whatever this device shows differently from the newest values (also
    // what an earlier sync merged before the app could apply it).
    if (device !== null) {
      const shown = device.read();
      const wanted = valuesOf(latest);
      const keys = SYNC_KEYS.filter(
        (key) =>
          shown[key] !== undefined &&
          wanted[key] !== undefined &&
          JSON.stringify(shown[key]) !== JSON.stringify(wanted[key])
      );
      if (keys.length > 0) {
        device.apply(wanted, keys);
      }
    }
    if (!sameDoc(latest, saved)) {
      await remote.write(latest);
    }
    setStatus({ at: Date.now(), backend: kind, state: "synced" });
  } catch {
    setStatus({ backend: kind, state: "error" });
  }
};

/** Syncs now (one run at a time; a request during a run runs after it). */
export const syncPreferences = async (): Promise<void> => {
  if (running !== null) {
    again = true;
    await running;
    return;
  }
  setStatus({ state: "syncing" });
  running = (async () => {
    do {
      again = false;
      // oxlint-disable-next-line no-await-in-loop -- one run after another, on purpose.
      await syncOnce().catch(() => {
        setStatus({ state: "local" });
      });
    } while (again);
  })();
  try {
    await running;
  } finally {
    running = null;
  }
};

let first: Promise<void> | null = null;

/**
 * The first sync of this page load (started if nothing started it): for
 * readers of a synced value that may have been set on another device.
 */
export const syncedOnce = async (): Promise<void> => {
  first ??= syncPreferences();
  await first;
};

/** Saves a change on this device now and on the server shortly after. */
export const recordChange = <K extends SyncKey>(
  key: K,
  value: SyncValue<K>
): void => {
  const stored = readStored();
  writeStored({
    ...stored,
    doc: withValue(stored.doc, key, value, Date.now()),
  });
  if (timer !== null) {
    clearTimeout(timer);
  }
  timer = setTimeout(() => {
    timer = null;
    void syncPreferences();
  }, PUSH_DELAY_MS);
};

export const setSyncDevice = (next: Device | null): void => {
  device = next;
};

export const syncStatus = (): SyncStatus => status;

/** Calls `onChange` when a synced value (or the status) may have changed. */
export const subscribeSyncStatus = (onChange: () => void): (() => void) => {
  globalThis.addEventListener(EVENT, onChange);
  return () => {
    globalThis.removeEventListener(EVENT, onChange);
  };
};

/**
 * Calls `onChange` when a synced value may have changed, here or in
 * another tab (the cache is dropped first, whatever listener runs first).
 */
export const subscribeSyncedValues = (onChange: () => void): (() => void) => {
  const fromOtherTab = (event: StorageEvent): void => {
    if (otherTabWrote(event)) {
      cache = null;
      onChange();
    }
  };
  const unsubscribe = subscribeSyncStatus(onChange);
  globalThis.addEventListener("storage", fromOtherTab);
  return () => {
    unsubscribe();
    globalThis.removeEventListener("storage", fromOtherTab);
  };
};

/** Logout: forget the device copy (the server's copy stays). */
export const forgetSyncedPreferences = (): void => {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
  cache = null;
  lastValues = {};
  try {
    globalThis.localStorage.removeItem(STORE_KEY);
  } catch {
    // Nothing stored.
  }
  status = { state: "idle" };
};
