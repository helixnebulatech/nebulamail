// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { EMPTY_DOC, serializeDoc, withValue } from "./model";

vi.mock(import("../mail/client"), () => ({
  getClient: vi.fn<() => Promise<never>>(
    async () => await Promise.reject(new Error("offline"))
  ),
  resetClient: vi.fn<() => void>(),
}));

const STORE_KEY = "wm-prefs-sync";

/** A localStorage that counts reads. */
const fakeStorage = () => {
  const items = new Map<string, string>();
  return {
    getItem: vi.fn<(key: string) => string | null>(
      (key) => items.get(key) ?? null
    ),
    items,
    removeItem: vi.fn<(key: string) => void>((key) => {
      items.delete(key);
    }),
    setItem: vi.fn<(key: string, value: string) => void>((key, value) => {
      items.set(key, value);
    }),
  };
};

const storedText = (lists: readonly string[]): string =>
  JSON.stringify({
    account: null,
    doc: serializeDoc(withValue(EMPTY_DOC, "taskLists", [...lists], 1)),
  });

const storageEvent = (key: string | null): Event =>
  Object.assign(new Event("storage"), { key });

let storage = fakeStorage();
let events = new EventTarget();

const load = async () => await import("./sync");

describe("syncedValue", () => {
  beforeEach(() => {
    vi.resetModules();
    storage = fakeStorage();
    events = new EventTarget();
    vi.stubGlobal("localStorage", storage);
    vi.stubGlobal("addEventListener", events.addEventListener.bind(events));
    vi.stubGlobal(
      "removeEventListener",
      events.removeEventListener.bind(events)
    );
    vi.stubGlobal("dispatchEvent", events.dispatchEvent.bind(events));
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("reads and checks the device copy once, and keeps the reference", async () => {
    storage.items.set(STORE_KEY, storedText(["a/1"]));
    const { syncedValue } = await load();
    const first = syncedValue("taskLists");
    expect(first).toStrictEqual(["a/1"]);
    expect(syncedValue("taskLists")).toBe(first);
    expect(syncedValue("theme")).toBeUndefined();
    expect(storage.getItem).toHaveBeenCalledOnce();
  });

  it("sees this tab's changes; unchanged options keep their object", async () => {
    vi.useFakeTimers();
    storage.items.set(STORE_KEY, storedText(["a/1"]));
    const { recordChange, syncedValue } = await load();
    const lists = syncedValue("taskLists");
    recordChange("removedTaskLists", ["a/2"]);
    expect(syncedValue("removedTaskLists")).toStrictEqual(["a/2"]);
    expect(syncedValue("taskLists")).toBe(lists);
    recordChange("taskLists", ["a/1", "a/3"]);
    expect(syncedValue("taskLists")).toStrictEqual(["a/1", "a/3"]);
    expect(storage.getItem).toHaveBeenCalledOnce();
  });

  it("reads again after another tab wrote it", async () => {
    storage.items.set(STORE_KEY, storedText(["a/1"]));
    const { subscribeSyncedValues, syncedValue } = await load();
    const changed = vi.fn<() => void>();
    const stop = subscribeSyncedValues(changed);
    expect(syncedValue("taskLists")).toStrictEqual(["a/1"]);
    storage.items.set(STORE_KEY, storedText(["b/2"]));
    globalThis.dispatchEvent(storageEvent("other-key"));
    expect(changed).not.toHaveBeenCalled();
    expect(syncedValue("taskLists")).toStrictEqual(["a/1"]);
    globalThis.dispatchEvent(storageEvent(STORE_KEY));
    expect(changed).toHaveBeenCalledOnce();
    expect(syncedValue("taskLists")).toStrictEqual(["b/2"]);
    stop();
  });

  it("forgets the copy on logout", async () => {
    storage.items.set(STORE_KEY, storedText(["a/1"]));
    const { forgetSyncedPreferences, syncedValue } = await load();
    expect(syncedValue("taskLists")).toStrictEqual(["a/1"]);
    forgetSyncedPreferences();
    expect(syncedValue("taskLists")).toBeUndefined();
  });
});
