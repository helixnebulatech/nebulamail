// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Core limits added 2026-10-26: global rate limits (also async ones), the
// per-email request budget, process-wide download slots, the tracker check
// on redirect hops, and retention beyond 30 days.

import { setTimeout as sleep } from "node:timers/promises";

import { createConcurrencyLimiter } from "@nebulamail/net-guard";
import { describe, expect, it } from "vitest";
import { z } from "zod";

import type { FetchImage, ProxyConfig, ProxyLogEvent } from "./index";
import {
  createMemoryStorage,
  createProxyHandler,
  DEFAULT_LIMITS,
} from "./index";
import { APP_ORIGIN, keys, NOW, png, ticket } from "./test-fixtures";

const BASE = "https://img.example.com";
const URL_A = "https://images.example.com/a.png";
const TRACKER = "https://www.google-analytics.com/collect?v=1";
const DAY = 24 * 60 * 60 * 1000;

const Images = z.object({
  images: z.record(z.string(), z.record(z.string(), z.unknown())),
});

const post = (tickets: string[]) =>
  new Request(`${BASE}/batch`, {
    body: JSON.stringify({ tickets }),
    headers: { "content-type": "application/json", origin: APP_ORIGIN },
    method: "POST",
  });

const ok: FetchImage = async () =>
  await Promise.resolve({ bytes: png(), type: "image/png" });

const setup = async (overrides: Partial<ProxyConfig> = {}) => {
  const k = await keys();
  const handler = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage: ok,
    keys: k,
    now: () => NOW,
    ...overrides,
  });
  return { handler, k };
};

const statusOf = async (pending: Promise<Response>): Promise<number> => {
  const response = await pending;
  return response.status;
};

const imagesOf = async (response: Response) =>
  Images.parse(await response.json()).images;

describe("global and async rate limits", () => {
  it("checks batch:global and image:global for everybody", async () => {
    const seen: string[] = [];
    const storage = createMemoryStorage({ now: () => NOW });
    const { handler, k } = await setup({
      rateLimit: (key) => {
        seen.push(key);
        return !key.endsWith(":global");
      },
      storage,
    });
    const t = await ticket(k);
    await expect(statusOf(handler(post([t])))).resolves.toBe(429);
    await expect(
      statusOf(handler(new Request(`${BASE}/i/${t}`)))
    ).resolves.toBe(429);
    expect(seen).toStrictEqual([
      "batch:account:acc1",
      "batch:global",
      "image:global",
    ]);
  });

  it("awaits an async limiter; a throwing one allows", async () => {
    const { handler, k } = await setup({
      rateLimit: async (key) => {
        await sleep(1);
        return key !== "batch:account:acc1";
      },
    });
    await expect(statusOf(handler(post([await ticket(k)])))).resolves.toBe(429);
    const failing = await setup({
      rateLimit: async () => await Promise.reject(new Error("limiter down")),
    });
    await expect(
      statusOf(failing.handler(post([await ticket(failing.k)])))
    ).resolves.toBe(200);
  });
});

