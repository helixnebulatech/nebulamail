// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { mkdtemp, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { png } from "../test-fixtures";
import { createDiskStateStore } from "./disk-state";
import { createDiskStorage } from "./disk-storage";

const KEY = `v1:${"a".repeat(64)}:${"b".repeat(64)}`;
const STATE = `refs:${"c".repeat(64)}`;

describe(createDiskStateStore, () => {
  let dir = "";
  afterEach(async () => {
    await rm(dir, { force: true, recursive: true });
  });

  it("writes 0600 files named by hash; the image sweep and cap never touch them", async () => {
    dir = await mkdtemp(path.join(tmpdir(), "preload-state-"));
    let now = 0;
    const images = createDiskStorage(dir, { maxBytes: 1, now: () => now });
    const state = createDiskStateStore(path.join(dir, "preload-state"));
    await state.put(STATE, '{"v":1}');
    const file = path.join(
      dir,
      "preload-state",
      "refs",
      `${"c".repeat(64)}.json`
    );
    const info = await stat(file);
    // oxlint-disable-next-line no-bitwise -- file mode bits.
    expect(info.mode & 0o777).toBe(0o600);
    await images.put(KEY, { bytes: png(), type: "image/png" }, 1000);
    now = 10_000;
    await images.sweep();
    await expect(state.get(STATE)).resolves.toBe('{"v":1}');
    await expect(
      readdir(path.join(dir, "preload-state"))
    ).resolves.toStrictEqual(["refs"]);
    await state.delete(STATE);
    await expect(state.get(STATE)).resolves.toBeNull();
    await expect(state.put("refs:../../x", "{}")).rejects.toThrow(TypeError);
  });

  it("lists only well-formed keys of one kind", async () => {
    dir = await mkdtemp(path.join(tmpdir(), "preload-state-"));
    const root = path.join(dir, "preload-state");
    const state = createDiskStateStore(root);
    const collect = async (kind: "usage" | "refs") => {
      const keys: string[] = [];
      for await (const key of state.list(kind)) {
        keys.push(key);
      }
      return keys.toSorted();
    };
    await expect(collect("usage")).resolves.toStrictEqual([]);
    await state.put(`usage:${"a".repeat(64)}`, "{}");
    await state.put(`refs:${"b".repeat(64)}`, "{}");
    await state.put(`prefs:${"d".repeat(64)}`, "{}");
    // An interrupted write and a stray file are not keys.
    await writeFile(
      path.join(root, "usage", `${"e".repeat(64)}.json.1234.tmp`),
      "{}"
    );
    await writeFile(path.join(root, "usage", "notes.json"), "{}");
    await expect(collect("usage")).resolves.toStrictEqual([
      `usage:${"a".repeat(64)}`,
    ]);
    await expect(collect("refs")).resolves.toStrictEqual([
      `refs:${"b".repeat(64)}`,
    ]);
  });

  it("disk images can be deleted one by one", async () => {
    dir = await mkdtemp(path.join(tmpdir(), "preload-state-"));
    const images = createDiskStorage(dir);
    await images.put(KEY, { bytes: png(), type: "image/png" }, 60_000);
    await images.delete?.(KEY);
    await expect(images.get(KEY)).resolves.toBeNull();
  });
});
