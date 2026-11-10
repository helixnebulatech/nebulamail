// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { once } from "node:events";
import { setTimeout as sleep } from "node:timers/promises";

import { describe, expect, it, vi } from "vitest";
import { z } from "zod";

import type {
  FetchImage,
  ImageStorage,
  ProxyConfig,
  ProxyLogEvent,
  StoredImage,
} from "./index";
import {
  createMemoryStorage,
  createProxyHandler,
  createRateLimiter,
  DEFAULT_LIMITS,
  IMAGE_CSP,
} from "./index";
import {
  APP_ORIGIN,
  gif,
  KEY_1,
  keys,
  NOW,
  png,
  text,
  ticket,
} from "./test-fixtures";

const BASE = "https://img.example.com";
const URL_A = "https://images.example.com/a.png";
const URL_B = "https://images.example.com/b.gif";
const TRACKER = "https://www.google-analytics.com/collect?v=1";

type Responses = Record<string, Awaited<ReturnType<FetchImage>>>;
type Handler = ReturnType<typeof createProxyHandler>;

const DEFAULT_RESPONSES: Responses = {
  [URL_A]: { bytes: png(), type: "image/png" },
  [URL_B]: { bytes: gif(), type: "image/gif" },
};

const Json = z.record(z.string(), z.unknown());
const Images = z.object({
  images: z.record(z.string(), z.record(z.string(), z.unknown())),
});

/** Fake fetcher: answers from a table and records calls. */
const fakeFetcher = (responses: Responses) => {
  const calls: string[] = [];
  const fetchImage: FetchImage = async (url) => {
    calls.push(url);
    await Promise.resolve();
    return responses[url] ?? { error: "dns" };
  };
  return { calls, fetchImage };
};

const setup = async (
  overrides: Partial<ProxyConfig> = {},
  responses: Responses = DEFAULT_RESPONSES
) => {
  const k = await keys();
  const fetcher = fakeFetcher(responses);
  const handler = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage: fetcher.fetchImage,
    keys: k,
    now: () => NOW,
    ...overrides,
  });
  return { calls: fetcher.calls, handler, k };
};

const cacheSetup = async (
  overrides: Partial<ProxyConfig> = {},
  responses?: Responses
) => {
  const storage = createMemoryStorage({ now: () => NOW });
  return { storage, ...(await setup({ storage, ...overrides }, responses)) };
};

const post = (body: unknown, headers: Record<string, string> = {}) =>
  new Request(`${BASE}/batch`, {
    body: typeof body === "string" ? body : JSON.stringify(body),
    headers: {
      "content-type": "application/json",
      origin: APP_ORIGIN,
      ...headers,
    },
    method: "POST",
  });

const get = (token: string) => new Request(`${BASE}/i/${token}`);

const preflight = (headers: Record<string, string>) =>
  new Request(`${BASE}/batch`, {
    headers: { origin: APP_ORIGIN, ...headers },
    method: "OPTIONS",
  });

const statusOf = async (pending: Promise<Response>): Promise<number> => {
  const response = await pending;
  return response.status;
};

const bodyOf = async (pending: Promise<Response>) => {
  const response = await pending;
  return Json.parse(await response.json());
};

const imagesOf = async (pending: Promise<Response>) => {
  const response = await pending;
  const body = Images.parse(await response.json());
  return body.images;
};

/** The result for one ticket of a batch request. */
const resultOf = async (
  handler: Handler,
  tickets: string[],
  token: string,
  mode?: "batch" | "cache"
) => {
  const images = await imagesOf(
    handler(post(mode === undefined ? { tickets } : { mode, tickets }))
  );
  return images[token];
};

const imageUrlFor = (name: string) => `https://images.example.com/${name}.png`;

const decode = (data: unknown): Uint8Array =>
  Uint8Array.from(atob(String(data)), (c) => c.codePointAt(0) ?? 0);

const manyTickets = async (
  handler: { k: Awaited<ReturnType<typeof keys>> },
  count: number
) =>
  await Promise.all(
    Array.from(
      { length: count },
      async (_, i) =>
        await ticket(handler.k, { url: `https://images.example.com/${i}.png` })
    )
  );

