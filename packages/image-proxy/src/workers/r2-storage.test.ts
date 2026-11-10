// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { png } from "../test-fixtures";
import { createR2Storage, isR2Bucket } from "./r2-storage";
import { createFakeR2 } from "./test-r2";

const DAY = 24 * 60 * 60 * 1000;
const KEY = `v1:${"a".repeat(64)}:${"b".repeat(64)}`;
const KEY_2 = `v1:${"c".repeat(64)}:${"d".repeat(64)}`;

describe(createR2Storage, () => {
  it("stores under img/ + the hashed key, with the type as metadata", async () => {
    const bucket = createFakeR2();
    const storage = createR2Storage(bucket);
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    expect([...bucket.objects.keys()]).toStrictEqual([`img/${KEY}`]);
    const stored = await storage.get(KEY);
    expect(stored).toMatchObject({ bytes: png(), type: "image/png" });
    // The proxy never lets browsers keep it longer (RFC 9111).
    expect(stored?.expiresAt).toBeTypeOf("number");
    await expect(storage.get(KEY_2)).resolves.toBeNull();
  });

  it("refuses keys that are not the core's hashed keys", async () => {
    const storage = createR2Storage(createFakeR2());
    await expect(
      storage.put(
        "https://evil.example/a.png",
        { bytes: png(), type: "image/png" },
        DAY
      )
    ).rejects.toThrow(TypeError);
    await expect(storage.get("../x")).rejects.toThrow(TypeError);
  });

  it("expires maxAgeMs after upload and deletes on read", async () => {
    let now = 0;
    const bucket = createFakeR2(() => now);
    const storage = createR2Storage(bucket, { maxAgeMs: DAY, now: () => now });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now = DAY - 1;
    await expect(storage.get(KEY)).resolves.not.toBeNull();
    now = DAY;
    await expect(storage.get(KEY)).resolves.toBeNull();
    expect(bucket.objects.size).toBe(0);
  });

  it("keeps entries as long as the admin says, also beyond 30 days", async () => {
    let now = 0;
    const bucket = createFakeR2(() => now);
    const storage = createR2Storage(bucket, {
      maxAgeMs: 365 * DAY,
      now: () => now,
    });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 365 * DAY);
    now = 364 * DAY;
    await expect(storage.get(KEY)).resolves.not.toBeNull();
    now = 365 * DAY;
    await expect(storage.get(KEY)).resolves.toBeNull();
  });

  it("defaults to 30 days and refuses a non-positive retention", async () => {
    let now = 0;
    const storage = createR2Storage(
      createFakeR2(() => now),
      {
        now: () => now,
      }
    );
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    now = 30 * DAY;
    await expect(storage.get(KEY)).resolves.toBeNull();
    expect(() => createR2Storage(createFakeR2(), { maxAgeMs: 0 })).toThrow(
      TypeError
    );
  });

  it("treats oversized or non-image objects as missing", async () => {
    const bucket = createFakeR2();
    const storage = createR2Storage(bucket, { maxBytes: 100 });
    await storage.put(KEY, { bytes: png(200), type: "image/png" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
    bucket.objects.set(`img/${KEY_2}`, {
      bytes: png(),
      type: "image/svg+xml",
      uploaded: new Date(),
    });
    await expect(storage.get(KEY_2)).resolves.toBeNull();
  });

  it("sweep deletes expired entries across pages", async () => {
    let now = 0;
    const bucket = createFakeR2(() => now, 2);
    const storage = createR2Storage(bucket, { maxAgeMs: DAY, now: () => now });
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
    expect([...bucket.objects.keys()]).toStrictEqual([`img/${KEY}`]);
  });
});

describe(isR2Bucket, () => {
  it("checks the binding shape", () => {
    expect(isR2Bucket(createFakeR2())).toBeTruthy();
    expect(isR2Bucket("bucket")).toBeFalsy();
    expect(isR2Bucket({ get: () => null })).toBeFalsy();
  });
});
