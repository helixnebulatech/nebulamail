// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Worker run as Cloudflare would: `fetch(request, env)` with a fake env
// (vars, secret, R2 binding) and a fake network that plays the DoH server
// and the image servers. Also: every blocked vector of ssrf-vectors.json as
// a correctly signed ticket, and the fetch vectors that apply to Workers.

import { afterEach, describe, expect, it, vi } from "vitest";
import { z } from "zod";

// Test-only helpers, deliberately not in net-guard's package exports.
import { DOH, fakeNet } from "../../../net-guard/src/workers/testing";
import type { FakeDns, Route } from "../../../net-guard/src/workers/testing";
import { APP_ORIGIN, gif, KEY_1, keys, png, text } from "../test-fixtures";
import {
  BLOCKED_VECTORS,
  expectedForBlocked,
  runBlockedVector,
} from "../test-vectors";
import { signTicket } from "../tickets/index";
import { createFakeR2 } from "./test-r2";
import { createWorkersProxyForTests } from "./testing";

const PUBLIC = "93.184.215.14";
const BASE = "https://img.example.com";

const Images = z.object({
  images: z.record(z.string(), z.record(z.string(), z.unknown())),
});

const image =
  (type: string, bytes: Uint8Array): Route =>
  () =>
    new Response(Uint8Array.from(bytes), { headers: { "content-type": type } });

const ROUTES: Record<string, Route> = {
  "https://images.example.com/a.png": image("image/png", png()),
  "https://images.example.com/b.gif": image("image/gif", gif()),
  "https://images.example.com/page": image("text/html", text("<html>")),
  "https://images.example.com/to-private": () =>
    new Response(null, {
      headers: { location: "http://10.0.0.1/x.png" },
      status: 302,
    }),
};

const DNS = (name: string): FakeDns | undefined => {
  if (name === "images.example.com") {
    return { A: [PUBLIC] };
  }
  if (name === "mixed.example.com") {
    return { A: [PUBLIC, "127.0.0.1"] };
  }
  return undefined;
};

const baseEnv = {
  APP_URL: `${APP_ORIGIN}/app`,
  IMAGE_PROXY_DOH_URL: DOH,
  IMAGE_PROXY_KEYS: KEY_1,
};

const setup = (env: Record<string, unknown> = baseEnv) => {
  const net = fakeNet(DNS, ROUTES);
  const worker = createWorkersProxyForTests({
    net: net.fetch,
    rateLimit: false,
  });
  const call = async (request: Request) => await worker.fetch(request, env);
  return { call, env, net, worker };
};

const sign = async (url: string, emailId = "mail1") =>
  await signTicket(
    await keys(),
    { accountId: "acc1", emailId, expiresAt: Date.now() + 60_000, url },
    Date.now()
  );

const batch = (body: unknown, origin = APP_ORIGIN) =>
  new Request(`${BASE}/batch`, {
    body: JSON.stringify(body),
    headers: { "content-type": "application/json", origin },
    method: "POST",
  });

const imagesOf = async (response: Response) =>
  Images.parse(await response.json()).images;