// In order, so the buckets drain one request at a time.
const inOrder = async (
  count: number,
  send: (i: number) => Promise<number>
): Promise<number[]> => {
  const statuses: number[] = [];
  for (let i = 0; i < count; i += 1) {
    // oxlint-disable-next-line no-await-in-loop
    const status = await send(i);
    statuses.push(status);
  }
  return statuses;
};

describe("config", () => {
  it("refuses an appOrigin that is not an exact origin", async () => {
    await expect(
      setup({ appOrigin: "https://mail.example.com/" })
    ).rejects.toThrow(/appOrigin/u);
    await expect(setup({ appOrigin: "*" })).rejects.toThrow(/appOrigin/u);
  });

  it("limits can only be tightened", async () => {
    await expect(setup({ limits: { maxImagesPerEmail: 0 } })).rejects.toThrow(
      /positive/u
    );
    const proxy = await setup({ limits: { maxImagesPerEmail: 1000 } });
    const tickets = await manyTickets(proxy, 61);
    await expect(
      bodyOf(proxy.handler(post({ tickets })))
    ).resolves.toStrictEqual({ error: "too-many-images" });
  });
});

describe("POST /batch, batch mode", () => {
  it("returns every image of the email", async () => {
    const { handler, k, calls } = await setup();
    const a = await ticket(k, { url: URL_A });
    const b = await ticket(k, { url: URL_B });
    const images = await imagesOf(handler(post({ tickets: [a, b] })));
    expect(images[a]?.type).toBe("image/png");
    expect(decode(images[a]?.data)).toStrictEqual(png());
    expect(images[b]?.type).toBe("image/gif");
    expect(calls.toSorted()).toStrictEqual([URL_A, URL_B]);
  });

  it("downloads a URL once even if several tickets carry it", async () => {
    const { handler, k, calls } = await setup();
    const a = await ticket(k, { url: URL_A });
    const again = await ticket(k, { expiresAt: NOW + 1000, url: URL_A });
    const images = await imagesOf(handler(post({ tickets: [a, again, a] })));
    expect(Object.keys(images)).toHaveLength(2);
    expect(calls).toStrictEqual([URL_A]);
  });

  it("refuses the whole batch if any ticket is invalid", async () => {
    const { handler, k, calls } = await setup();
    const good = await ticket(k, { url: URL_A });
    const other = await ticket(await keys(`1:${"Q".repeat(42)}A`), {
      url: URL_B,
    });
    const bodies = await Promise.all(
      [
        [good, other],
        [good, "garbage"],
        [good, `${good.slice(0, -2)}AA`],
      ].map(async (tickets) => await bodyOf(handler(post({ tickets }))))
    );
    expect(bodies).toStrictEqual(
      Array.from({ length: 3 }, () => ({ error: "invalid-ticket" }))
    );
    expect(calls).toStrictEqual([]);
  });

  it("refuses expired tickets", async () => {
    const { k } = await setup();
    const a = await ticket(k, { expiresAt: NOW + 1000, url: URL_A });
    const { handler } = await setup({ keys: k, now: () => NOW + 1000 });
    await expect(statusOf(handler(post({ tickets: [a] })))).resolves.toBe(400);
  });

  it("one email per batch: mixed accounts or emails → 400", async () => {
    const { handler, k, calls } = await setup();
    const a = await ticket(k, { url: URL_A });
    const others = await Promise.all(
      [{ emailId: "mail2" }, { accountId: "acc2" }].map(
        async (other) => await ticket(k, { ...other, url: URL_B })
      )
    );
    const bodies = await Promise.all(
      others.map(async (b) => await bodyOf(handler(post({ tickets: [a, b] }))))
    );
    expect(bodies).toStrictEqual([
      { error: "mixed-email" },
      { error: "mixed-email" },
    ]);
    expect(calls).toStrictEqual([]);
  });

  it.each([
    ["not JSON", "{"],
    ["no tickets", { tickets: [] }],
    ["wrong type", { tickets: "x" }],
    ["extra field", { extra: 1, tickets: ["x"] }],
    ["unknown mode", { mode: "preload", tickets: ["x"] }],
  ])("400 for %s", async (_name, body) => {
    const { handler } = await setup();
    const response = await handler(post(body));
    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toStrictEqual({
      error: "invalid-body",
    });
  });

  it("415 without a JSON content type", async () => {
    const { handler } = await setup();
    const request = post({ tickets: ["x"] }, { "content-type": "text/plain" });
    await expect(statusOf(handler(request))).resolves.toBe(415);
  });

  it("413 for an oversized body, whatever Content-Length says", async () => {
    const { handler } = await setup({ limits: { maxImagesPerEmail: 1 } });
    const request = post(`{"tickets":["${"A".repeat(20_000)}"]}`);
    await expect(statusOf(handler(request))).resolves.toBe(413);
  });

  it("cache mode without storage → 400", async () => {
    const { handler, k } = await setup();
    const request = post({ mode: "cache", tickets: [await ticket(k)] });
    await expect(bodyOf(handler(request))).resolves.toStrictEqual({
      error: "cache-unavailable",
    });
  });

  it("refuses a tracker URL inside a validly signed ticket (leaked key)", async () => {
    const log = vi.fn<(event: ProxyLogEvent) => void>();
    const { handler, k, calls } = await setup(
      { log },
      { [TRACKER]: { bytes: gif(), type: "image/gif" } }
    );
    const t = await ticket(k, { url: TRACKER });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      error: "tracker",
    });
    expect(calls).toStrictEqual([]);
    expect(log).toHaveBeenCalledWith({ event: "tracker-in-ticket" });
  });

  it("a throwing tracker check fails closed", async () => {
    const { handler, k, calls } = await setup({
      isTracker: () => {
        throw new Error("boom");
      },
    });
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      error: "tracker",
    });
    expect(calls).toStrictEqual([]);
  });

  it("re-checks the fetcher's output: SVG, HTML, mismatched magic, oversize", async () => {
    const cases: [string, Awaited<ReturnType<FetchImage>>, string][] = [
      [
        "html",
        { bytes: text("<html><script>alert(1)</script>"), type: "text/html" },
        "not-image",
      ],
      ["liar", { bytes: gif(), type: "image/png" }, "not-image"],
      [
        "svg",
        {
          bytes: text("<svg xmlns='http://www.w3.org/2000/svg'/>"),
          type: "image/svg+xml",
        },
        "not-image",
      ],
      ["fail", { error: "content-type" }, "not-image"],
      [
        "big",
        { bytes: png(5 * 1024 * 1024 + 1), type: "image/png" },
        "too-large",
      ],
    ];
    const { handler, k } = await setup(
      {},
      Object.fromEntries(
        cases.map(([name, response]) => [imageUrlFor(name), response])
      )
    );
    const tickets = await Promise.all(
      cases.map(async ([name]) => await ticket(k, { url: imageUrlFor(name) }))
    );
    const images = await imagesOf(handler(post({ tickets })));
    expect(tickets.map((t) => images[t])).toStrictEqual(
      cases.map((entry) => ({ error: entry[2] }))
    );
    // Half a second alone, over 5 s while turbo runs every package at once.
  }, 30_000);

  it("maps guard failures to coarse errors and treats throws as upstream", async () => {
    const urls = ["p", "n", "t", "u", "x"].map(
      (n) => `https://images.example.com/${n}.png`
    );
    const table: Responses = {
      [urls[0] ?? ""]: { error: "private-ip" },
      [urls[1] ?? ""]: { error: "network" },
      [urls[2] ?? ""]: { error: "timeout" },
      [urls[3] ?? ""]: { error: "upstream" },
    };
    const { handler, k } = await setup({
      fetchImage: async (url) => {
        await Promise.resolve();
        if (url.endsWith("x.png")) {
          throw new Error(url);
        }
        return table[url] ?? { error: "dns" };
      },
    });
    const tickets = await Promise.all(
      urls.map(async (url) => await ticket(k, { url }))
    );
    const images = await imagesOf(handler(post({ tickets })));
    expect(tickets.map((t) => images[t])).toStrictEqual([
      { error: "blocked" },
      { error: "upstream" },
      { error: "timeout" },
      { error: "upstream" },
      { error: "upstream" },
    ]);
  });

  it("per-email byte budget: bodies read and refused count too", async () => {
    let read = 0;
    const proxy = await setup({
      // Like net-guard: every byte read is taken, then the body is refused.
      fetchImage: async (_url, _signal, options) => {
        await Promise.resolve();
        if (options?.budget.takeBytes(1000) !== true) {
          return { error: "size" };
        }
        read += 1000;
        return { error: "content-type" };
      },
      limits: { concurrency: 1, maxBytesPerEmail: 3500 },
    });
    const tickets = await manyTickets(proxy, 10);
    const images = await imagesOf(proxy.handler(post({ tickets })));
    expect(tickets.map((t) => images[t]?.error ?? "ok")).toStrictEqual([
      "not-image",
      "not-image",
      "not-image",
      ...Array.from({ length: 7 }, () => "too-large"),
    ]);
    expect(read).toBe(3000);
  });

  it("per-email byte budget: what the fetcher took is not taken twice", async () => {
    const proxy = await setup({
      fetchImage: async (_url, _signal, options) => {
        await Promise.resolve();
        const bytes = png(400);
        if (options?.budget.takeBytes(bytes.byteLength) !== true) {
          return { error: "size" };
        }
        return { bytes, type: "image/png" };
      },
      limits: { concurrency: 1, maxBytesPerEmail: 1000 },
    });
    const tickets = await manyTickets(proxy, 4);
    const images = await imagesOf(proxy.handler(post({ tickets })));
    expect(tickets.map((t) => images[t]?.error ?? "ok")).toStrictEqual([
      "ok",
      "ok",
      "too-large",
      "too-large",
    ]);
  });

  // The fake ignores `options.budget`: the core still takes what it keeps.
  it("per-email byte budget: stops at the budget, the rest are too-large", async () => {
    const urls = Array.from(
      { length: 5 },
      (_, i) => `https://images.example.com/${i}.png`
    );
    const responses: Responses = Object.fromEntries(
      urls.map((url) => [url, { bytes: png(400), type: "image/png" }])
    );
    const proxy = await setup(
      { limits: { concurrency: 1, maxBytesPerEmail: 1000 } },
      responses
    );
    const tickets = await manyTickets(proxy, 5);
    const images = await imagesOf(proxy.handler(post({ tickets })));
    const results = tickets.map((t) => images[t]?.error ?? "ok");
    expect(results).toStrictEqual([
      "ok",
      "ok",
      "too-large",
      "too-large",
      "too-large",
    ]);
  });

  it("respects the concurrency bound", async () => {
    let active = 0;
    let peak = 0;
    const proxy = await setup({
      fetchImage: async () => {
        active += 1;
        peak = Math.max(peak, active);
        await sleep(5);
        active -= 1;
        return { bytes: png(), type: "image/png" };
      },
      limits: { concurrency: 3 },
    });
    const tickets = await manyTickets(proxy, 12);
    const images = await imagesOf(proxy.handler(post({ tickets })));
    expect(Object.values(images).map((r) => r.type)).toStrictEqual(
      Array.from({ length: 12 }, () => "image/png")
    );
    expect(peak).toBe(3);
  });

  it("batch deadline: unfinished images become timeout", async () => {
    const { handler, k } = await setup({
      fetchImage: async (_url, signal) => {
        await once(signal, "abort");
        return { error: "timeout" };
      },
      limits: { batchTimeoutMs: 20 },
    });
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      error: "timeout",
    });
  });

  const HTTP_URL = "http://images.example.com/a.png";
  const HTTPS_URL = "https://images.example.com/a.png";
  const IMAGE = { bytes: png(), type: "image/png" } as const;

  it.each([
    ["https works", { [HTTPS_URL]: IMAGE }, [HTTPS_URL], "image/png"],
    [
      "https refused → http",
      { [HTTPS_URL]: { error: "network" }, [HTTP_URL]: IMAGE },
      [HTTPS_URL, HTTP_URL],
      "image/png",
    ],
    // Not connection failures: no downgrade.
    [
      "https 404",
      { [HTTPS_URL]: { error: "upstream" }, [HTTP_URL]: IMAGE },
      [HTTPS_URL],
      undefined,
    ],
    [
      "https blocked",
      { [HTTPS_URL]: { error: "private-ip" }, [HTTP_URL]: IMAGE },
      [HTTPS_URL],
      undefined,
    ],
    [
      "https not an image",
      { [HTTPS_URL]: { error: "content-type" }, [HTTP_URL]: IMAGE },
      [HTTPS_URL],
      undefined,
    ],
  ] satisfies [string, Responses, string[], string | undefined][])(
    "http URL: %s",
    async (_name, responses, expectedCalls, type) => {
      const { handler, k, calls } = await setup({}, responses);
      const t = await ticket(k, { url: HTTP_URL });
      const result = await resultOf(handler, [t], t);
      expect(calls).toStrictEqual(expectedCalls);
      expect(result?.type).toBe(type);
    }
  );

  it("keeps explicit ports when upgrading (net-guard then refuses them)", async () => {
    const { handler, k, calls } = await setup({}, {});
    const t = await ticket(k, { url: "http://images.example.com:8080/a.png" });
    await handler(post({ tickets: [t] }));
    expect(calls[0]).toBe("https://images.example.com:8080/a.png");
  });

  it("rate limits per IP and per account", async () => {
    const seen: string[] = [];
    const { handler, k } = await setup({
      rateLimit: (key) => {
        seen.push(key);
        return !key.startsWith("batch:account:");
      },
    });
    const request = post({ tickets: [await ticket(k)] });
    await expect(
      statusOf(handler(request, { clientAddress: "203.0.113.9" }))
    ).resolves.toBe(429);
    expect(seen).toStrictEqual(["batch:ip:203.0.113.9", "batch:account:acc1"]);
  });

  it("batch:global is spent only by valid tickets, after the account", async () => {
    const { handler, k } = await setup({
      rateLimit: createRateLimiter({
        rules: {
          "batch:account": { capacity: 2, refillPerSecond: 0 },
          "batch:global": { capacity: 3, refillPerSecond: 0 },
          "batch:ip": { capacity: 100, refillPerSecond: 0 },
        },
      }),
    });
    const valid = await ticket(k);
    const forged = `${valid.slice(0, -4)}AAAA`;
    const statuses = await inOrder(
      10,
      async (i) =>
        await statusOf(
          handler(post({ tickets: [forged] }), {
            clientAddress: `203.0.113.${i}`,
          })
        )
    );
    expect(statuses).toStrictEqual(Array.from({ length: 10 }, () => 400));
    // One account over its own limit, from many IPs: 429, global untouched.
    const accountStatuses = await inOrder(
      6,
      async (i) =>
        await statusOf(
          handler(post({ tickets: [valid] }), {
            clientAddress: `198.51.100.${i}`,
          })
        )
    );
    expect(accountStatuses).toStrictEqual([200, 200, 429, 429, 429, 429]);
    // Another account still gets the one global token left.
    const other = await ticket(k, { accountId: "acc2" });
    await expect(statusOf(handler(post({ tickets: [other] })))).resolves.toBe(
      200
    );
  });

  it("image:global is spent only by valid tickets", async () => {
    const { handler, k } = await cacheSetup({
      rateLimit: createRateLimiter({
        rules: {
          "image:global": { capacity: 2, refillPerSecond: 0 },
          "image:ip": { capacity: 100, refillPerSecond: 0 },
        },
      }),
    });
    for (let i = 0; i < 10; i += 1) {
      // oxlint-disable-next-line no-await-in-loop
      await expect(
        statusOf(
          handler(get("not-a-ticket"), { clientAddress: `203.0.113.${i}` })
        )
      ).resolves.toBe(403);
    }
    const t = await ticket(k, { url: URL_A });
    await expect(statusOf(handler(get(t)))).resolves.toBe(404);
  });

  it("logs never contain URLs, tickets or ids", async () => {
    const events: ProxyLogEvent[] = [];
    const { handler, k } = await setup({
      log: (event) => {
        events.push(event);
      },
    });
    const t = await ticket(k, { url: URL_A });
    await handler(post({ tickets: [t, "bad"] }));
    await handler(post({ tickets: [t] }));
    const dump = JSON.stringify(events);
    expect(events.length).toBeGreaterThan(1);
    for (const secret of [
      "images.example.com",
      t.slice(0, 20),
      "acc1",
      "mail1",
    ]) {
      expect(dump).not.toContain(secret);
    }
  });
});

