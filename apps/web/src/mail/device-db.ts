// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The one IndexedDB database of the mail UI on this device:
// draft copies (draft-backup.ts) and the identities for writing offline
// (identity-cache.ts). Deleted as a whole on logout (device.ts).

/** The name predates the identities; kept so existing copies stay. */
const DB_NAME = "nebula-mail-drafts";
const VERSION = 2;

export type StoreName = "backups" | "identities";

const STORES: readonly StoreName[] = ["backups", "identities"];

export const settle = async <T>(request: IDBRequest<T>): Promise<T> => {
  const { promise, reject, resolve } = Promise.withResolvers<T>();
  request.addEventListener("success", () => {
    resolve(request.result);
  });
  request.addEventListener("error", () => {
    reject(request.error ?? new Error("IndexedDB request failed"));
  });
  return await promise;
};

const openDb = async (): Promise<IDBDatabase> => {
  const request = globalThis.indexedDB.open(DB_NAME, VERSION);
  request.addEventListener("upgradeneeded", () => {
    // Version 1 had only "backups": create whatever is missing.
    for (const name of STORES) {
      if (!request.result.objectStoreNames.contains(name)) {
        request.result.createObjectStore(name, { keyPath: "id" });
      }
    }
  });
  return await settle(request);
};

export const withStore = async <T>(
  name: StoreName,
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>
): Promise<T> => {
  const db = await openDb();
  try {
    return await settle(run(db.transaction(name, mode).objectStore(name)));
  } finally {
    db.close();
  }
};

/** Everything in it, for every account (logout on this device). */
export const forgetDeviceDb = async (): Promise<void> => {
  await settle(globalThis.indexedDB.deleteDatabase(DB_NAME));
};
