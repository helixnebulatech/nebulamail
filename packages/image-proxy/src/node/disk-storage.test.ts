// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createHash } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  stat,
  utimes,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { gif, png } from "../test-fixtures";
import { createDiskStorage } from "./disk-storage";

const KEY = `v1:${"a".repeat(64)}:${"b".repeat(64)}`;
const OTHER = `v1:${"c".repeat(64)}:${"d".repeat(64)}`;

/** Key `i` of an account, as the core builds it. */
const accountKey = (account: string, i: number) =>
  `v1:${createHash("sha256").update(account).digest("hex")}:${String(i % 10).repeat(63)}${String(Math.floor(i / 10))}`;

let dir = "";

const fileOf = (key: string): string => {
  const name = createHash("sha256").update(key).digest("hex");
  return path.join(dir, name.slice(0, 2), `${name}.img`);
};

/** The shard folders (the directory also holds `owner.key`). */
const shards = async (): Promise<string[]> => {
  const names = await readdir(dir);
  return names.filter((name) => /^[0-9a-f]{2}$/u.test(name));
};

const files = async (): Promise<string[]> => {
  const out: string[] = [];
  for (const shard of await shards()) {
    // oxlint-disable-next-line no-await-in-loop
    for (const name of await readdir(path.join(dir, shard))) {
      out.push(path.join(shard, name));
    }
  }
  return out;
};

/** Permission bits as an octal string, e.g. "600". */
const permissions = async (file: string): Promise<string> => {
  const info = await stat(file);
  return info.mode.toString(8).slice(-3);
};