describe("CORS", () => {
  it("allows exactly the app origin", async () => {
    const { handler, k } = await setup();
    const response = await handler(post({ tickets: [await ticket(k)] }));
    expect(response.headers.get("access-control-allow-origin")).toBe(
      APP_ORIGIN
    );
    expect(response.headers.get("vary")).toBe("Origin");
    expect(response.headers.get("access-control-allow-credentials")).toBeNull();
  });

  it.each([
    ["other origin", { origin: "https://evil.example" }],
    ["sibling", { origin: "https://mail.example.com.evil.example" }],
    ["null", { origin: "null" }],
    ["http variant", { origin: "http://mail.example.com" }],
  ])("403 for %s", async (_name, headers) => {
    const { handler, k, calls } = await setup();
    const response = await handler(
      post({ tickets: [await ticket(k)] }, headers)
    );
    expect(response.status).toBe(403);
    expect(response.headers.get("access-control-allow-origin")).toBeNull();
    expect(calls).toStrictEqual([]);
  });

  it("403 without Origin", async () => {
    const { handler, k } = await setup();
    const request = new Request(`${BASE}/batch`, {
      body: JSON.stringify({ tickets: [await ticket(k)] }),
      headers: { "content-type": "application/json" },
      method: "POST",
    });
    await expect(statusOf(handler(request))).resolves.toBe(403);
  });

  it("answers the preflight for POST + content-type", async () => {
    const { handler } = await setup();
    const response = await handler(
      preflight({
        "access-control-request-headers": "Content-Type",
        "access-control-request-method": "POST",
      })
    );
    expect(response.status).toBe(204);
    expect(Object.fromEntries(response.headers)).toMatchObject({
      "access-control-allow-headers": "content-type",
      "access-control-allow-methods": "POST",
      "access-control-allow-origin": APP_ORIGIN,
      vary: "Origin",
    });
  });

  it.each([
    ["other method", { "access-control-request-method": "PUT" }],
    [
      "extra header",
      {
        "access-control-request-headers": "content-type, authorization",
        "access-control-request-method": "POST",
      },
    ],
    [
      "other origin",
      {
        "access-control-request-method": "POST",
        origin: "https://evil.example",
      },
    ],
  ])("refuses the preflight: %s", async (_name, headers) => {
    const { handler } = await setup();
    const response = await handler(preflight(headers));
    expect(response.status).toBe(403);
    expect(response.headers.get("access-control-allow-origin")).toBeNull();
  });
});

