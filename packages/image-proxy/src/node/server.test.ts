// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// End to end: the Node proxy on a random port, real net-guard, and a local
// image server. net-guard blocks 127.0.0.1 by design, so the guarded fetch
// comes from net-guard's test-only `dial` hook: names resolve (fake resolver)
// to public addresses, the guard validates them as usual, and only the TCP
// connect of a validated address is redirected to the local server.

import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { z } from "zod";

// Test-only hook, deliberately not in net-guard's package exports.
import { createGuardedFetchForTests } from "../../../net-guard/src/node/testing";
import { APP_ORIGIN, gif, KEY_1, keys, png, text } from "../test-fixtures";
import { signTicket } from "../tickets/index";
import type { TicketKeys } from "../tickets/index";
import { createNodeProxy } from "./index";
import { createNodeProxyForTests } from "./testing";

const PUBLIC = "93.184.215.14";
const PACKAGE_DIR = new URL("../..", import.meta.url).pathname;
const LISTENING = /listening on http:\/\/127\.0\.0\.1:(?<port>\d+)/u;

const Images = z.object({
  images: z.record(z.string(), z.record(z.string(), z.unknown())),
});

const portOf = (server: Server): number => {
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("server is not listening on TCP");
  }
  return address.port;
};

const listen = async (
  handler: (req: IncomingMessage, res: ServerResponse) => void
): Promise<Server> => {
  const server = createServer(handler);
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return server;
};

const imagesOf = async (pending: Promise<Response>) => {
  const response = await pending;
  const body = Images.parse(await response.json());
  return body.images;
};

const statusOf = async (pending: Promise<Response>): Promise<number> => {
  const response = await pending;
  return response.status;
};

const postBatch = async (base: string, body: unknown, origin = APP_ORIGIN) =>
  await fetch(`${base}/batch`, {
    body: JSON.stringify(body),
    headers: { "content-type": "application/json", origin },
    method: "POST",
  });

const routes: Record<string, [string, Uint8Array] | { location: string }> = {
  "/a.png": ["image/png", png()],
  "/b.gif": ["image/gif", gif()],
  "/page.png": ["text/html", text("<html></html>")],
  "/to-private": { location: "http://internal.example/x.png" },
  "/x.svg": [
    "image/svg+xml",
    text("<svg xmlns='http://www.w3.org/2000/svg'/>"),
  ],
};

const serveImages = (req: IncomingMessage, res: ServerResponse): void => {
  const route = routes[req.url ?? ""];
  if (route === undefined) {
    res.writeHead(404).end();
  } else if ("location" in route) {
    res.writeHead(302, { location: route.location }).end();
  } else {
    res.writeHead(200, { "content-type": route[0] }).end(route[1]);
  }
};

interface RawAnswer {
  readonly status: number;
  readonly body: string;
  readonly elapsedMs: number;
}

/**
 * A chunked POST written piece by piece with pauses (like a slow sender).
 * With `end: false` the client stops after the last piece and never ends
 * the request: the answer must still arrive.
 */
const chunkedPost = async (
  url: string,
  headers: Record<string, string>,
  pieces: readonly Uint8Array[],
  end: boolean
): Promise<RawAnswer> => {
  const started = Date.now();
  const { promise, reject, resolve } = Promise.withResolvers<RawAnswer>();
  const req = httpRequest(url, {
    headers: { ...headers, "transfer-encoding": "chunked" },
    method: "POST",
  });
  req.on("error", reject);
  req.on("response", (res) => {
    let body = "";
    res.setEncoding("utf-8");
    res.on("data", (chunk: string) => {
      body += chunk;
    });
    res.on("end", () => {
      resolve({
        body,
        elapsedMs: Date.now() - started,
        status: res.statusCode ?? 0,
      });
      req.destroy();
    });
    res.on("error", reject);
  });
  req.flushHeaders();
  for (const piece of pieces) {
    req.write(piece);
    // A pause between pieces, so they arrive as separate chunks.
    // oxlint-disable-next-line no-await-in-loop
    await sleep(30);
  }
  if (end) {
    req.end();
  }
  return await promise;
};

/**
 * Like Stalwart's MTA Hook: a raw HTTP/1.1 POST with Content-Length that
 * sends the whole body before it reads anything. Resolves with what came
 * back and whether the connection ended cleanly or with an error (reset).
 */
