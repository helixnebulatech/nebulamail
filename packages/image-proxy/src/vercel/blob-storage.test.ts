// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { getRecord, putRecord } from "../preload/records";
import { png, text } from "../test-fixtures";
import { createBlobStateStore } from "./blob-state";
import { createBlobStorage } from "./blob-storage";
import { createFakeBlob } from "./test-blob";

const DAY = 24 * 60 * 60 * 1000;
const KEY = `v1:${"a".repeat(64)}:${"b".repeat(64)}`;

describe(createBlobStorage, () => {
  it("stores private blobs under hashed pathnames", async () => {
    const api = createFakeBlob();
    const storage = createBlobStorage(api);
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    expect([...api.blobs.keys()]).toStrictEqual([
      `img/v1/${"a".repeat(64)}/${"b".repeat(64)}`,
    ]);
    expect(
      api.blobs.get(`img/v1/${"a".repeat(64)}/${"b".repeat(64)}`)?.access
    ).toBe("private");
    const stored = await storage.get(KEY);
    expect(stored).toMatchObject({ bytes: png(), type: "image/png" });
    // The proxy never lets browsers keep it longer (RFC 9111).
    expect(stored?.expiresAt).toBeTypeOf("number");
  });

  it("refuses keys that are not the core's hashed keys", async () => {
    const storage = createBlobStorage(createFakeBlob());
    await expect(storage.get("https://evil.example/x")).rejects.toThrow(
      TypeError
    );
  });

  it("expires maxAgeMs after upload (any value, 30 days by default) and deletes on read", async () => {
    let now = 0;
    const api = createFakeBlob(() => now);
    const storage = createBlobStorage(api, {
      maxAgeMs: 90 * DAY,
      now: () => now,
    });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now = 90 * DAY - 1;
    await expect(storage.get(KEY)).resolves.not.toBeNull();
    now = 90 * DAY;
    await expect(storage.get(KEY)).resolves.toBeNull();
    expect(api.blobs.size).toBe(0);
    const byDefault = createBlobStorage(api, { now: () => now });
    await byDefault.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now += 30 * DAY;
    await expect(byDefault.get(KEY)).resolves.toBeNull();
  });

  it("treats oversized or non-image blobs as missing", async () => {
    const api = createFakeBlob();
    const storage = createBlobStorage(api, { maxBytes: 100 });
    await storage.put(KEY, { bytes: png(200), type: "image/png" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
    await storage.put(KEY, { bytes: png(), type: "text/html" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });

  it("keeps preload records (email tokens), still drops other types", async () => {
    const storage = createBlobStorage(createFakeBlob());
    const hashes = ["1".repeat(64), "2".repeat(64)];
    await putRecord(storage, "e".repeat(64), hashes, DAY);
    await expect(getRecord(storage, "e".repeat(64))).resolves.toStrictEqual(
      new Set(hashes)
    );
    await storage.put(KEY, { bytes: text("<b>x</b>"), type: "text/html" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });

  it("sweep deletes expired blobs across pages", async () => {
    let now = 0;
    const api = createFakeBlob(() => now, 2);
    const storage = createBlobStorage(api, { maxAgeMs: DAY, now: () => now });
    for (const c of ["1", "2", "3", "4", "5"]) {
      // oxlint-disable-next-line no-await-in-loop
      await storage.put(
        `v1:${c.repeat(64)}:${"e".repeat(64)}`,
        { bytes: png(), type: "image/png" },
        DAY
      );
    }
    now = DAY / 2;
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now = DAY;
    await expect(storage.sweep()).resolves.toBe(5);
    expect(api.blobs.size).toBe(1);
  });
});

describe(createBlobStateStore, () => {
  it("lists the state keys of one kind across pages", async () => {
    const api = createFakeBlob(Date.now, 2);
    const state = createBlobStateStore(api);
    for (const c of ["a", "b", "c"]) {
      // oxlint-disable-next-line no-await-in-loop
      await state.put(`refs:${c.repeat(64)}`, "{}");
    }
    await state.put(`usage:${"d".repeat(64)}`, "{}");
    await api.put("pre/v1/refs/junk", new Blob([]), {
      access: "private",
      addRandomSuffix: false,
      allowOverwrite: true,
      contentType: "application/json",
    });
    const keys: string[] = [];
    for await (const key of state.list("refs")) {
      keys.push(key);
    }
    expect(keys).toStrictEqual(
      ["a", "b", "c"].map((c) => `refs:${c.repeat(64)}`)
    );
  });

  it("keeps private state under pre/v1/, never swept, and deletes images", async () => {
    let now = 0;
    const api = createFakeBlob(() => now);
    const state = createBlobStateStore(api);
    const key = `usage:${"f".repeat(64)}`;
    await state.put(key, '{"v":1}');
    expect(api.blobs.get(`pre/v1/usage/${"f".repeat(64)}`)?.access).toBe(
      "private"
    );
    await expect(state.get(key)).resolves.toBe('{"v":1}');
    const storage = createBlobStorage(api);
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now = 400 * DAY;
    await storage.sweep();
    await expect(state.get(key)).resolves.toBe('{"v":1}');
    await state.delete(key);
    await expect(state.get(key)).resolves.toBeNull();
    now = 0;
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    await storage.delete?.(KEY);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });
});