describe("cache mode", () => {
  it("POST downloads and stores", async () => {
    const { handler, k, calls } = await cacheSetup();
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      cached: true,
    });
    expect(calls).toStrictEqual([URL_A]);
  });

  it("GET serves from storage with exactly the image response headers", async () => {
    const { handler, k } = await cacheSetup();
    const t = await ticket(k, { url: URL_A });
    await handler(post({ tickets: [t] }));
    const response = await handler(get(t));
    expect(response.status).toBe(200);
    expect(new Uint8Array(await response.arrayBuffer())).toStrictEqual(png());
    expect(Object.fromEntries(response.headers)).toStrictEqual({
      "cache-control": "private, max-age=300, immutable",
      "content-length": String(png().length),
      "content-security-policy":
        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
      "content-type": "image/png",
      "cross-origin-resource-policy": "cross-origin",
      "referrer-policy": "no-referrer",
      "x-content-type-options": "nosniff",
    });
    expect(IMAGE_CSP).toBe(
      "default-src 'none'; style-src 'unsafe-inline'; sandbox"
    );
  });

  it("GET never calls the fetcher, even when the image is missing", async () => {
    const fetchImage = vi.fn<FetchImage>();
    const { handler, k } = await cacheSetup({ fetchImage });
    const t = await ticket(k, { url: URL_A });
    await expect(statusOf(handler(get(t)))).resolves.toBe(404);
    expect(fetchImage).not.toHaveBeenCalled();
  });

  it("a second POST is answered from storage without downloading", async () => {
    const { handler, k, calls } = await cacheSetup();
    const t = await ticket(k, { url: URL_A });
    await handler(post({ tickets: [t] }));
    const again = await ticket(k, { expiresAt: NOW + 1000, url: URL_A });
    await expect(resultOf(handler, [again], again)).resolves.toStrictEqual({
      cached: true,
    });
    expect(calls).toStrictEqual([URL_A]);
  });

  it("concurrent POSTs for the same image download it once", async () => {
    const { handler, k, calls } = await cacheSetup();
    const t = await ticket(k, { url: URL_A });
    const results = await Promise.all([
      resultOf(handler, [t], t),
      resultOf(handler, [t], t),
    ]);
    expect(results).toStrictEqual([{ cached: true }, { cached: true }]);
    expect(calls).toStrictEqual([URL_A]);
  });

  it("entries are per account", async () => {
    const { handler, k, calls } = await cacheSetup();
    await handler(post({ tickets: [await ticket(k, { url: URL_A })] }));
    const other = await ticket(k, { accountId: "acc2", url: URL_A });
    await expect(statusOf(handler(get(other)))).resolves.toBe(404);
    await handler(post({ tickets: [other] }));
    expect(calls).toStrictEqual([URL_A, URL_A]);
  });

  it("the client may choose batch mode even with storage (nothing stored)", async () => {
    const { handler, k } = await cacheSetup();
    const t = await ticket(k, { url: URL_A });
    const result = await resultOf(handler, [t], t, "batch");
    expect(result?.type).toBe("image/png");
    await expect(statusOf(handler(get(t)))).resolves.toBe(404);
  });

  it("errors are reported and nothing is stored", async () => {
    const { handler, k } = await cacheSetup(
      {},
      { [URL_A]: { error: "private-ip" } }
    );
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      error: "blocked",
    });
    await expect(statusOf(handler(get(t)))).resolves.toBe(404);
  });

  it("a failing storage reports `storage`", async () => {
    const storage: ImageStorage = {
      get: async () => await Promise.resolve(null),
      put: async () => {
        await Promise.resolve();
        throw new Error("disk full");
      },
    };
    const { handler, k } = await setup({ storage });
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      error: "storage",
    });
  });

  it("keys are opaque hashes; tampered contents are not served", async () => {
    const entries = new Map<string, StoredImage>();
    const storage: ImageStorage = {
      get: async (key) => await Promise.resolve(entries.get(key) ?? null),
      put: async (key, image) => {
        await Promise.resolve();
        entries.set(key, image);
      },
    };
    const { handler, k } = await setup({ storage });
    const t = await ticket(k, { url: URL_A });
    await handler(post({ tickets: [t] }));
    const [key = ""] = entries.keys();
    expect(key).toMatch(/^v1:[0-9a-f]{64}:[0-9a-f]{64}$/u);
    entries.set(key, { bytes: text("<svg/>"), type: "image/svg+xml" });
    await expect(statusOf(handler(get(t)))).resolves.toBe(404);
  });

  it("GET refuses invalid, expired and tracker tickets", async () => {
    const { handler, k, storage } = await cacheSetup();
    const t = await ticket(k, { expiresAt: NOW + 1000, url: URL_A });
    await handler(post({ tickets: [t] }));
    const later = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: vi.fn<FetchImage>(),
      keys: k,
      now: () => NOW + 1000,
      storage,
    });
    const tracker = await ticket(k, { url: TRACKER });
    const statuses = await Promise.all([
      statusOf(handler(get("garbage"))),
      statusOf(later(get(t))),
      statusOf(handler(get(tracker))),
    ]);
    expect(statuses).toStrictEqual([403, 403, 403]);
  });

  it("the sender's no-store, no-cache or max-age don't stop one account's cache", async () => {
    for (const cacheControl of ["no-store", "no-cache", "max-age=0"]) {
      // oxlint-disable-next-line no-await-in-loop -- one setup per value.
      const { handler, k, calls } = await cacheSetup(
        {},
        {
          [URL_A]: {
            bytes: png(),
            cacheHeaders: { "cache-control": cacheControl },
            type: "image/png",
          },
        }
      );
      // oxlint-disable-next-line no-await-in-loop
      const t = await ticket(k, { url: URL_A });
      // oxlint-disable-next-line no-await-in-loop
      await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
        cached: true,
      });
      // oxlint-disable-next-line no-await-in-loop
      await expect(statusOf(handler(get(t)))).resolves.toBe(200);
      // Opening the email again downloads nothing: the sender can't count
      // the opens.
      // oxlint-disable-next-line no-await-in-loop
      await handler(post({ tickets: [t] }));
      expect(calls).toStrictEqual([URL_A]);
    }
  });

  it("a short max-age of the sender doesn't shorten the browser's copy either", async () => {
    const { handler, k } = await cacheSetup(
      {},
      {
        [URL_A]: {
          bytes: png(),
          cacheHeaders: { "cache-control": "private, max-age=120" },
          type: "image/png",
        },
      }
    );
    const t = await ticket(k, { url: URL_A });
    await expect(resultOf(handler, [t], t)).resolves.toStrictEqual({
      cached: true,
    });
    const response = await handler(get(t));
    expect(response.headers.get("cache-control")).toBe(
      "private, max-age=300, immutable"
    );
  });

  it("a storage with a fixed lifetime gets every image", async () => {
    const memory = createMemoryStorage({ now: () => NOW });
    const storage: ImageStorage = {
      ...memory,
      fixedTtlMs: DEFAULT_LIMITS.cacheTtlMs,
    };
    const { handler, k } = await setup(
      { storage },
      {
        [URL_A]: {
          bytes: png(),
          cacheHeaders: { "cache-control": "max-age=3600" },
          type: "image/png",
        },
        [URL_B]: { bytes: gif(), type: "image/gif" },
      }
    );
    const a = await ticket(k, { url: URL_A });
    const b = await ticket(k, { url: URL_B });
    const images = await imagesOf(handler(post({ tickets: [a, b] })));
    expect(images[a]).toStrictEqual({ cached: true });
    expect(images[b]).toStrictEqual({ cached: true });
  });

  it("max-age is the ticket's remaining lifetime", async () => {
    const { handler, k } = await cacheSetup();
    const t = await ticket(k, { expiresAt: NOW + 61_500, url: URL_A });
    await handler(post({ tickets: [t] }));
    const response = await handler(get(t));
    expect(response.headers.get("cache-control")).toBe(
      "private, max-age=61, immutable"
    );
  });
});