const sendAllThenRead = async (
  port: string,
  target: string,
  headers: Record<string, string>,
  size: number
): Promise<{ readonly answer: string; readonly error: string | null }> => {
  const { promise, resolve } = Promise.withResolvers<{
    answer: string;
    error: string | null;
  }>();
  const socket = connect(Number(port), "127.0.0.1");
  let answer = "";
  socket.setEncoding("latin1");
  let settle: ReturnType<typeof setTimeout> | undefined;
  let flushed = false;
  const check = (): void => {
    const end = answer.indexOf("\r\n\r\n");
    const length = /content-length: (?<n>\d+)/iu.exec(answer)?.groups?.n;
    if (
      settle === undefined &&
      flushed &&
      end !== -1 &&
      length !== undefined &&
      answer.length >= end + 4 + Number(length)
    ) {
      // The whole body went out and the whole answer is here. A server that
      // keeps the connection (Deno) is fine; one that resets it does so
      // right away.
      settle = setTimeout(() => {
        resolve({ answer, error: null });
        socket.destroy();
      }, 300);
    }
  };
  socket.on("data", (chunk: string) => {
    answer += chunk;
    check();
  });
  socket.on("error", (error: NodeJS.ErrnoException) => {
    clearTimeout(settle);
    resolve({ answer, error: error.code ?? "error" });
  });
  socket.on("close", () => {
    clearTimeout(settle);
    resolve({ answer, error: flushed ? null : "closed-before-sent" });
  });
  const head = Object.entries({
    ...headers,
    "content-length": String(size),
    host: "proxy",
  })
    .map(([name, value]) => `${name}: ${value}\r\n`)
    .join("");
  socket.write(`POST ${target} HTTP/1.1\r\n${head}\r\n`);
  const block = Buffer.alloc(64 * 1024, 0x78);
  let sent = 0;
  const onSent = (): void => {
    flushed = true;
    check();
  };
  const pump = (): void => {
    while (sent < size) {
      const n = Math.min(block.length, size - sent);
      sent += n;
      if (!socket.write(block.subarray(0, n))) {
        socket.once("drain", pump);
        return;
      }
    }
    // Writes go out in order: this callback runs once everything is sent.
    socket.write("", onSent);
  };
  pump();
  return await promise;
};

const piece = (size: number): Uint8Array => text("x".repeat(size));

const IS_BUN = process.versions.bun !== undefined;
const IS_DENO = Reflect.get(globalThis, "Deno") !== undefined;

/**
 * `src/node/server.ts` the way each runtime starts it: Node with the `.ts`
 * import hook (`bun run dev:proxy`, without --watch), Bun directly, and Deno
 * through `scripts/start-deno.sh` with its minimal permissions.
 */
const devServerCommand = (): readonly [string, string[]] => {
  if (IS_BUN) {
    return [process.execPath, ["src/node/server.ts"]];
  }
  if (IS_DENO) {
    return ["sh", ["scripts/start-deno.sh"]];
  }
  return [
    process.execPath,
    ["--import", "./scripts/node-resolve-ts.ts", "src/node/server.ts"],
  ];
};

