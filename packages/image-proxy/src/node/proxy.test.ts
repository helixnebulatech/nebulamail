// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Node adapter wiring: the disk cap reaches the storage, and evicting
// shared (preloaded) entries is logged.

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import type { ProxyLogEvent } from "../config";
import { preloadKey } from "../preload/hook";
import { APP_ORIGIN, keys, png } from "../test-fixtures";
import { createNodeProxy } from "./index";

describe(createNodeProxy, () => {
  let dir = "";
  afterEach(async () => {
    await rm(dir, { force: true, recursive: true });
  });

  it("caps the disk cache with cacheMaxBytes and logs shared evictions", async () => {
    dir = await mkdtemp(path.join(tmpdir(), "node-proxy-"));
    const events: ProxyLogEvent[] = [];
    const proxy = createNodeProxy({
      appOrigin: APP_ORIGIN,
      cacheDir: dir,
      cacheMaxBytes: 250_000,
      keys: await keys(),
      log: (event) => {
        events.push(event);
      },
    });
    const image = { bytes: png(100_000), type: "image/png" };
    for (const name of ["a", "b", "c"]) {
      // Sequential: "a" is the oldest.
      // oxlint-disable-next-line no-await-in-loop
      const key = await preloadKey(`https://images.example.com/${name}.png`);
      // oxlint-disable-next-line no-await-in-loop
      await proxy.storage?.put(key, image, 60_000);
    }
    await expect(
      proxy.storage?.get(await preloadKey("https://images.example.com/a.png"))
    ).resolves.toBeNull();
    expect(events).toContainEqual({ event: "shared-evicted", files: 1 });
  });
});