describe("per-email budget and redirects", () => {
  it("passes the budget and a tracker check for redirect hops to the fetcher", async () => {
    const events: ProxyLogEvent[] = [];
    const budgets: number[] = [];
    const { handler, k } = await setup({
      fetchImage: async (_url, _signal, options) => {
        budgets.push(options?.budget.remainingRequests() ?? -1);
        // Pretend the server redirected to a tracker.
        const allowed = options?.allowRedirect(TRACKER) ?? true;
        // A normal redirect target passes.
        budgets.push(options?.allowRedirect(URL_A) === true ? 1 : 0);
        return await Promise.resolve(
          allowed
            ? { bytes: png(), type: "image/png" }
            : { error: "refused" as const }
        );
      },
      log: (event) => {
        events.push(event);
      },
    });
    const t = await ticket(k, { url: URL_A });
    const images = await imagesOf(await handler(post([t])));
    expect(images[t]).toStrictEqual({ error: "tracker" });
    expect(budgets).toStrictEqual([DEFAULT_LIMITS.maxRequestsPerEmail, 1]);
    expect(events).toContainEqual({ event: "tracker-redirect" });
  });

  it("an exhausted request budget makes the rest too-large", async () => {
    const { handler, k } = await setup({
      fetchImage: async (_url, _signal, options) => {
        // Each image takes two requests (a redirect and the image).
        const first = options?.budget.takeRequest() ?? false;
        const second = options?.budget.takeRequest() ?? false;
        return await Promise.resolve(
          first && second
            ? { bytes: png(), type: "image/png" }
            : { error: "budget" as const }
        );
      },
      limits: { concurrency: 1, maxRequestsPerEmail: 3 },
    });
    const tickets = await Promise.all(
      ["a", "b"].map(
        async (n) =>
          await ticket(k, { url: `https://images.example.com/${n}.png` })
      )
    );
    const images = await imagesOf(await handler(post(tickets)));
    expect(tickets.map((t) => images[t]?.error ?? "ok")).toStrictEqual([
      "ok",
      "too-large",
    ]);
  });
});

describe("process-wide download slots", () => {
  it("bounds downloads across concurrent requests", async () => {
    let active = 0;
    let peak = 0;
    const { handler, k } = await setup({
      fetchImage: async () => {
        active += 1;
        peak = Math.max(peak, active);
        await sleep(5);
        active -= 1;
        return { bytes: png(), type: "image/png" };
      },
      limits: { globalConcurrency: 2 },
    });
    const batches = await Promise.all(
      [0, 1, 2].map(
        async (b) =>
          await Promise.all(
            [0, 1, 2].map(
              async (i) =>
                await ticket(k, {
                  emailId: `mail${String(b)}`,
                  url: `https://images.example.com/${String(b)}-${String(i)}.png`,
                })
            )
          )
      )
    );
    const responses = await Promise.all(
      batches.map(async (tickets) => await handler(post(tickets)))
    );
    expect(responses.map((r) => r.status)).toStrictEqual([200, 200, 200]);
    expect(peak).toBe(2);
  });

  it("a full queue answers timeout instead of waiting forever", async () => {
    const slots = createConcurrencyLimiter({ max: 1, maxQueue: 0 });
    const gate = Promise.withResolvers<undefined>();
    const busy = slots.run(async () => {
      await gate.promise;
    });
    const { handler, k } = await setup({ downloadSlots: slots });
    const t = await ticket(k);
    const images = await imagesOf(await handler(post([t])));
    expect(images[t]).toStrictEqual({ error: "timeout" });
    gate.resolve(undefined);
    await busy;
  });
});

describe("retention", () => {
  it("accepts any retention, also beyond 30 days", async () => {
    let now = NOW;
    const storage = createMemoryStorage({ now: () => now });
    const k = await keys();
    const handler = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: ok,
      keys: k,
      limits: { cacheTtlMs: 400 * DAY },
      now: () => now,
      storage,
    });
    const t = await ticket(k, { url: URL_A });
    await handler(post([t]));
    now += 399 * DAY;
    const later = await ticket(k, { url: URL_A });
    // The new ticket is signed at NOW; give it its own clock window.
    const fresh = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: ok,
      keys: k,
      limits: { cacheTtlMs: 400 * DAY },
      now: () => NOW,
      storage,
    });
    await expect(
      statusOf(fresh(new Request(`${BASE}/i/${later}`)))
    ).resolves.toBe(200);
    now += DAY;
    await expect(
      statusOf(fresh(new Request(`${BASE}/i/${later}`)))
    ).resolves.toBe(404);
    expect(() =>
      createProxyHandler({
        appOrigin: APP_ORIGIN,
        fetchImage: ok,
        keys: k,
        limits: { cacheTtlMs: 0 },
      })
    ).toThrow(/positive/u);
  });
});
