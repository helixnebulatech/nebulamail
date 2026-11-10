// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The c-ares resolver against a fake DNS server on loopback, on Node, Deno
// and Bun: families are independent, an abort cancels at once and stops the
// retries, and hung lookups don't hold the libuv threadpool.
// Bun rounds the per-try timeout up to whole seconds, so the 200 ms cases
// take about 1 s there.

import { setTimeout as sleep } from "node:timers/promises";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { GuardError } from "../errors";
import { fakeDns } from "./fake-dns";
import type { FakeDns } from "./fake-dns";
import { createCancelableResolver } from "./resolver";

const quick = (server: FakeDns) =>
  createCancelableResolver({
    servers: [server.server],
    timeoutMs: 200,
    tries: 1,
  });

// Node, Deno and Bun: every pinned adapter uses it by default.
describe("cancelable resolver (c-ares)", () => {
  const servers: FakeDns[] = [];
  let both: FakeDns;
  let noAaaa: FakeDns;
  let slowAaaa: FakeDns;
  let silent: FakeDns;
  let broken: FakeDns;

  beforeAll(async () => {
    both = await fakeDns();
    noAaaa = await fakeDns({ AAAA: "servfail" });
    slowAaaa = await fakeDns({ AAAA: "silent" });
    silent = await fakeDns({ A: "silent", AAAA: "silent" });
    broken = await fakeDns({ A: "servfail", AAAA: "servfail" });
    servers.push(both, noAaaa, slowAaaa, silent, broken);
  });

  afterAll(() => {
    for (const server of servers) {
      server.close();
    }
  });

  it("asks for both families", async () => {
    await expect(quick(both)("example.com")).resolves.toStrictEqual({
      A: ["93.184.216.34"],
      AAAA: ["2606:2800:220:1:248:1893:25c8:1946"],
    });
  });

  it("uses one family when the other fails or times out", async () => {
    for (const server of [noAaaa, slowAaaa]) {
      // oxlint-disable-next-line no-await-in-loop
      await expect(quick(server)("example.com")).resolves.toStrictEqual({
        A: ["93.184.216.34"],
        AAAA: [],
      });
    }
  });

  it("fails when both families fail", async () => {
    await expect(quick(broken)("example.com")).rejects.toBeInstanceOf(
      GuardError
    );
  });

  it("an abort cancels the lookup at once", async () => {
    const resolver = createCancelableResolver({
      servers: [silent.server],
      timeoutMs: 5000,
      tries: 2,
    });
    const started = performance.now();
    const pending = resolver("example.com", AbortSignal.timeout(50));
    await expect(pending).rejects.toMatchObject({ reason: "timeout" });
    // Without the cancel it would take 2 × 5 s.
    expect(performance.now() - started).toBeLessThan(2000);
  });

  it("hung lookups leave the threadpool free", async () => {
    const resolver = createCancelableResolver({
      servers: [silent.server],
      timeoutMs: 5000,
      tries: 2,
    });
    const stop = new AbortController();
    const hung = Array.from(
      { length: 8 },
      async (_, n) =>
        // oxlint-disable-next-line promise/prefer-await-to-then
        await resolver(`host${String(n)}.example.com`, stop.signal).catch(
          (error: unknown) => error
        )
    );
    // WebCrypto runs in the libuv threadpool (4 threads): with 8 lookups
    // in `getaddrinfo` it would wait for them (seconds).
    const started = performance.now();
    await crypto.subtle.digest("SHA-256", new Uint8Array(1024 * 1024));
    expect(performance.now() - started).toBeLessThan(1500);
    stop.abort();
    await Promise.all(hung);
  });

  it("a signal already aborted throws timeout without sending a query", async () => {
    const server = await fakeDns();
    try {
      const pending = createCancelableResolver({ servers: [server.server] })(
        "example.com",
        AbortSignal.abort()
      );
      await expect(pending).rejects.toMatchObject({ reason: "timeout" });
      await sleep(100);
      expect(server.queries).toStrictEqual([]);
    } finally {
      server.close();
    }
  });

  it("an abort stops the retries", async () => {
    const server = await fakeDns({ A: "silent", AAAA: "silent" });
    try {
      const resolver = createCancelableResolver({
        servers: [server.server],
        timeoutMs: 1000,
        tries: 2,
      });
      await expect(
        resolver("example.com", AbortSignal.timeout(50))
      ).rejects.toMatchObject({ reason: "timeout" });
      // Past the first try: without `cancel()` the retries would go out (4).
      await sleep(1500);
      expect(server.queries).toHaveLength(2);
      expect(server.queries).toContainEqual({ name: "example.com", type: "A" });
      expect(server.queries).toContainEqual({
        name: "example.com",
        type: "AAAA",
      });
    } finally {
      server.close();
    }
  });
});
