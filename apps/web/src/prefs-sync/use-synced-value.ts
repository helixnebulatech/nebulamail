// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A synced option kept only in the synced copy (sync.ts), like the task
// lists: undefined until the user chooses it, on this or another device.

import { useSyncExternalStore } from "react";

import type { SyncKey, SyncValue } from "./model";
import { recordChange, subscribeSyncedValues, syncedValue } from "./sync";

const serverValue = (): undefined => undefined;

/** The value and a setter that saves it here and on the server. */
export const useSyncedValue = <K extends SyncKey>(
  key: K
): readonly [SyncValue<K> | undefined, (value: SyncValue<K>) => void] => {
  // `syncedValue` is checked (valuesOf) and keeps its reference until the
  // device copy changes, as a snapshot must (sync.ts caches it).
  const value = useSyncExternalStore(
    subscribeSyncedValues,
    () => syncedValue(key),
    serverValue
  );
  return [
    value,
    (next) => {
      recordChange(key, next);
    },
  ] as const;
};