const startDevServer = (env: Record<string, string>) => {
  const [command, args] = devServerCommand();
  const child = spawn(command, args, {
    cwd: PACKAGE_DIR,
    env: {
      PATH: process.env.PATH ?? "",
      ...(IS_DENO ? { DENO: process.execPath } : {}),
      ...env,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const collect = (chunk: Buffer) => {
    output += chunk.toString();
  };
  child.stdout.on("data", collect);
  child.stderr.on("data", collect);
  return { child, output: () => output };
};

const waitForPort = async (output: () => string): Promise<string> => {
  for (let i = 0; i < 100; i += 1) {
    const port = LISTENING.exec(output())?.groups?.port;
    if (port !== undefined) {
      return port;
    }
    // Polling the child's output until it reports its port.
    // oxlint-disable-next-line no-await-in-loop
    await sleep(50);
  }
  throw new Error("dev server did not start");
};

describe("node proxy end to end", () => {
  const hits: string[] = [];
  const dialed: number[] = [];
  let images: Server;
  let proxyServer: Server;
  let proxyUrl = "";
  let cacheDir = "";
  let k: TicketKeys;

  const sign = async (url: string, accountId = "acc1"): Promise<string> =>
    await signTicket(k, {
      accountId,
      emailId: "mail1",
      expiresAt: Date.now() + 60_000,
      url,
    });

  beforeAll(async () => {
    images = await listen((req, res) => {
      hits.push(req.url ?? "");
      serveImages(req, res);
    });
    const closed = await listen(() => {
      // Never used: closed right away to get a port nobody listens on.
    });
    const closedPort = portOf(closed);
    closed.close();

    k = await keys();
    cacheDir = await mkdtemp(path.join(tmpdir(), "image-proxy-e2e-"));
    const guardedFetch = createGuardedFetchForTests({
      dial: (_address, port) => {
        dialed.push(port);
        // Port 80 reaches the image server; 443 is "closed" (refused).
        return {
          host: "127.0.0.1",
          port: port === 80 ? portOf(images) : closedPort,
        };
      },
      resolver: async (host) => {
        await Promise.resolve();
        if (host === "images.example.com") {
          return { A: [PUBLIC], AAAA: [] };
        }
        if (host === "internal.example") {
          return { A: ["10.0.0.5"], AAAA: [] };
        }
        throw new Error("ENOTFOUND");
      },
    });
    const proxy = createNodeProxyForTests({
      appOrigin: APP_ORIGIN,
      cacheDir,
      guardedFetch,
      keys: k,
    });
    proxyServer = await listen(proxy.listener);
    proxyUrl = `http://127.0.0.1:${portOf(proxyServer)}`;
  });

  afterAll(async () => {
    images.close();
    proxyServer.close();
    await rm(cacheDir, { force: true, recursive: true });
  });

  const buildForTests = () =>
    createNodeProxyForTests({
      appOrigin: APP_ORIGIN,
      guardedFetch: async () =>
        await Promise.resolve({ ok: false, reason: "dns" } as const),
      keys: k,
    });

  it("batch mode: http URL tries https (refused), falls back to http", async () => {
    dialed.length = 0;
    const t = await sign("http://images.example.com/a.png");
    const response = await postBatch(proxyUrl, { mode: "batch", tickets: [t] });
    expect(response.headers.get("access-control-allow-origin")).toBe(
      APP_ORIGIN
    );
    const { images: results } = Images.parse(await response.json());
    expect(results[t]?.type).toBe("image/png");
    expect(Buffer.from(String(results[t]?.data), "base64")).toStrictEqual(
      Buffer.from(png())
    );
    expect(dialed).toStrictEqual([443, 80]);
  });

  it("cache mode: POST downloads and stores", async () => {
    const t = await sign("http://images.example.com/b.gif");
    const results = await imagesOf(postBatch(proxyUrl, { tickets: [t] }));
    expect(results[t]).toStrictEqual({ cached: true });
  });

  it("cache mode: GET serves from disk without touching upstream", async () => {
    const t = await sign("http://images.example.com/b.gif");
    await postBatch(proxyUrl, { tickets: [t] });
    const before = hits.length;
    dialed.length = 0;
    const response = await fetch(`${proxyUrl}/i/${t}`);
    expect({
      csp: response.headers.get("content-security-policy"),
      corp: response.headers.get("cross-origin-resource-policy"),
      cookie: response.headers.get("set-cookie"),
      status: response.status,
      type: response.headers.get("content-type"),
    }).toStrictEqual({
      cookie: null,
      corp: "cross-origin",
      csp: "default-src 'none'; style-src 'unsafe-inline'; sandbox",
      status: 200,
      type: "image/gif",
    });
    expect(new Uint8Array(await response.arrayBuffer())).toStrictEqual(gif());
    expect(hits).toHaveLength(before);
    expect(dialed).toStrictEqual([]);
  });

  it("GET of an image never downloaded is 404 and contacts nobody", async () => {
    dialed.length = 0;
    const t = await sign("http://images.example.com/a.png", "accNever");
    await expect(statusOf(fetch(`${proxyUrl}/i/${t}`))).resolves.toBe(404);
    expect(dialed).toStrictEqual([]);
  });

  it("redirect to a private address, SVG and HTML are refused", async () => {
    const tickets = await Promise.all(
      ["/to-private", "/x.svg", "/page.png"].map(
        async (pathname) => await sign(`http://images.example.com${pathname}`)
      )
    );
    const results = await imagesOf(
      postBatch(proxyUrl, { mode: "batch", tickets })
    );
    expect(tickets.map((t) => results[t])).toStrictEqual([
      { error: "blocked" },
      { error: "not-image" },
      { error: "not-image" },
    ]);
  });

  it("CORS preflight and wrong origin over HTTP", async () => {
    const preflight = fetch(`${proxyUrl}/batch`, {
      headers: {
        "access-control-request-headers": "content-type",
        "access-control-request-method": "POST",
        origin: APP_ORIGIN,
      },
      method: "OPTIONS",
    });
    await expect(statusOf(preflight)).resolves.toBe(204);
    const wrong = postBatch(proxyUrl, {}, "https://evil.example");
    await expect(statusOf(wrong)).resolves.toBe(403);
  });

  it("healthz reports both modes", async () => {
    const response = await fetch(`${proxyUrl}/healthz`);
    await expect(response.json()).resolves.toStrictEqual({
      modes: ["batch", "cache"],
      status: "ok",
    });
  });

  it("production factory has no way to replace the guarded fetch", () => {
    const guardedFetch = createGuardedFetchForTests({
      dial: () => ({ host: "127.0.0.1", port: 1 }),
    });
    const options = { appOrigin: APP_ORIGIN, guardedFetch, keys: k };
    // @ts-expect-error `guardedFetch` is not an option of createNodeProxy.
    const build = () => createNodeProxy({ ...options, guardedFetch });
    expect(build).not.toThrow();
  });

  it("the test factory refuses to run outside Vitest", () => {
    const saved = process.env.VITEST;
    process.env.VITEST = "false";
    try {
      expect(buildForTests).toThrow(/only available in tests/u);
    } finally {
      process.env.VITEST = saved;
    }
  });
});

const RUNTIME_NAME = IS_DENO ? "Deno" : "Node";

describe(`dev server process (src/node/server.ts on ${IS_BUN ? "Bun" : RUNTIME_NAME})`, () => {
  it("starts, answers, and blocks a private image URL with the real guard", async () => {
    const k = await keys();
    const { child, output } = startDevServer({
      APP_URL: APP_ORIGIN,
      IMAGE_PROXY_KEYS: KEY_1,
      IMAGE_PROXY_PORT: "0",
    });
    try {
      const base = `http://127.0.0.1:${await waitForPort(output)}`;
      expect(output()).toContain("(modes: batch)");
      await expect(statusOf(fetch(`${base}/healthz`))).resolves.toBe(200);
      const t = await signTicket(k, {
        accountId: "acc1",
        emailId: "mail1",
        expiresAt: Date.now() + 60_000,
        url: "http://127.0.0.1/secret.png",
      });
      const results = await imagesOf(postBatch(base, { tickets: [t] }));
      expect(results[t]).toStrictEqual({ error: "blocked" });
    } finally {
      child.kill();
    }
  }, 15_000);

  it("IMAGE_PROXY_CLIENT_IP=x-forwarded-for: one bucket per client behind the proxy", async () => {
    const { child, output } = startDevServer({
      APP_URL: APP_ORIGIN,
      IMAGE_PROXY_CLIENT_IP: "x-forwarded-for",
      IMAGE_PROXY_KEYS: KEY_1,
      IMAGE_PROXY_PORT: "0",
    });
    try {
      const base = `http://127.0.0.1:${await waitForPort(output)}`;
      expect(output()).toContain("trusting x-forwarded-for");
      const from = async (forwarded: string): Promise<number> => {
        const response = await fetch(`${base}/batch`, {
          body: "{}",
          headers: {
            "content-type": "application/json",
            origin: APP_ORIGIN,
            // The client's own claim first, the proxy's entry last.
            "x-forwarded-for": `198.51.100.7, ${forwarded}`,
          },
          method: "POST",
        });
        await response.body?.cancel();
        return response.status;
      };
      // `batch:ip` allows bursts of 30 and refills 0.5/s: on a slow machine
      // a token or two may come back while the burst is sent.
      const statuses: number[] = [];
      while (statuses.length < 40 && !statuses.includes(429)) {
        // One after another, to drain the bucket in order.
        // oxlint-disable-next-line no-await-in-loop
        statuses.push(await from("203.0.113.1"));
      }
      expect(statuses.at(-1)).toBe(429);
      expect(statuses.length).toBeGreaterThan(30);
      await expect(from("203.0.113.2")).resolves.not.toBe(429);
    } finally {
      child.kill();
    }
  }, 30_000);

  describe("bodies over the limit get an answer, not a reset", () => {
    const SECRET = "s".repeat(40);
    let cache = "";
    let base = "";
    let server: ReturnType<typeof startDevServer> | undefined;

    beforeAll(async () => {
      cache = await mkdtemp(path.join(tmpdir(), "image-proxy-body-"));
      server = startDevServer({
        APP_URL: APP_ORIGIN,
        IMAGE_CACHE_DIR: cache,
        IMAGE_PRELOAD_FULL_EMAIL: "true",
        IMAGE_PRELOAD_MAX_MESSAGE_BYTES: "2000",
        IMAGE_PRELOAD_SECRET: SECRET,
        IMAGE_PROXY_KEYS: KEY_1,
        IMAGE_PROXY_PORT: "0",
      });
      base = `http://127.0.0.1:${await waitForPort(server.output)}`;
    }, 15_000);

    afterAll(async () => {
      server?.child.kill();
      await rm(cache, { force: true, recursive: true });
    });

    it("POST /hooks/stalwart over IMAGE_PRELOAD_MAX_MESSAGE_BYTES: accept", async () => {
      const answer = await chunkedPost(
        `${base}/hooks/stalwart`,
        {
          authorization: `Bearer ${SECRET}`,
          "content-type": "application/json",
        },
        [piece(1500), piece(1500), piece(1500)],
        true
      );
      expect(answer.status).toBe(200);
      expect(JSON.parse(answer.body)).toStrictEqual({ action: "accept" });
    }, 15_000);

    it("POST /batch over its body limit: 413", async () => {
      // 60 tickets of up to 8 KB each (~490 KB): 800 KB is over.
      const answer = await chunkedPost(
        `${base}/batch`,
        { "content-type": "application/json", origin: APP_ORIGIN },
        [piece(200_000), piece(200_000), piece(200_000), piece(200_000)],
        true
      );
      expect(answer.status).toBe(413);
    }, 15_000);

    it("a sender that writes everything before reading gets the answer, not a reset", async () => {
      const { port } = new URL(base);
      const hook = await sendAllThenRead(
        port,
        "/hooks/stalwart",
        {
          authorization: `Bearer ${SECRET}`,
          "content-type": "application/json",
        },
        4 * 1024 * 1024
      );
      expect(hook.error).toBeNull();
      expect(hook.answer).toMatch(/^HTTP\/1\.1 200 /u);
      expect(hook.answer).toContain('{"action":"accept"}');
      const batch = await sendAllThenRead(
        port,
        "/batch",
        { "content-type": "application/json", origin: APP_ORIGIN },
        4 * 1024 * 1024
      );
      expect(batch.error).toBeNull();
      expect(batch.answer).toMatch(/^HTTP\/1\.1 413 /u);
    }, 15_000);

    it("a sender that stops past the limit still gets the answer at once", async () => {
      const answer = await chunkedPost(
        `${base}/hooks/stalwart`,
        {
          authorization: `Bearer ${SECRET}`,
          "content-type": "application/json",
        },
        [piece(1500), piece(1500)],
        false
      );
      expect(answer.status).toBe(200);
      expect(JSON.parse(answer.body)).toStrictEqual({ action: "accept" });
      // Far below requestTimeout (60 s) and the drain limit (10 s).
      expect(answer.elapsedMs).toBeLessThan(5000);
    }, 15_000);
  });

  it("fails fast on bad config without echoing secrets", async () => {
    const { child, output } = startDevServer({
      APP_URL: APP_ORIGIN,
      IMAGE_PROXY_KEYS: "1:top-secret",
    });
    await once(child, "exit");
    expect(child.exitCode).toBe(1);
    expect(output()).toContain("image proxy keys");
    expect(output()).not.toContain("top-secret");
  });
});