describe(createDiskStorage, () => {
  beforeEach(async () => {
    dir = await mkdtemp(path.join(tmpdir(), "image-proxy-"));
  });
  afterEach(async () => {
    await rm(dir, { force: true, recursive: true });
  });

  it("stores and reads back", async () => {
    const storage = createDiskStorage(dir);
    await expect(storage.get(KEY)).resolves.toBeNull();
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 60_000);
    const stored = await storage.get(KEY);
    expect(stored).toMatchObject({ bytes: png(), type: "image/png" });
    // The proxy never lets browsers keep it longer (RFC 9111).
    expect(stored?.expiresAt).toBeTypeOf("number");
  });

  it("names files by a hash, never by the key or a URL; private permissions", async () => {
    const storage = createDiskStorage(dir);
    await storage.put(
      "https://images.example.com/a.png",
      { bytes: png(), type: "image/png" },
      60_000
    );
    const [file] = await files();
    expect(file).toMatch(/^[0-9a-f]{2}\/[0-9a-f]{64}\.img$/u);
    expect(file).not.toContain("example");
    const raw = await readFile(path.join(dir, file ?? ""));
    expect(raw.toString("latin1")).not.toContain("example");
    await expect(permissions(path.join(dir, file ?? ""))).resolves.toBe("600");
    await expect(
      permissions(path.join(dir, (file ?? "").slice(0, 2)))
    ).resolves.toBe("700");
  });

  it("writes atomically: overwrite leaves no temp files, readers see whole files", async () => {
    const storage = createDiskStorage(dir);
    await storage.put(KEY, { bytes: png(1000), type: "image/png" }, 60_000);
    const writes = Array.from({ length: 20 }, async (_, i) => {
      await storage.put(
        KEY,
        i % 2 === 0
          ? { bytes: gif(500), type: "image/gif" }
          : { bytes: png(1000), type: "image/png" },
        60_000
      );
    });
    const reads = Array.from(
      { length: 20 },
      async () => await storage.get(KEY)
    );
    const [, results] = await Promise.all([
      Promise.all(writes),
      Promise.all(reads),
    ]);
    for (const result of results) {
      expect(
        result === null ? [] : [result.type, result.bytes.length]
      ).toSatisfy(
        (v: unknown[]) =>
          JSON.stringify(v) === '["image/gif",500]' ||
          JSON.stringify(v) === '["image/png",1000]'
      );
    }
    const leftovers = await files();
    expect(leftovers.filter((f) => f.endsWith(".tmp"))).toStrictEqual([]);
  });

  it("expires by TTL (header, not mtime)", async () => {
    let t = 1000;
    const storage = createDiskStorage(dir, { now: () => t });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 500);
    t = 1499;
    await expect(storage.get(KEY)).resolves.not.toBeNull();
    t = 1500;
    await expect(storage.get(KEY)).resolves.toBeNull();
    await expect(files()).resolves.toStrictEqual([]);
  });

  it("sweep removes expired entries, stale temp files and garbage", async () => {
    let t = Date.now();
    const storage = createDiskStorage(dir, { now: () => t });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 1000);
    await storage.put(OTHER, { bytes: png(), type: "image/png" }, 10_000_000);
    const [shard] = await shards();
    const temp = path.join(
      dir,
      shard ?? "",
      `${"e".repeat(64)}.${"0".repeat(8)}-0000-0000-0000-${"0".repeat(12)}.tmp`
    );
    await writeFile(temp, "partial");
    const old = new Date(t - 2 * 60 * 60 * 1000);
    await utimes(temp, old, old);
    t += 2000;
    await expect(storage.sweep()).resolves.toBe(2);
    await expect(storage.get(OTHER)).resolves.not.toBeNull();
  });

  it("refuses corrupt or non-image entries", async () => {
    const storage = createDiskStorage(dir);
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 60_000);
    const [file] = await files();
    await writeFile(
      path.join(dir, file ?? ""),
      'WMIMG1\n\u0000\u001A{"expiresAt":1e20,"type":"image/svg+xml"}<svg/>'
    );
    await expect(storage.get(KEY)).resolves.toBeNull();
  });

  it("size cap evicts the oldest entries first", async () => {
    const storage = createDiskStorage(dir, { maxBytes: 3000 });
    const keys = Array.from(
      { length: 4 },
      (_, i) => `v1:${String(i).repeat(64)}:${"f".repeat(64)}`
    );
    for (const [i, key] of keys.entries()) {
      // Sequential: each entry gets an older mtime than the next one.
      // oxlint-disable-next-line no-await-in-loop
      await storage.put(key, { bytes: png(1000), type: "image/png" }, 60_000);
      const when = new Date(1_700_000_000_000 + i * 1000);
      // oxlint-disable-next-line no-await-in-loop
      await utimes(fileOf(key), when, when);
    }
    // ~1050 bytes each: 4 exceed 3000, so the oldest go until ≤ 2700.
    await expect(storage.usage()).resolves.toBeLessThanOrEqual(2700);
    await expect(storage.get(keys[0] ?? "")).resolves.toBeNull();
    await expect(storage.get(keys[1] ?? "")).resolves.toBeNull();
    await expect(storage.get(keys[2] ?? "")).resolves.not.toBeNull();
    await expect(storage.get(keys[3] ?? "")).resolves.not.toBeNull();
  });

  describe("fair eviction", () => {
    const image = { bytes: png(100_000), type: "image/png" };

    it("evicts the account that uses the most; others and shared entries stay", async () => {
      const shared: number[] = [];
      const storage = createDiskStorage(dir, {
        maxBytes: 1_000_000,
        onSharedEvicted: (n) => {
          shared.push(n);
        },
      });
      await storage.put(accountKey("bob", 0), image, 60_000);
      await storage.put(accountKey("*preload", 0), image, 60_000);
      await storage.put(accountKey("*mail", 0), image, 60_000);
      for (let i = 0; i < 20; i += 1) {
        // oxlint-disable-next-line no-await-in-loop
        await storage.put(accountKey("alice", i), image, 60_000);
      }
      const kept = await Promise.all(
        Array.from(
          { length: 20 },
          async (_, i) => (await storage.get(accountKey("alice", i))) !== null
        )
      );
      expect({
        // About 900 KB left under the cap: the 3 others plus 5 or 6 of hers.
        alice: kept.filter(Boolean).length <= 6,
        bob: (await storage.get(accountKey("bob", 0))) !== null,
        mail: (await storage.get(accountKey("*mail", 0))) !== null,
        newest: kept.at(-1),
        preload: (await storage.get(accountKey("*preload", 0))) !== null,
      }).toStrictEqual({
        alice: true,
        bob: true,
        mail: true,
        newest: true,
        preload: true,
      });
      expect(shared).toStrictEqual([]);
    });

    it("shared entries go last, oldest first, and are reported", async () => {
      const shared: number[] = [];
      const storage = createDiskStorage(dir, {
        maxBytes: 350_000,
        onSharedEvicted: (n) => {
          shared.push(n);
        },
      });
      for (let i = 0; i < 5; i += 1) {
        // oxlint-disable-next-line no-await-in-loop
        await storage.put(accountKey("*preload", i), image, 60_000);
      }
      const kept = await Promise.all(
        Array.from(
          { length: 5 },
          async (_, i) =>
            (await storage.get(accountKey("*preload", i))) !== null
        )
      );
      expect(kept).toStrictEqual([false, false, true, true, true]);
      expect(shared.reduce((a, b) => a + b, 0)).toBe(2);
    });

    it("owners are keyed per directory; a restart rebuilds the index from the headers", async () => {
      const first = createDiskStorage(dir);
      await first.put(accountKey("alice", 0), image, 60_000);
      await first.put(accountKey("*preload", 0), image, 60_000);
      const file = await readFile(fileOf(accountKey("alice", 0)));
      const header = file.subarray(9, 9 + file.readUInt16BE(7)).toString();
      const aliceHash = createHash("sha256").update("alice").digest("hex");
      expect(header).not.toContain(aliceHash.slice(0, 16));
      expect(header).toMatch(/"owner":"[0-9a-f]{16}"/u);
      await expect(permissions(path.join(dir, "owner.key"))).resolves.toBe(
        "600"
      );
      // A new process with a cap: alice's files go, the shared one stays.
      const second = createDiskStorage(dir, { maxBytes: 250_000 });
      await second.put(accountKey("alice", 1), image, 60_000);
      await second.put(accountKey("alice", 2), image, 60_000);
      await expect(
        second.get(accountKey("*preload", 0))
      ).resolves.not.toBeNull();
      await expect(second.get(accountKey("alice", 0))).resolves.toBeNull();
    });

    it("reads files written before owners existed (one legacy account)", async () => {
      const storage = createDiskStorage(dir, { maxBytes: 10_000_000 });
      await storage.put(KEY, { bytes: png(), type: "image/png" }, 60_000);
      const header = Buffer.from(
        JSON.stringify({ expiresAt: Date.now() + 60_000, type: "image/png" })
      );
      const length = Buffer.alloc(2);
      length.writeUInt16BE(header.length);
      await mkdir(path.dirname(fileOf(OTHER)), { recursive: true });
      await writeFile(
        fileOf(OTHER),
        Buffer.concat([Buffer.from("WMIMG1\n"), length, header, png()])
      );
      await expect(storage.get(OTHER)).resolves.toMatchObject({
        type: "image/png",
      });
      await expect(storage.usage()).resolves.toBeGreaterThan(0);
    });
  });
});