describe("routing", () => {
  it("GET /i/<ticket> without storage → 404, never fetches", async () => {
    const fetchImage = vi.fn<FetchImage>();
    const { handler, k } = await setup({ fetchImage });
    const request = new Request(`${BASE}/i/${await ticket(k)}`);
    await expect(statusOf(handler(request))).resolves.toBe(404);
    expect(fetchImage).not.toHaveBeenCalled();
  });

  it("healthz", async () => {
    const { handler } = await setup();
    await expect(
      bodyOf(handler(new Request(`${BASE}/healthz`)))
    ).resolves.toStrictEqual({ modes: ["batch"], status: "ok" });
  });

  it.each([
    ["GET", "/"],
    ["GET", "/batch"],
    ["GET", "/i/"],
    ["GET", "/i/a/b"],
    ["GET", "/i/%2e%2e"],
    ["POST", "/i/abc"],
    ["OPTIONS", "/healthz"],
  ])("%s %s → 404", async (method, path) => {
    const { handler } = await setup();
    const request = new Request(`${BASE}${path}`, { method });
    await expect(statusOf(handler(request))).resolves.toBe(404);
  });

  it.each(["PUT", "DELETE", "PATCH", "HEAD"])("%s → 405", async (method) => {
    const { handler } = await setup();
    const response = await handler(new Request(`${BASE}/batch`, { method }));
    expect(response.status).toBe(405);
    expect(response.headers.get("allow")).toBe("GET, POST, OPTIONS");
  });

  it("error responses never echo input", async () => {
    const { handler } = await setup();
    const secret = "https://secret.example/private.png";
    const response = await handler(post({ tickets: [secret] }));
    await expect(response.text()).resolves.not.toContain("secret");
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it("tickets for keys it does not have are refused", async () => {
    const { handler } = await setup({ keys: await keys(KEY_1) });
    const request = post({ tickets: ["1.1.AAAA.AAAA"] });
    await expect(statusOf(handler(request))).resolves.toBe(400);
  });
});