describe("workers adapter", () => {
  it("reports batch only without R2, batch and cache with it", async () => {
    const { call } = setup();
    const plain = await call(new Request(`${BASE}/healthz`));
    await expect(plain.json()).resolves.toStrictEqual({
      modes: ["batch"],
      status: "ok",
    });
    const withR2 = setup({ ...baseEnv, IMAGE_CACHE_BUCKET: createFakeR2() });
    const cached = await withR2.call(new Request(`${BASE}/healthz`));
    await expect(cached.json()).resolves.toStrictEqual({
      modes: ["batch", "cache"],
      status: "ok",
    });
  });

  it("batch mode: checks DoH, downloads everything, answers base64", async () => {
    const { call } = setup();
    const a = await sign("https://images.example.com/a.png");
    const b = await sign("http://images.example.com/b.gif");
    const html = await sign("https://images.example.com/page");
    const response = await call(batch({ tickets: [a, b, html] }));
    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBe(
      APP_ORIGIN
    );
    const images = await imagesOf(response);
    expect(images[a]).toStrictEqual({
      data: Buffer.from(png()).toString("base64"),
      type: "image/png",
    });
    // http:// is tried over https:// first.
    expect(images[b]).toMatchObject({ type: "image/gif" });
    expect(images[html]).toStrictEqual({ error: "not-image" });
  });

  it("asks DoH for A and AAAA, and sends no cookies or referrer", async () => {
    const { call, net } = setup();
    const a = await sign("https://images.example.com/a.png");
    await call(batch({ tickets: [a] }));
    expect(net.queries).toStrictEqual([
      "images.example.com/1",
      "images.example.com/28",
    ]);
    expect(
      net.requests.map((r) => [
        r.init.redirect,
        /cookie|referer/iu.test(JSON.stringify(r.init.headers)),
      ])
    ).toStrictEqual([["manual", false]]);
  });

  it("falls back to http:// only when https fails to connect", async () => {
    const net = fakeNet(DNS, {
      "http://images.example.com/only-http.png": image("image/png", png()),
    });
    const worker = createWorkersProxyForTests({
      net: net.fetch,
      rateLimit: false,
    });
    const t = await sign("http://images.example.com/only-http.png");
    const images = await imagesOf(
      await worker.fetch(batch({ tickets: [t] }), baseEnv)
    );
    expect(images[t]).toMatchObject({ type: "image/png" });
    expect(net.requests.map((r) => r.url)).toStrictEqual([
      "https://images.example.com/only-http.png",
      "http://images.example.com/only-http.png",
    ]);
  });

  it("blocks private DoH answers and redirects to private IPs without fetching them", async () => {
    const { call, net } = setup();
    const mixed = await sign("https://mixed.example.com/a.png");
    const redirect = await sign("https://images.example.com/to-private");
    const images = await imagesOf(
      await call(batch({ tickets: [mixed, redirect] }))
    );
    expect(images[mixed]).toStrictEqual({ error: "blocked" });
    expect(images[redirect]).toStrictEqual({ error: "blocked" });
    expect(net.requests.map((r) => r.url)).toStrictEqual([
      "https://images.example.com/to-private",
    ]);
  });

  it("cache mode: stores in R2 under hashed keys, GET serves only from R2", async () => {
    const bucket = createFakeR2();
    const { call, net } = setup({ ...baseEnv, IMAGE_CACHE_BUCKET: bucket });
    const url = "https://images.example.com/a.png";
    const t = await sign(url);

    // Before the batch: not stored, and a GET never downloads.
    const missing = await call(new Request(`${BASE}/i/${t}`));
    expect([missing.status, net.requests.length]).toStrictEqual([404, 0]);

    const images = await imagesOf(await call(batch({ tickets: [t] })));
    expect(images[t]).toStrictEqual({ cached: true });
    // Only hashes in the bucket: no URL, no account id.
    expect([...bucket.objects.keys()]).toStrictEqual([
      expect.stringMatching(/^img\/v1:[0-9a-f]{64}:[0-9a-f]{64}$/u),
    ]);

    const served = await call(new Request(`${BASE}/i/${t}`));
    expect({
      body: new Uint8Array(await served.arrayBuffer()),
      nosniff: served.headers.get("x-content-type-options"),
      status: served.status,
      type: served.headers.get("content-type"),
    }).toStrictEqual({
      body: png(),
      nosniff: "nosniff",
      status: 200,
      type: "image/png",
    });
    // Still the single download of the batch.
    expect(net.requests).toHaveLength(1);
  });

  it("the cron sweep deletes expired R2 entries", async () => {
    const bucket = createFakeR2(() => Date.now() - 31 * 24 * 60 * 60 * 1000);
    const { call, worker, env } = setup({
      ...baseEnv,
      IMAGE_CACHE_BUCKET: bucket,
    });
    const t = await sign("https://images.example.com/a.png");
    await call(batch({ tickets: [t] }));
    expect(bucket.objects.size).toBe(1);
    await worker.scheduled({}, env);
    expect(bucket.objects.size).toBe(0);
  });

  it("registers the work with waitUntil, so a disconnect cannot leak slots", async () => {
    const { env, worker } = setup();
    const registered: Promise<unknown>[] = [];
    const ctx = {
      waitUntil: (promise: Promise<unknown>) => {
        registered.push(promise);
      },
    };
    const t = await sign("https://images.example.com/a.png");
    const response = await worker.fetch(batch({ tickets: [t] }), env, ctx);
    expect(response.status).toBe(200);
    expect(registered).toHaveLength(1);
    // The registered work settles (its `finally` blocks have run).
    await expect(registered[0]).resolves.toBeInstanceOf(Response);
    // A second batch is not stuck behind a leaked slot.
    const again = await worker.fetch(batch({ tickets: [t] }), env, ctx);
    expect(again.status).toBe(200);
    expect(registered).toHaveLength(2);
  });

  it("the waitUntil promise never rejects, even when the handler throws", async () => {
    const { env, worker } = setup();
    const registered: Promise<unknown>[] = [];
    const ctx = {
      waitUntil: (promise: Promise<unknown>) => {
        registered.push(promise);
      },
    };
    // A body that errors while the handler reads it.
    const body = new ReadableStream<Uint8Array>({
      pull: (controller) => {
        controller.error(new Error("client went away"));
      },
    });
    const response = await worker.fetch(
      new Request(`${BASE}/batch`, {
        body,
        duplex: "half",
        headers: { "content-type": "application/json", origin: APP_ORIGIN },
        method: "POST",
      } as RequestInit),
      env,
      ctx
    );
    expect(response.status).toBeGreaterThanOrEqual(400);
    expect(registered).toHaveLength(1);
    await expect(registered[0]).resolves.toBeDefined();
  });

  it("refuses other origins", async () => {
    const { call, net } = setup();
    const t = await sign("https://images.example.com/a.png");
    const response = await call(
      batch({ tickets: [t] }, "https://evil.example")
    );
    expect(response.status).toBe(403);
    expect(net.requests).toStrictEqual([]);
  });

  it("rate limits by cf-connecting-ip", async () => {
    const net = fakeNet(DNS, ROUTES);
    const worker = createWorkersProxyForTests({
      net: net.fetch,
      rateLimit: { "batch:ip": { capacity: 1, refillPerSecond: 0.001 } },
    });
    const t = await sign("https://images.example.com/a.png");
    const send = async () => {
      const request = batch({ tickets: [t] });
      request.headers.set("cf-connecting-ip", "203.0.113.7");
      const response = await worker.fetch(request, baseEnv);
      return response.status;
    };
    expect([await send(), await send()]).toStrictEqual([200, 429]);
  });

  it("uses Cloudflare's Rate Limiting bindings when they are bound", async () => {
    const net = fakeNet(DNS, ROUTES);
    const worker = createWorkersProxyForTests({ net: net.fetch });
    const asked: string[] = [];
    const binding = (allow: boolean) => ({
      limit: async ({ key }: { key: string }) => {
        asked.push(key);
        return await Promise.resolve({ success: allow });
      },
    });
    const env = {
      ...baseEnv,
      RATE_LIMIT_BATCH_ACCOUNT: binding(true),
      RATE_LIMIT_BATCH_GLOBAL: binding(false),
      RATE_LIMIT_BATCH_IP: binding(true),
    };
    const request = batch({
      tickets: [await sign("https://images.example.com/a.png")],
    });
    request.headers.set("cf-connecting-ip", "203.0.113.7");
    const response = await worker.fetch(request, env);
    expect(response.status).toBe(429);
    expect(asked).toStrictEqual([
      "batch:ip:203.0.113.7",
      "batch:account:acc1",
      "batch:global",
    ]);
    expect(net.requests).toStrictEqual([]);
  });

  it("a failing binding falls back to the in-memory limit", async () => {
    const net = fakeNet(DNS, ROUTES);
    const worker = createWorkersProxyForTests({
      net: net.fetch,
      rateLimit: { "batch:global": { capacity: 1, refillPerSecond: 0.001 } },
    });
    const env = {
      ...baseEnv,
      RATE_LIMIT_BATCH_GLOBAL: {
        limit: async () => await Promise.reject(new Error("down")),
      },
    };
    const t = await sign("https://images.example.com/a.png");
    const send = async () => {
      const response = await worker.fetch(batch({ tickets: [t] }), env);
      return response.status;
    };
    expect([await send(), await send()]).toStrictEqual([200, 429]);
  });

  it("S3 storage instead of R2 (cache mode), signed requests to the endpoint", async () => {
    const stored = new Map<string, { bytes: Uint8Array; type: string }>();
    const base = fakeNet(DNS, ROUTES);
    const s3Calls: string[] = [];
    const net = async (input: string, init: RequestInit): Promise<Response> => {
      if (!input.startsWith("https://s3.example.net/")) {
        return await base.fetch(input, init);
      }
      s3Calls.push(`${init.method ?? "GET"} ${new URL(input).pathname}`);
      const headers = new Headers(init.headers);
      expect(headers.get("authorization")).toMatch(/^AWS4-HMAC-SHA256 /u);
      const key = new URL(input).pathname;
      if (init.method === "PUT") {
        stored.set(key, {
          bytes: new Uint8Array(await new Response(init.body).arrayBuffer()),
          type: headers.get("content-type") ?? "",
        });
        return new Response(null, { status: 200 });
      }
      const object = stored.get(key);
      return object === undefined
        ? new Response(null, { status: 404 })
        : new Response(Uint8Array.from(object.bytes), {
            headers: {
              "content-type": object.type,
              "last-modified": new Date().toUTCString(),
            },
          });
    };
    const worker = createWorkersProxyForTests({ net, rateLimit: false });
    const env = {
      ...baseEnv,
      IMAGE_CACHE_S3_ACCESS_KEY_ID: "AKID",
      IMAGE_CACHE_S3_BUCKET: "images",
      IMAGE_CACHE_S3_ENDPOINT: "https://s3.example.net",
      IMAGE_CACHE_S3_REGION: "auto",
      IMAGE_CACHE_S3_SECRET_ACCESS_KEY: "secret",
    };
    const t = await sign("https://images.example.com/a.png");
    const posted = await worker.fetch(batch({ tickets: [t] }), env);
    await expect(posted.json()).resolves.toStrictEqual({
      images: { [t]: { cached: true } },
    });
    const served = await worker.fetch(new Request(`${BASE}/i/${t}`), env);
    expect(served.status).toBe(200);
    expect(s3Calls.map((c) => c.split(" ")[0])).toStrictEqual([
      "GET",
      "PUT",
      "GET",
    ]);
  });
});

