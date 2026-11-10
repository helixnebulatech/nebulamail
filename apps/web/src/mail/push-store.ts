// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the tabs and the service worker share for new-mail notifications
//in its own IndexedDB database. The worker (pwa/sw.ts)
// can't import this file: it has its own copy of the same few lines, so
// the names and shapes below must stay in sync with it.
//
// - "kv" { id, value }:
//   - "on": the "New mail notifications" setting of this device (the
//     worker can't read localStorage). Off unless the user turned it on.
//   - "state <account>": the last Email state seen for that account, so
//     the worker can ask `Email/changes` what is new when a push arrives.
//   - "email push": { account, accountId, inboxId } or null, whose mail
//     the server puts inside the push (email-push.ts) and its inbox, so
//     the worker can show an `EmailPush` with no request.
// - "notified" { id: "<account> <emailId>", at }: emails already
//   announced. Claimed in one read-write transaction, so a hidden tab and
//   the worker never both show the same email; pruned after two days.
//
// <account> = "<API origin> <username> <accountId>": ids like "c" repeat
// across servers and users. Nothing else (no subjects, no senders) is
// stored. Deleted on logout (device.ts).

export const PUSH_DB = "nebula-mail-push";
const VERSION = 1;
const KEEP_MS = 2 * 24 * 60 * 60_000;

const settle = async <T>(request: IDBRequest<T>): Promise<T> => {
  const { promise, reject, resolve } = Promise.withResolvers<T>();
  request.addEventListener("success", () => {
    resolve(request.result);
  });
  request.addEventListener("error", () => {
    reject(request.error ?? new Error("IndexedDB request failed"));
  });
  return await promise;
};

const done = async (transaction: IDBTransaction): Promise<void> => {
  const { promise, reject, resolve } = Promise.withResolvers<undefined>();
  transaction.addEventListener("complete", () => {
    resolve(undefined);
  });
  transaction.addEventListener("abort", () => {
    reject(transaction.error ?? new Error("IndexedDB transaction aborted"));
  });
  await promise;
};

const open = async (): Promise<IDBDatabase> => {
  const request = globalThis.indexedDB.open(PUSH_DB, VERSION);
  request.addEventListener("upgradeneeded", () => {
    for (const name of ["kv", "notified"]) {
      if (!request.result.objectStoreNames.contains(name)) {
        request.result.createObjectStore(name, { keyPath: "id" });
      }
    }
  });
  return await settle(request);
};

const withDb = async <T>(run: (db: IDBDatabase) => Promise<T>): Promise<T> => {
  const db = await open();
  try {
    return await run(db);
  } finally {
    db.close();
  }
};

/** The device's account key for an account of a session. */
export const pushAccountKey = (
  apiOrigin: string,
  username: string,
  accountId: string
): string => `${apiOrigin} ${username} ${accountId}`;

/** Tells the worker whether this device wants new-mail notifications. */
export const setMailPushOn = async (on: boolean): Promise<void> => {
  await withDb(async (db) => {
    const transaction = db.transaction("kv", "readwrite");
    transaction.objectStore("kv").put({ id: "on", value: on });
    await done(transaction);
  });
};

export const mailPushOn = async (): Promise<boolean> =>
  await withDb(async (db) => {
    const found: unknown = await settle(
      db.transaction("kv").objectStore("kv").get("on")
    );
    return (
      typeof found === "object" &&
      found !== null &&
      Reflect.get(found, "value") === true
    );
  });

/** The stored Email state of an account, if any. */
export const mailState = async (account: string): Promise<string | null> =>
  await withDb(async (db) => {
    const found: unknown = await settle(
      db.transaction("kv").objectStore("kv").get(`state ${account}`)
    );
    const value: unknown =
      typeof found === "object" && found !== null
        ? Reflect.get(found, "value")
        : null;
    return typeof value === "string" ? value : null;
  });

/**
 * Where the worker starts from: the state the tab saw, unless one is
 * already stored (the worker may be further on).
 */
export const seedMailState = async (
  account: string,
  state: string
): Promise<void> => {
  await withDb(async (db) => {
    const transaction = db.transaction("kv", "readwrite");
    const store = transaction.objectStore("kv");
    const id = `state ${account}`;
    const found: unknown = await settle(store.get(id));
    if (found === undefined) {
      store.put({ id, value: state });
    }
    await done(transaction);
  });
};

/** Whose mail comes inside the push, for the worker (email-push.ts). */
export interface EmailPushTarget {
  readonly account: string;
  readonly accountId: string;
  readonly inboxId: string;
}

export const setEmailPushTarget = async (
  target: EmailPushTarget | null
): Promise<void> => {
  await withDb(async (db) => {
    const transaction = db.transaction("kv", "readwrite");
    transaction.objectStore("kv").put({ id: "email push", value: target });
    await done(transaction);
  });
};

/**
 * Claims an email for a notification: true the first time on this device,
 * false if the worker or another tab already showed it.
 */
export const claimNotice = async (
  account: string,
  emailId: string,
  now: number = Date.now()
): Promise<boolean> =>
  await withDb(async (db) => {
    const transaction = db.transaction("notified", "readwrite");
    const store = transaction.objectStore("notified");
    const id = `${account} ${emailId}`;
    const found: unknown = await settle(store.get(id));
    if (found === undefined) {
      store.put({ at: now, id });
    }
    await done(transaction);
    return found === undefined;
  });

/** Forgets announced emails older than two days. */
export const pruneNotices = async (now: number = Date.now()): Promise<void> => {
  await withDb(async (db) => {
    const transaction = db.transaction("notified", "readwrite");
    const store = transaction.objectStore("notified");
    const all: unknown = await settle(store.getAll());
    for (const item of Array.isArray(all) ? all : []) {
      const at: unknown =
        typeof item === "object" && item !== null
          ? Reflect.get(item, "at")
          : null;
      const id: unknown =
        typeof item === "object" && item !== null
          ? Reflect.get(item, "id")
          : null;
      if (
        typeof id === "string" &&
        (typeof at !== "number" || now - at > KEEP_MS)
      ) {
        store.delete(id);
      }
    }
    await done(transaction);
  });
};

/** Logout: the worker stops announcing and forgets every state. */
export const forgetPushStore = async (): Promise<void> => {
  await settle(globalThis.indexedDB.deleteDatabase(PUSH_DB));
};
