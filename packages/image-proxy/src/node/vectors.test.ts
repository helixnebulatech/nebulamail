// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// security/ssrf-vectors.json against the proxy: for every URL
// vector that must be blocked, a CORRECTLY SIGNED ticket for that URL (as if
// the signing key had leaked) must still be refused, and no connection may
// be attempted. The fetch goes through the Node adapter and net-guard. The
// Workers and Vercel adapters run the same helper.

import { describe, expect, it } from "vitest";

import { createGuardedFetchForTests } from "../../../net-guard/src/node/testing";
import { APP_ORIGIN, keys } from "../test-fixtures";
import {
  BLOCKED_VECTORS,
  expectedForBlocked,
  runBlockedVector,
} from "../test-vectors";
import { createNodeProxyForTests } from "./testing";

describe("ssrf vectors through the proxy", () => {
  it("has blocked vectors to run", () => {
    expect(BLOCKED_VECTORS.length).toBeGreaterThan(20);
  });

  it.each(BLOCKED_VECTORS)("$id", async (vector) => {
    const k = await keys();
    const dialed: number[] = [];
    const guardedFetch = createGuardedFetchForTests({
      dial: (_address, port) => {
        dialed.push(port);
        return { host: "127.0.0.1", port: 1 };
      },
      resolver: async () => {
        await Promise.resolve();
        if (vector.dns === undefined) {
          throw new Error("ENOTFOUND");
        }
        return { A: vector.dns.A ?? [], AAAA: vector.dns.AAAA ?? [] };
      },
    });
    const proxy = createNodeProxyForTests({
      appOrigin: APP_ORIGIN,
      guardedFetch,
      keys: k,
      rateLimit: false,
    });
    const outcome = await runBlockedVector(
      k,
      vector,
      async (request) => await proxy.handle(request)
    );
    expect(outcome).toStrictEqual(expectedForBlocked(vector));
    expect(dialed).toStrictEqual([]);
  });
});