describe("workers env", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each([
    ["missing keys", { APP_URL: APP_ORIGIN }],
    ["bad APP_URL", { ...baseEnv, APP_URL: "ftp://mail.example.com" }],
    ["http DoH", { ...baseEnv, IMAGE_PROXY_DOH_URL: "http://doh.test/q" }],
    ["retention of 0 days", { ...baseEnv, IMAGE_CACHE_TTL_DAYS: "0" }],
    ["retention not whole days", { ...baseEnv, IMAGE_CACHE_TTL_DAYS: "1.5" }],
    ["not an R2 binding", { ...baseEnv, IMAGE_CACHE_BUCKET: "bucket" }],
    ["not a rate limit binding", { ...baseEnv, RATE_LIMIT_BATCH_IP: "100" }],
    [
      "S3 without keys",
      {
        ...baseEnv,
        IMAGE_CACHE_S3_BUCKET: "images",
        IMAGE_CACHE_S3_ENDPOINT: "https://s3.example.net",
      },
    ],
    ["bad key", { ...baseEnv, IMAGE_PROXY_KEYS: "1:short" }],
  ])("%s: 500 misconfigured, logs names only", async (_name, env) => {
    const errors: unknown[] = [];
    vi.spyOn(console, "error").mockImplementation((message: unknown) => {
      errors.push(message);
    });
    const { call } = setup(env);
    const response = await call(new Request(`${BASE}/healthz`));
    expect(response.status).toBe(500);
    await expect(response.json()).resolves.toStrictEqual({
      error: "misconfigured",
    });
    // One line naming the problem, never a value.
    expect(errors).toStrictEqual([
      expect.stringMatching(/^image proxy[^/]*$/u),
    ]);
    expect(String(errors[0])).not.toContain("short");
  });
});

describe("ssrf vectors through the Worker", () => {
  it.each(BLOCKED_VECTORS)("$id", async (vector) => {
    const { dns } = vector;
    const net = fakeNet(
      () =>
        dns === undefined
          ? undefined
          : { A: dns.A ?? [], AAAA: dns.AAAA ?? [] },
      {}
    );
    const worker = createWorkersProxyForTests({
      net: net.fetch,
      rateLimit: false,
    });
    const outcome = await runBlockedVector(
      await keys(),
      vector,
      async (request) => await worker.fetch(request, baseEnv)
    );
    expect(outcome).toStrictEqual(expectedForBlocked(vector));
    // Only DoH queries, never a request to the target.
    expect(net.requests).toStrictEqual([]);
  });
});
