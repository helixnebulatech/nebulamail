// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Vercel function as Vercel calls it (`fetch(request)`, env from
// `process.env`-like objects), with net-guard's real pinned Node fetch. As
// in the Node tests, names resolve (fake resolver) to public addresses and
// only the TCP connect of a validated address goes to a local server.

import { once } from "node:events";
import { createServer } from "node:http";
import type { Server } from "node:http";

import type { GuardedFetch } from "@nebulamail/net-guard/node";
import {
  afterAll,
  afterEach,
  beforeAll,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import { z } from "zod";

// Test-only hook, deliberately not in net-guard's package exports.
import { createGuardedFetchForTests } from "../../../net-guard/src/node/testing";
import { APP_ORIGIN, KEY_1, keys, png } from "../test-fixtures";
import {
  BLOCKED_VECTORS,
  expectedForBlocked,
  runBlockedVector,
} from "../test-vectors";
import { signTicket } from "../tickets/index";
import type { VercelProxyOptions } from "./proxy";
import { createFakeBlob } from "./test-blob";
import { createVercelProxyForTests } from "./testing";

const PUBLIC = "93.184.215.14";
const BASE = "https://img.example.com";
const MB = 1024 * 1024;
const CRON_SECRET = "s".repeat(32);

const Images = z.object({
  images: z.record(z.string(), z.record(z.string(), z.unknown())),
});

const env = {
  APP_URL: APP_ORIGIN,
  IMAGE_PROXY_KEYS: KEY_1,
};
const blobEnv = {
  ...env,
  BLOB_READ_WRITE_TOKEN: "vercel_blob_rw_test",
  CRON_SECRET,
  IMAGE_CACHE_STORAGE: "vercel-blob",
};

let images: Server;
const dialed: string[] = [];

const guarded = (): GuardedFetch =>
  createGuardedFetchForTests({
    dial: (address) => {
      dialed.push(address);
      const info = images.address();
      return {
        host: "127.0.0.1",
        port: typeof info === "object" && info !== null ? info.port : 1,
      };
    },
    // https is refused by the dial target (plain http), so http:// is used.
    resolver: async (host) => {
      await Promise.resolve();
      if (host === "images.example.com") {
        return { A: [PUBLIC], AAAA: [] };
      }
      throw new Error("ENOTFOUND");
    },
  });

const proxyWith = (options: Partial<VercelProxyOptions> = {}) =>
  createVercelProxyForTests({
    env,
    guardedFetch: guarded(),
    rateLimit: false,
    ...options,
  });

const sign = async (url: string) =>
  await signTicket(
    await keys(),
    {
      accountId: "acc1",
      emailId: "mail1",
      expiresAt: Date.now() + 60_000,
      url,
    },
    Date.now()
  );

/** A request as Vercel delivers it after the `vercel.json` rewrite. */
const rewritten = (path: string, init: RequestInit = {}) =>
  new Request(`${BASE}/api/proxy?__path=${path}`, init);

const batch = (tickets: string[]) =>
  rewritten("batch", {
    body: JSON.stringify({ tickets }),
    headers: { "content-type": "application/json", origin: APP_ORIGIN },
    method: "POST",
  });

const sweepRequest = (authorization?: string) =>
  new Request(`${BASE}/cron/sweep`, {
    headers: authorization === undefined ? {} : { authorization },
  });

/** Build with `options`, call it once, return the logged config error. */
const misconfigured = async (options: Partial<VercelProxyOptions>) => {
  const errors: unknown[] = [];
  vi.spyOn(console, "error").mockImplementation((message: unknown) => {
    errors.push(message);
  });
  const response = await proxyWith(options).fetch(rewritten("healthz"));
  const body: unknown = await response.json();
  return { body, message: String(errors[0]), status: response.status };
};

describe("vercel function", () => {
  beforeAll(async () => {
    images = createServer((req, res) => {
      if (req.url === "/a.png") {
        res.writeHead(200, { "content-type": "image/png" }).end(png());
      } else if (req.url === "/big.png") {
        res.writeHead(200, { "content-type": "image/png" }).end(png(3.5 * MB));
      } else {
        res.writeHead(404).end();
      }
    });
    images.listen(0, "127.0.0.1");
    await once(images, "listening");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    dialed.length = 0;
  });

  afterAll(() => {
    images.close();
  });

  describe("vercel adapter", () => {
    it("routes the rewritten path and the original one", async () => {
      const proxy = proxyWith();
      for (const request of [
        rewritten("healthz"),
        new Request(`${BASE}/healthz`),
      ]) {
        // oxlint-disable-next-line no-await-in-loop
        const response = await proxy.fetch(request);
        // oxlint-disable-next-line no-await-in-loop
        await expect(response.json()).resolves.toStrictEqual({
          modes: ["batch"],
          status: "ok",
        });
      }
    });

    it("batch mode downloads through the pinned Node fetch", async () => {
      const proxy = proxyWith();
      const t = await sign("http://images.example.com/a.png");
      const response = await proxy.fetch(batch([t]));
      expect(response.status).toBe(200);
      const body = Images.parse(await response.json());
      expect(body.images[t]).toStrictEqual({
        data: Buffer.from(png()).toString("base64"),
        type: "image/png",
      });
      // Every connection went to the validated IP.
      expect(new Set(dialed)).toStrictEqual(new Set([PUBLIC]));
    });

    it("keeps responses under Vercel's 4.5 MB limit (3 MB per email)", async () => {
      const proxy = proxyWith();
      const t = await sign("http://images.example.com/big.png");
      const response = await proxy.fetch(batch([t]));
      const body = Images.parse(await response.json());
      expect(body.images[t]).toStrictEqual({ error: "too-large" });
      // Half a second alone, over 5 s while turbo runs every package at once.
    }, 30_000);

    it("cache mode with Vercel Blob: POST stores, GET serves from Blob only", async () => {
      const blob = createFakeBlob();
      const proxy = proxyWith({ blob, env: blobEnv });
      const t = await sign("http://images.example.com/a.png");
      const missing = await proxy.fetch(rewritten(`i/${t}`));
      expect([missing.status, dialed.length]).toStrictEqual([404, 0]);
      const stored = await proxy.fetch(batch([t]));
      const body = Images.parse(await stored.json());
      expect(body.images[t]).toStrictEqual({ cached: true });
      expect([...blob.blobs.keys()]).toStrictEqual([
        expect.stringMatching(/^img\/v1\/[0-9a-f]{64}\/[0-9a-f]{64}$/u),
      ]);
      const before = dialed.length;
      const served = await proxy.fetch(rewritten(`i/${t}`));
      expect({
        body: new Uint8Array(await served.arrayBuffer()),
        dials: dialed.length - before,
        status: served.status,
        type: served.headers.get("content-type"),
      }).toStrictEqual({
        body: png(),
        dials: 0,
        status: 200,
        type: "image/png",
      });
    });

    it("the cron sweep needs the CRON_SECRET bearer", async () => {
      const blob = createFakeBlob(() => Date.now() - 31 * 24 * 60 * 60 * 1000);
      const proxy = proxyWith({ blob, env: blobEnv });
      const t = await sign("http://images.example.com/a.png");
      await proxy.fetch(batch([t]));
      expect(blob.blobs.size).toBe(1);
      const missing = await proxy.fetch(sweepRequest());
      const wrong = await proxy.fetch(sweepRequest("Bearer wrong"));
      expect([missing.status, wrong.status]).toStrictEqual([401, 401]);
      const ok = await proxy.fetch(sweepRequest(`Bearer ${CRON_SECRET}`));
      await expect(ok.json()).resolves.toStrictEqual({ deleted: 1 });
      expect(blob.blobs.size).toBe(0);
    });

    it("rate limits by x-real-ip", async () => {
      const proxy = proxyWith({
        rateLimit: { "batch:ip": { capacity: 1, refillPerSecond: 0.001 } },
      });
      const t = await sign("http://images.example.com/a.png");
      const send = async () => {
        const request = batch([t]);
        request.headers.set("x-real-ip", "203.0.113.7");
        const response = await proxy.fetch(request);
        return response.status;
      };
      expect([await send(), await send()]).toStrictEqual([200, 429]);
    });
  });

  describe("vercel configuration", () => {
    it("refuses the Edge runtime", async () => {
      vi.stubGlobal("EdgeRuntime", "edge-runtime");
      const { body, message, status } = await misconfigured({});
      expect({ body, status }).toStrictEqual({
        body: { error: "misconfigured" },
        status: 500,
      });
      expect(message).toMatch(/Edge runtime/u);
    });

    it("vercel-blob needs the module, CRON_SECRET and a token", async () => {
      const blob = createFakeBlob();
      const { CRON_SECRET: _cron, ...noCron } = blobEnv;
      const { BLOB_READ_WRITE_TOKEN: _token, ...noToken } = blobEnv;
      const messages = [
        await misconfigured({ env: blobEnv }),
        await misconfigured({ blob, env: noCron }),
        await misconfigured({ blob, env: noToken }),
      ].map((result) => result.message);
      expect(messages).toStrictEqual([
        expect.stringMatching(/@vercel\/blob/u),
        expect.stringMatching(/CRON_SECRET/u),
        expect.stringMatching(/BLOB_READ_WRITE_TOKEN/u),
      ]);
    });

    it("s3 needs a bucket and CRON_SECRET; preload needs storage and the parser", async () => {
      const s3Env = {
        ...env,
        CRON_SECRET,
        IMAGE_CACHE_S3_ACCESS_KEY_ID: "AKID",
        IMAGE_CACHE_S3_BUCKET: "images",
        IMAGE_CACHE_S3_ENDPOINT: "https://s3.example.net",
        IMAGE_CACHE_S3_SECRET_ACCESS_KEY: "secret",
        IMAGE_CACHE_STORAGE: "s3",
      };
      const { IMAGE_CACHE_S3_BUCKET: _bucket, ...noBucket } = s3Env;
      const { CRON_SECRET: _cron, ...noCron } = s3Env;
      const messages = [
        await misconfigured({ env: noBucket }),
        await misconfigured({ env: noCron }),
        await misconfigured({
          env: { ...env, IMAGE_PRELOAD_SECRET: "p".repeat(40) },
        }),
        await misconfigured({
          env: {
            ...s3Env,
            IMAGE_PRELOAD_FULL_EMAIL: "true",
            IMAGE_PRELOAD_SECRET: "p".repeat(40),
          },
        }),
        await misconfigured({
          env: { ...s3Env, IMAGE_PRELOAD_SECRET: "short" },
        }),
      ].map((result) => result.message);
      expect(messages).toStrictEqual([
        expect.stringMatching(/IMAGE_CACHE_S3_BUCKET/u),
        expect.stringMatching(/CRON_SECRET/u),
        expect.stringMatching(/IMAGE_CACHE_STORAGE/u),
        expect.stringMatching(/htmlExtractor/u),
        expect.stringMatching(/IMAGE_PRELOAD_SECRET/u),
      ]);
    });

    it("preload with S3: the hook answers accept and Vercel's waitUntil keeps the work", async () => {
      const objects = new Map<string, Response>();
      const s3 = vi.fn<typeof fetch>(async (input, init = {}) => {
        const key = new URL(input instanceof Request ? input.url : input)
          .pathname;
        if (init.method === "PUT") {
          objects.set(
            key,
            new Response(
              new Uint8Array(await new Response(init.body).arrayBuffer()),
              {
                headers: {
                  "content-type":
                    new Headers(init.headers).get("content-type") ?? "",
                  "last-modified": new Date().toUTCString(),
                },
              }
            )
          );
          return new Response(null, { status: 200 });
        }
        return objects.get(key)?.clone() ?? new Response(null, { status: 404 });
      });
      vi.stubGlobal("fetch", s3);
      const pending: Promise<unknown>[] = [];
      vi.stubGlobal(Symbol.for("@vercel/request-context"), {
        get: () => ({
          waitUntil: (promise: Promise<unknown>) => {
            pending.push(promise);
          },
        }),
      });
      const { JSDOM } = await import("jsdom");
      const { inProcessExtractor } =
        await import("@nebulamail/mail-render/extract");
      const secret = "p".repeat(40);
      const proxy = proxyWith({
        env: {
          ...env,
          CRON_SECRET,
          IMAGE_CACHE_S3_ACCESS_KEY_ID: "AKID",
          IMAGE_CACHE_S3_BUCKET: "images",
          IMAGE_CACHE_S3_ENDPOINT: "https://s3.example.net",
          IMAGE_CACHE_S3_SECRET_ACCESS_KEY: "secret",
          IMAGE_CACHE_STORAGE: "s3",
          IMAGE_PRELOAD_FULL_EMAIL: "true",
          IMAGE_PRELOAD_SECRET: secret,
        },
        htmlExtractor: inProcessExtractor(new new JSDOM("").window.DOMParser()),
      });
      const url = "http://images.example.com/a.png";
      const response = await proxy.fetch(
        rewritten("hooks/stalwart", {
          body: JSON.stringify({
            context: { stage: "DATA" },
            envelope: { to: [{ address: "alice@example.org" }] },
            message: {
              contents: `<p><img src="${url}"></p>`,
              headers: [["Content-Type", "text/html"]],
            },
          }),
          headers: {
            authorization: `Bearer ${secret}`,
            "content-type": "application/json",
          },
          method: "POST",
        })
      );
      await expect(response.json()).resolves.toStrictEqual({
        action: "accept",
      });
      expect(pending).toHaveLength(1);
      await Promise.all(pending);
      expect(dialed).toContain(PUBLIC);
      // One image under img/; usage and references outside it.
      expect(
        [...objects.keys()]
          .map((name) => name.replaceAll(/[0-9a-f]{64}/gu, "<hash>"))
          .toSorted()
      ).toStrictEqual([
        "/images/img/v1/<hash>/<hash>",
        "/images/pre/v1/refs/<hash>",
        "/images/pre/v1/usage/<hash>",
      ]);
      const served = await proxy.fetch(rewritten(`i/${await sign(url)}`));
      expect(served.status).toBe(200);
      vi.unstubAllGlobals();
    });

    it("validates like the Node env and never echoes values", async () => {
      const { message, status } = await misconfigured({
        env: { APP_URL: "ftp://secret-value", IMAGE_PROXY_KEYS: KEY_1 },
      });
      expect(status).toBe(500);
      expect(message).toMatch(/APP_URL/u);
      expect(message).not.toContain("secret-value");
    });
  });

  describe("ssrf vectors through the Vercel function", () => {
    it.each(BLOCKED_VECTORS)("$id", async (vector) => {
      const { dns } = vector;
      const proxy = createVercelProxyForTests({
        env,
        guardedFetch: createGuardedFetchForTests({
          dial: (address) => {
            dialed.push(address);
            return { host: "127.0.0.1", port: 1 };
          },
          resolver: async () => {
            await Promise.resolve();
            if (dns === undefined) {
              throw new Error("ENOTFOUND");
            }
            return { A: dns.A ?? [], AAAA: dns.AAAA ?? [] };
          },
        }),
        rateLimit: false,
      });
      const outcome = await runBlockedVector(
        await keys(),
        vector,
        async (request) => await proxy.fetch(request)
      );
      expect(outcome).toStrictEqual(expectedForBlocked(vector));
      expect(dialed).toStrictEqual([]);
    });
  });
});
