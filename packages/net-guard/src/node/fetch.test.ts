// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `fetch` vectors from security/ssrf-vectors.json against real node:http
// servers on 127.0.0.1. The guard blocks 127.0.0.1 by design, so tests use the
// test-only `dial` hook: hostnames resolve (fake resolver) to public-looking
// IPs, the guard validates them as usual, and only the final TCP connect for
// a validated IP is redirected to the local server. `dial` also records which
// validated IP was dialed.
//
// Under Bun (`bun run test:bun`) `createGuardedFetch` is the Bun adapter, so
// this same suite runs against it; its own TLS, IPv6 and proxy tests are in
// `../bun/fetch.test.ts`. Under Deno (`test:deno`) it runs the Node adapter
// on Deno's `node:` layer and the real undici.

import { Resolver as DnsResolver } from "node:dns/promises";
import { once } from "node:events";
import { createServer } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import { createServer as createTlsServer } from "node:tls";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

import {
  EnvHttpProxyAgent,
  getGlobalDispatcher,
  setGlobalDispatcher,
} from "undici";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import { isBun } from "../bun/fetch";
import {
  fakeDns as fakeDnsServer,
  withoutResolverCancel,
} from "../http/fake-dns";
import type { FakeDns, FakeDnsOptions } from "../http/fake-dns";
import { createFetchBudget } from "../index";
import type { DnsAnswer, Resolver } from "../index";
import { fail, later, SAMPLES } from "../test-fixtures";
import type { FetchLimits, FetchResult } from "./fetch";
import { guardedFetch } from "./index";
import { createGuardedFetchForTests } from "./testing";

const PUBLIC = "93.184.215.14";
const PUBLIC_DEAD = "93.184.215.99";
const MB = 1024 * 1024;

const routes = new Map<
  string,
  (req: IncomingMessage, res: ServerResponse) => void
>();
const seen: IncomingMessage[] = [];
let server: Server;
let port = 0;
let tlsPort = 0;
const sniSeen: string[] = [];
let tlsServer: ReturnType<typeof createTlsServer>;

const portOf = (s: Server | ReturnType<typeof createTlsServer>): number => {
  const address = s.address();
  if (address === null || typeof address === "string") {
    throw new Error("server is not listening on TCP");
  }
  return address.port;
};

/** Narrow to a successful result (fails the test otherwise). */
const okResult = (result: FetchResult) => {
  if (!result.ok) {
    throw new Error(`unexpected block: ${result.reason}`);
  }
  return result;
};

const listen = async (handler: Parameters<typeof createServer>[1]) => {
  const s = createServer(handler);
  s.listen(0, "127.0.0.1");
  await once(s, "listening");
  return s;
};

const route = (
  path: string,
  handler: (req: IncomingMessage, res: ServerResponse) => void
) => {
  routes.set(path, handler);
};

const send =
  (
    type: string,
    body: Uint8Array | string,
    extra: Record<string, string> = {}
  ) =>
  (_req: IncomingMessage, res: ServerResponse) => {
    res
      .writeHead(200, { "content-type": type, "set-cookie": "a=b", ...extra })
      .end(body);
  };

const redirectTo =
  (location: string, status = 302) =>
  (_req: IncomingMessage, res: ServerResponse) => {
    res.writeHead(status, { location }).end();
  };

const streamBytes =
  (total: number) => (_req: IncomingMessage, res: ServerResponse) => {
    res.writeHead(200, { "content-type": "image/png" });
    const chunk = Buffer.alloc(64 * 1024, 1);
    let sent = 0;
    const pump = () => {
      while (sent < total) {
        sent += chunk.length;
        if (!res.write(chunk)) {
          res.once("drain", pump);
          return;
        }
      }
      res.end();
    };
    res.on("error", () => {
      // Client went away on purpose.
    });
    pump();
  };

/**
 * `Connection: keep-alive` from undici; Bun sends none (the adapter asks
 * for a new connection every time).
 */
const CONNECTION = isBun() ? [] : ["connection"];

const DNS: Readonly<Record<string, DnsAnswer>> = {
  "dual.test": { A: [PUBLIC_DEAD, PUBLIC], AAAA: [] },
  "img.test": { A: [PUBLIC], AAAA: [] },
  localhost: { A: ["127.0.0.1"], AAAA: ["::1"] },
  "metadata.google.internal": { A: ["169.254.169.254"], AAAA: [] },
};

const fakeDns: Resolver = async (hostname) => {
  const answer = DNS[hostname];
  if (answer === undefined) {
    return await fail();
  }
  return await later(answer);
};

interface Harness {
  readonly fetch: ReturnType<typeof createGuardedFetchForTests>;
  readonly dialed: string[];
  readonly lookups: string[];
}

const harness = (
  options: { resolver?: Resolver; limits?: Partial<FetchLimits> } = {}
): Harness => {
  const dialed: string[] = [];
  const lookups: string[] = [];
  const resolver = options.resolver ?? fakeDns;
  const fetch = createGuardedFetchForTests({
    dial: (address, targetPort) => {
      dialed.push(`${address}:${targetPort}`);
      // Only the "public" test IP reaches the local server; anything else
      // goes to a closed port (connection refused).
      if (address !== PUBLIC) {
        return { host: "127.0.0.1", port: 1 };
      }
      return { host: "127.0.0.1", port: targetPort === 443 ? tlsPort : port };
    },
    ...(options.limits === undefined ? {} : { limits: options.limits }),
    resolver: async (hostname) => {
      lookups.push(hostname);
      return await resolver(hostname);
    },
  });
  return { dialed, fetch, lookups };
};

/** Where the harness dials a validated address (only `PUBLIC` reaches the server). */
const dialTo = (dialed: string[]) => (address: string, targetPort: number) => {
  dialed.push(`${address}:${targetPort}`);
  if (address !== PUBLIC) {
    return { host: "127.0.0.1", port: 1 };
  }
  return { host: "127.0.0.1", port: targetPort === 443 ? tlsPort : port };
};

/**
 * Point every c-ares `Resolver` (the adapter's real default resolver,
 * `new Resolver()` inside `createCancelableResolver`) at a fake DNS server.
 * `setServers` runs once per instance, before its first query: calling it
 * while a query of that instance is pending throws in Node.
 */
const routeResolversTo = (fake: FakeDns) => {
  const configured = new WeakSet<DnsResolver>();
  const configure = (dns: DnsResolver) => {
    if (!configured.has(dns)) {
      configured.add(dns);
      dns.setServers([fake.server]);
    }
  };
  const { resolve4, resolve6 } = DnsResolver.prototype;
  const spy4 = vi
    .spyOn(DnsResolver.prototype, "resolve4")
    .mockImplementation(async function resolve4Spy(
      this: DnsResolver,
      hostname: string
    ) {
      configure(this);
      // `ttl: false` is the default; it picks the `string[]` overload.
      return await resolve4.call(this, hostname, { ttl: false });
    });
  const spy6 = vi
    .spyOn(DnsResolver.prototype, "resolve6")
    .mockImplementation(async function resolve6Spy(
      this: DnsResolver,
      hostname: string
    ) {
      configure(this);
      // `ttl: false` is the default; it picks the `string[]` overload.
      return await resolve6.call(this, hostname, { ttl: false });
    });
  return { spy4, spy6 };
};

/** A harness on the adapter's default resolver (no `resolver` option). */
const defaultHarness = (limits?: Partial<FetchLimits>) => {
  const dialed: string[] = [];
  const fetch = createGuardedFetchForTests({
    dial: dialTo(dialed),
    ...(limits === undefined ? {} : { limits }),
  });
  return { dialed, fetch };
};

/**
 * The fake got A and AAAA once each, and nothing more after a try's timeout
 * (2 s): without `cancel()` the retries would go out. Sorted: the two
 * families race.
 */
const queriesAfterATry = async (dns: FakeDns) => {
  // Datagrams already sent may still be on their way to the fake.
  await sleep(300);
  const first = dns.queries.map((query) => `${query.type} ${query.name}`);
  await sleep(2500);
  return {
    after: dns.queries.length,
    first: first.toSorted(),
  };
};

const NOTHING_LEFT_RUNNING = {
  after: 2,
  first: ["A hung.img.test", "AAAA hung.img.test"],
};

describe(`pinned adapter (${isBun() ? "Bun" : "Node"})`, () => {
  beforeAll(async () => {
    server = await listen((req, res) => {
      seen.push(req);
      const path = new URL(req.url ?? "/", "http://x").pathname;
      const handler = routes.get(path);
      if (handler === undefined) {
        res.writeHead(404).end();
        return;
      }
      handler(req, res);
    });
    port = portOf(server);
    // TLS server with no certificate: it records the SNI and aborts the handshake.
    tlsServer = createTlsServer({
      // Node's SNICallback is a callback API; we only record the name and refuse.
      /* oxlint-disable promise/prefer-await-to-callbacks */
      SNICallback: (servername, callback) => {
        sniSeen.push(servername);
        callback(new Error("no certificate"));
      },
      /* oxlint-enable promise/prefer-await-to-callbacks */
    });
    tlsServer.on("tlsClientError", () => {
      // Expected: every handshake is refused.
    });
    tlsServer.listen(0, "127.0.0.1");
    await once(tlsServer, "listening");
    tlsPort = portOf(tlsServer);
  });

  beforeEach(() => {
    seen.length = 0;
  });

  afterAll(() => {
    server.closeAllConnections();
    server.close();
    tlsServer.close();
  });

  describe("happy path", () => {
    it("fetches an image with generic headers and pinned IP", async () => {
      route("/a.png", send("image/png", SAMPLES.png));
      const { fetch, dialed } = harness();
      const result = await fetch("http://img.test/a.png?x=1#frag", {
        expect: "image",
      });

      expect(result).toMatchObject({
        imageType: "image/png",
        ok: true,
        redirects: 0,
        status: 200,
        url: "http://img.test/a.png?x=1",
      });
      const { body, headers } = okResult(result);
      expect(Buffer.from(body)).toStrictEqual(Buffer.from(SAMPLES.png));
      expect(headers["set-cookie"]).toBeUndefined();
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });

    it("sends only generic headers: no cookies, referrer or credentials", async () => {
      route("/a.png", send("image/png", SAMPLES.png));
      const { fetch } = harness();
      okResult(await fetch("http://img.test/a.png", { expect: "image" }));
      const [req] = seen;
      expect(req?.headers).toMatchObject({
        "accept-encoding": "identity",
        host: "img.test",
      });
      expect(req?.headers.accept).toContain("image/webp");
      expect(req?.headers["user-agent"]).toMatch(/^Mozilla\/5\.0 /u);
      const names = Object.keys(req?.headers ?? {}).toSorted();
      expect(names).toStrictEqual([
        "accept",
        "accept-encoding",
        ...CONNECTION,
        "host",
        "user-agent",
      ]);
    });

    it("tries the next validated address if one is unreachable", async () => {
      route("/a.png", send("image/png", SAMPLES.png));
      const { fetch, dialed } = harness();
      const result = await fetch("http://dual.test/a.png");
      expect(result).toMatchObject({ ok: true, status: 200 });
      expect(dialed).toStrictEqual([`${PUBLIC_DEAD}:80`, `${PUBLIC}:80`]);
    });

    it("returns non-2xx statuses without the body", async () => {
      const { fetch } = harness();
      const result = await fetch("http://img.test/missing", {
        expect: "image",
      });
      expect(result).toMatchObject({ ok: true, status: 404 });
      expect(okResult(result).body.byteLength).toBe(0);
    });

    it("reports unreachable servers as network", async () => {
      const { fetch } = harness({
        resolver: async () => await later({ A: [PUBLIC_DEAD], AAAA: [] }),
      });
      await expect(fetch("http://img.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
    });
  });

  describe("https", () => {
    it("dials the validated IP but sends the hostname as SNI and verifies the certificate", async () => {
      sniSeen.length = 0;
      const { fetch, dialed } = harness();
      // The handshake fails (no certificate), so the result is a network error.
      await expect(fetch("https://img.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      expect(dialed).toStrictEqual([`${PUBLIC}:443`]);
      expect(sniSeen).toStrictEqual(["img.test"]);
    });
  });

  describe("vector: rebinding", () => {
    it("resolves once and connects to the first validated IP", async () => {
      route("/a.png", send("image/png", SAMPLES.png));
      // Vector dnsSequence: [["93.184.215.14"], ["127.0.0.1"]].
      const sequence = [[PUBLIC], ["127.0.0.1"]];
      let calls = 0;
      const { fetch, dialed, lookups } = harness({
        resolver: async () => {
          const A = sequence[Math.min(calls, sequence.length - 1)] ?? [];
          calls += 1;
          return await later({ A, AAAA: [] });
        },
      });

      const first = await fetch("http://rebind.test/a.png", {
        expect: "image",
      });
      expect(first).toMatchObject({ ok: true, status: 200 });
      expect(lookups).toStrictEqual(["rebind.test"]);
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);

      // The next request resolves again, gets the private answer and is blocked
      // before any connection is attempted.
      const second = await fetch("http://rebind.test/a.png", {
        expect: "image",
      });
      expect(second).toStrictEqual({ ok: false, reason: "private-ip" });
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });
  });

  describe("vector: redirects", () => {
    it.each([
      ["http://127.0.0.1/", "private-ip"],
      ["http://[::1]/", "private-ip"],
      ["http://2130706433/", "private-ip"],
      ["http://localhost/", "dns"],
      ["http://metadata.google.internal/computeMetadata/v1/", "dns"],
      ["http://169.254.169.254/latest/meta-data/", "private-ip"],
      ["file:///etc/passwd", "scheme"],
      ["gopher://img.test:6379/_INFO", "scheme"],
      ["http://img.test:8080/", "port"],
      ["http://user:pass@img.test/", "userinfo"],
      ["http://nxdomain.test/", "dns"],
    ])("blocks a redirect to %s (%s)", async (location, reason) => {
      route("/go", redirectTo(location));
      const { fetch, dialed } = harness();
      await expect(fetch("http://img.test/go")).resolves.toStrictEqual({
        ok: false,
        reason,
      });
      // Only the first, public hop was dialed.
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });

    it("follows up to 5 redirects (relative Location, every status)", async () => {
      const statuses = [301, 302, 303, 307, 308];
      for (const [i, status] of statuses.entries()) {
        route(`/chain/${i}`, redirectTo(`/chain/${i + 1}`, status));
      }
      route("/chain/5", send("image/gif", SAMPLES.gif));
      const { fetch, lookups } = harness();
      const result = await fetch("http://img.test/chain/0", {
        expect: "image",
      });
      expect(result).toMatchObject({
        imageType: "image/gif",
        ok: true,
        redirects: 5,
        url: "http://img.test/chain/5",
      });
      // The full guard (including DNS) ran on every hop.
      expect(lookups).toHaveLength(6);
    });

    it("blocks the 6th redirect (redirect-loop)", async () => {
      route("/loop", redirectTo("/loop"));
      const { fetch } = harness();
      await expect(fetch("http://img.test/loop")).resolves.toStrictEqual({
        ok: false,
        reason: "too-many-redirects",
      });
      expect(seen).toHaveLength(6);
    });
  });

  describe("caller rules: allowRedirect and budget", () => {
    it("asks allowRedirect about every redirect target, never the first URL", async () => {
      route("/r0", redirectTo("/r1"));
      route("/r1", redirectTo("http://img.test/r2.png"));
      route("/r2.png", send("image/png", SAMPLES.png));
      const asked: string[] = [];
      const { fetch } = harness();
      const result = await fetch("http://img.test/r0", {
        allowRedirect: (url) => {
          asked.push(url);
          return true;
        },
        expect: "image",
      });
      expect(result).toMatchObject({ ok: true, redirects: 2 });
      expect(asked).toStrictEqual([
        "http://img.test/r1",
        "http://img.test/r2.png",
      ]);
    });

    it("stops with refused before dialing a refused target", async () => {
      route("/go", redirectTo("http://dual.test/pixel"));
      const { fetch, dialed, lookups } = harness();
      await expect(
        fetch("http://img.test/go", {
          allowRedirect: (url) => !url.includes("dual.test"),
        })
      ).resolves.toStrictEqual({ ok: false, reason: "refused" });
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
      expect(lookups).toStrictEqual(["img.test"]);
    });

    it("a throwing allowRedirect refuses", async () => {
      route("/go", redirectTo("/next"));
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/go", {
          allowRedirect: () => {
            throw new Error("boom");
          },
        })
      ).resolves.toStrictEqual({ ok: false, reason: "refused" });
    });

    it("every hop takes a request from the budget", async () => {
      route("/b0", redirectTo("/b1"));
      route("/b1", redirectTo("/b2"));
      route("/b2", send("image/png", SAMPLES.png));
      const { fetch } = harness();
      const budget = createFetchBudget({ maxBytes: MB, maxRequests: 2 });
      await expect(
        fetch("http://img.test/b0", { budget, expect: "image" })
      ).resolves.toStrictEqual({ ok: false, reason: "budget" });
      expect(seen).toHaveLength(2);
      expect(budget.remainingRequests()).toBe(0);
      const roomy = createFetchBudget({ maxBytes: MB, maxRequests: 3 });
      okResult(
        await fetch("http://img.test/b0", { budget: roomy, expect: "image" })
      );
      // The body read is taken from the budget.
      expect(roomy.remainingBytes()).toBe(MB - SAMPLES.png.byteLength);
    });

    it("takes the bytes read even when the body is then refused", async () => {
      route("/fake.png", send("image/png", "x".repeat(1000)));
      const { fetch } = harness();
      const budget = createFetchBudget({ maxBytes: MB, maxRequests: 5 });
      await expect(
        fetch("http://img.test/fake.png", { budget, expect: "image" })
      ).resolves.toStrictEqual({ ok: false, reason: "content-type" });
      expect(budget.remainingBytes()).toBe(MB - 1000);
    });

    it("caps the body to the bytes left in the budget", async () => {
      route("/big", streamBytes(256 * 1024));
      const { fetch } = harness();
      const budget = createFetchBudget({ maxBytes: 1000, maxRequests: 5 });
      await expect(
        fetch("http://img.test/big", { budget })
      ).resolves.toStrictEqual({ ok: false, reason: "size" });
      // Reading past what was left exhausts the budget.
      expect(budget.exhausted()).toBeTruthy();
      expect(budget.takeBytes(1000)).toBeFalsy();
      // Exhausted: refused before any request.
      seen.length = 0;
      await expect(
        fetch("http://img.test/big", { budget })
      ).resolves.toStrictEqual({ ok: false, reason: "size" });
      expect(seen).toHaveLength(0);
    });
  });

  describe("vector: not-an-image", () => {
    it.each([
      ["text/html", "<html><script>alert(1)</script>"],
      [
        "image/svg+xml",
        '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>',
      ],
      ["image/png", SAMPLES.jpeg],
      ["image/gif", "<html>"],
      ["application/octet-stream", SAMPLES.png],
    ])("blocks %s with mismatching bytes", async (type, body) => {
      route("/x", send(type, body));
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/x", { expect: "image" })
      ).resolves.toStrictEqual({ ok: false, reason: "content-type" });
    });

    it("enforces allowedContentTypes for non-image fetches", async () => {
      route("/j", send("application/json; charset=utf-8", "{}"));
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/j", {
          allowedContentTypes: ["application/json"],
        })
      ).resolves.toMatchObject({ ok: true });
      await expect(
        fetch("http://img.test/j", { allowedContentTypes: ["application/xml"] })
      ).resolves.toStrictEqual({ ok: false, reason: "content-type" });
    });
  });

  describe("vector: too-large", () => {
    it("blocks a declared Content-Length over 5 MB without reading it", async () => {
      route("/big-declared", (_req, res) => {
        res.writeHead(200, {
          "content-length": String(6 * MB),
          "content-type": "image/png",
        });
        res.write(SAMPLES.png);
      });
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/big-declared", { expect: "image" })
      ).resolves.toStrictEqual({ ok: false, reason: "size" });
    });

    it("blocks a chunked body over 5 MB while streaming", async () => {
      route("/big-chunked", streamBytes(6 * MB));
      const { fetch } = harness();
      await expect(fetch("http://img.test/big-chunked")).resolves.toStrictEqual(
        {
          ok: false,
          reason: "size",
        }
      );
    });

    it("allows exactly 5 MB", async () => {
      route("/five", streamBytes(5 * MB));
      const { fetch } = harness();
      const result = await fetch("http://img.test/five");
      expect(okResult(result).body.byteLength).toBe(5 * MB);
    });

    it("cannot be loosened through options", async () => {
      route("/big-chunked", streamBytes(6 * MB));
      const { fetch } = harness({
        limits: { maxBodyBytes: 1e12, maxRedirects: 99, totalTimeoutMs: 1e9 },
      });
      await expect(fetch("http://img.test/big-chunked")).resolves.toStrictEqual(
        {
          ok: false,
          reason: "size",
        }
      );
    });
  });

  describe("vector: gzip-bomb", () => {
    // 16 MB of zeros: over 3x the limit, ~16 KB gzip and a few bytes of brotli.
    const zeros = Buffer.alloc(16 * MB);
    const gzipBomb = gzipSync(zeros, { level: 9 });
    const brBomb = brotliCompressSync(zeros, {
      params: { [constants.BROTLI_PARAM_QUALITY]: 1 },
    });

    it("bombs are small on the wire", () => {
      expect(gzipBomb.byteLength).toBeLessThan(MB);
      expect(brBomb.byteLength).toBeLessThan(MB);
    });

    it.each([
      ["gzip", gzipBomb],
      ["br", brBomb],
    ])("blocks a %s body that expands beyond 5 MB", async (encoding, body) => {
      route("/bomb", send("image/png", body, { "content-encoding": encoding }));
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/bomb", { expect: "image" })
      ).resolves.toStrictEqual({ ok: false, reason: "size" });
    });

    it("decodes small compressed bodies sent despite Accept-Encoding: identity", async () => {
      route(
        "/gz",
        send("image/png", gzipSync(SAMPLES.png), { "content-encoding": "gzip" })
      );
      const { fetch } = harness();
      const result = await fetch("http://img.test/gz", { expect: "image" });
      expect(result).toMatchObject({ imageType: "image/png", ok: true });
      expect(okResult(result).headers["content-encoding"]).toBeUndefined();
    });

    it("refuses unknown or stacked encodings", async () => {
      const { fetch } = harness();
      route(
        "/weird",
        send("image/png", SAMPLES.png, { "content-encoding": "compress" })
      );
      await expect(fetch("http://img.test/weird")).resolves.toStrictEqual({
        ok: false,
        reason: "content-type",
      });
      route(
        "/stacked",
        send("image/png", gzipSync(gzipSync(SAMPLES.png)), {
          "content-encoding": "gzip, gzip",
        })
      );
      await expect(fetch("http://img.test/stacked")).resolves.toStrictEqual({
        ok: false,
        reason: "content-type",
      });
    });
  });

  describe("vector: slow-loris", () => {
    // The real limit is 10 s; options may only tighten it, which keeps the test fast.
    const limits = { totalTimeoutMs: 800 };

    it("times out a body sent one byte at a time", async () => {
      route("/slow", (_req, res) => {
        res.writeHead(200, { "content-type": "image/png" });
        const timer = setInterval(() => {
          res.write("x");
        }, 100);
        res.on("close", () => {
          clearInterval(timer);
        });
      });
      const { fetch } = harness({ limits });
      const started = performance.now();
      await expect(fetch("http://img.test/slow")).resolves.toStrictEqual({
        ok: false,
        reason: "timeout",
      });
      expect(performance.now() - started).toBeLessThan(3000);
    });

    it("times out a server that never sends headers", async () => {
      route("/hang", () => {
        // Never answer.
      });
      const { fetch } = harness({ limits });
      await expect(fetch("http://img.test/hang")).resolves.toStrictEqual({
        ok: false,
        reason: "timeout",
      });
    });

    it("times out a resolver that never answers", async () => {
      const { fetch } = harness({
        limits,
        resolver: async () => await Promise.withResolvers<never>().promise,
      });
      await expect(fetch("http://img.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "timeout",
      });
    });

    it("reports a caller abort as timeout", async () => {
      route("/hang", () => {
        // Never answer.
      });
      const { fetch } = harness();
      const result = await fetch("http://img.test/hang", {
        signal: AbortSignal.timeout(100),
      });
      expect(result).toStrictEqual({ ok: false, reason: "timeout" });
    });
  });

  describe("vector: env-proxy-ignored", () => {
    it("connects directly even with HTTP(S)_PROXY and a global proxy dispatcher", async () => {
      let proxied = 0;
      const proxy = await listen((_req, res) => {
        proxied += 1;
        res.writeHead(200, { "content-type": "image/png" }).end(SAMPLES.png);
      });
      proxy.on("connect", () => {
        proxied += 1;
      });
      const proxyUrl = `http://127.0.0.1:${portOf(proxy)}`;
      const saved = { ...process.env };
      const previous = getGlobalDispatcher();
      for (const key of [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "http_proxy",
        "https_proxy",
      ]) {
        process.env[key] = proxyUrl;
      }
      setGlobalDispatcher(new EnvHttpProxyAgent());
      try {
        route("/a.png", send("image/png", SAMPLES.png));
        const { fetch, dialed } = harness();
        await expect(
          fetch("http://img.test/a.png", { expect: "image" })
        ).resolves.toMatchObject({ ok: true });
        expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
        expect(seen).toHaveLength(1);
        expect(proxied).toBe(0);
      } finally {
        setGlobalDispatcher(previous);
        process.env = saved;
        proxy.closeAllConnections();
        proxy.close();
      }
    });
  });

  // POST (List-Unsubscribe one-click, RFC 8058): the same
  // guard on every hop, plus the request body.
  describe("POST", () => {
    const FORM = {
      data: "List-Unsubscribe=One-Click",
      type: "application/x-www-form-urlencoded",
    } as const;
    const post = { body: FORM, method: "POST" } as const;
    const received: { method: string; body: string; path: string }[] = [];

    const record =
      (status: number, extra: Record<string, string> = {}) =>
      (req: IncomingMessage, res: ServerResponse) => {
        const chunks: Buffer[] = [];
        req.on("data", (chunk: Buffer) => {
          chunks.push(chunk);
        });
        req.on("end", () => {
          received.push({
            body: Buffer.concat(chunks).toString("utf-8"),
            method: req.method ?? "",
            path: new URL(req.url ?? "/", "http://x").pathname,
          });
          res.writeHead(status, extra).end("thanks");
        });
      };

    beforeEach(() => {
      received.length = 0;
    });

    it("sends the form body with only generic headers", async () => {
      route("/unsub", record(200, { "content-type": "text/html" }));
      const { fetch, dialed } = harness();
      const result = await fetch("http://img.test/unsub?u=1", post);

      expect(result).toMatchObject({ ok: true, status: 200 });
      expect(received).toStrictEqual([
        { body: FORM.data, method: "POST", path: "/unsub" },
      ]);
      const [req] = seen;
      expect(req?.headers["content-type"]).toBe(FORM.type);
      expect(Object.keys(req?.headers ?? {}).toSorted()).toStrictEqual([
        "accept",
        "accept-encoding",
        ...CONNECTION,
        "content-length",
        "content-type",
        "host",
        "user-agent",
      ]);
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });

    it("can skip the response body", async () => {
      route("/unsub", record(202));
      const { fetch } = harness();
      const result = okResult(
        await fetch("http://img.test/unsub", { ...post, readBody: false })
      );
      expect([result.status, result.body.byteLength]).toStrictEqual([202, 0]);
    });

    it.each([
      ["http://127.0.0.1/", "private-ip"],
      ["http://[::ffff:127.0.0.1]/", "private-ip"],
      ["http://localhost/", "dns"],
      ["http://metadata.google.internal/computeMetadata/v1/", "dns"],
      ["http://169.254.169.254/latest/meta-data/", "private-ip"],
      ["file:///etc/passwd", "scheme"],
      ["http://img.test:8080/", "port"],
      ["http://user:pass@img.test/", "userinfo"],
      ["http://nxdomain.test/", "dns"],
    ])(
      "blocks a 307 redirect to %s (%s) without re-posting",
      async (location, reason) => {
        route("/go", redirectTo(location, 307));
        const { fetch, dialed } = harness();
        await expect(fetch("http://img.test/go", post)).resolves.toStrictEqual({
          ok: false,
          reason,
        });
        expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
      }
    );

    it.each([
      ["http://127.0.0.1/x", "private-ip"],
      ["gopher://127.0.0.1:6379/_INFO", "scheme"],
      ["http://2130706433/", "private-ip"],
    ])("blocks a POST to %s before connecting", async (url, reason) => {
      const { fetch, dialed } = harness();
      await expect(fetch(url, post)).resolves.toStrictEqual({
        ok: false,
        reason,
      });
      expect(dialed).toStrictEqual([]);
    });

    it("307/308 repeat the POST; 303 and 302 continue as GET without body", async () => {
      route("/a", redirectTo("/b", 307));
      route("/b", redirectTo("/c", 308));
      route("/c", redirectTo("/d", 303));
      route("/d", record(200));
      route("/p", redirectTo("/q", 302));
      route("/q", record(200));
      const { fetch } = harness();

      okResult(await fetch("http://img.test/a", post));
      okResult(await fetch("http://img.test/p", post));
      expect(seen.map((req) => req.method)).toStrictEqual([
        "POST",
        "POST",
        "POST",
        "GET",
        "POST",
        "GET",
      ]);
      expect(received).toStrictEqual([
        { body: "", method: "GET", path: "/d" },
        { body: "", method: "GET", path: "/q" },
      ]);
    });

    it("same-host: follows on the same host, stops before another host", async () => {
      route("/here", redirectTo("http://img.test/done", 307));
      route("/done", record(204));
      route("/away", redirectTo("http://dual.test/x", 307));
      const { fetch, dialed } = harness();

      await expect(
        fetch("http://img.test/here", { ...post, redirects: "same-host" })
      ).resolves.toMatchObject({ ok: true, redirects: 1, status: 204 });
      dialed.length = 0;
      await expect(
        fetch("http://img.test/away", { ...post, redirects: "same-host" })
      ).resolves.toMatchObject({ ok: true, redirects: 0, status: 307 });
      // The other host was never resolved nor dialed.
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });

    it("none: returns the redirect without following it", async () => {
      route("/go", redirectTo("/done", 302));
      const { fetch } = harness();
      await expect(
        fetch("http://img.test/go", { ...post, redirects: "none" })
      ).resolves.toMatchObject({ ok: true, status: 302 });
      expect(seen).toHaveLength(1);
    });

    it("stops after 5 redirects", async () => {
      route("/loop", redirectTo("/loop", 307));
      const { fetch } = harness();
      await expect(fetch("http://img.test/loop", post)).resolves.toStrictEqual({
        ok: false,
        reason: "too-many-redirects",
      });
      expect(seen).toHaveLength(6);
    });

    it("keeps the size, bomb and timeout limits", async () => {
      route("/big", streamBytes(6 * MB));
      route(
        "/bomb",
        send("text/html", gzipSync(Buffer.alloc(16 * MB)), {
          "content-encoding": "gzip",
        })
      );
      route("/hang", () => {
        // Never answer.
      });
      const { fetch } = harness({ limits: { totalTimeoutMs: 800 } });
      await expect(fetch("http://img.test/big", post)).resolves.toStrictEqual({
        ok: false,
        reason: "size",
      });
      await expect(fetch("http://img.test/bomb", post)).resolves.toStrictEqual({
        ok: false,
        reason: "size",
      });
      await expect(fetch("http://img.test/hang", post)).resolves.toStrictEqual({
        ok: false,
        reason: "timeout",
      });
    });

    it("pins the validated IP (rebinding)", async () => {
      route("/unsub", record(200));
      let calls = 0;
      const { fetch, dialed } = harness({
        resolver: async () => {
          calls += 1;
          return await later({
            A: calls === 1 ? [PUBLIC] : ["127.0.0.1"],
            AAAA: [],
          });
        },
      });
      okResult(await fetch("http://rebind.test/unsub", post));
      await expect(
        fetch("http://rebind.test/unsub", post)
      ).resolves.toStrictEqual({ ok: false, reason: "private-ip" });
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
    });

    it.each([
      ["a GET with a body", { body: FORM, method: "GET" }],
      ["a POST without body", { method: "POST" }],
      [
        "a header injection in the type",
        {
          body: { data: "x", type: "text/plain\r\nx-evil: 1" },
          method: "POST",
        },
      ],
      [
        "a body over 16 KB",
        { body: { ...FORM, data: "x".repeat(16 * 1024 + 1) }, method: "POST" },
      ],
    ] as const)("refuses %s before any lookup", async (_name, request) => {
      const { fetch, lookups } = harness();
      await expect(fetch("http://img.test/unsub", request)).rejects.toThrow(
        TypeError
      );
      expect(lookups).toStrictEqual([]);
    });
  });

  describe("default resolver (c-ares, every runtime)", () => {
    // No `resolver` option: the adapter's own default, against a fake DNS.
    const servers: FakeDns[] = [];
    const start = async (options: FakeDnsOptions) => {
      const dns = await fakeDnsServer(options);
      servers.push(dns);
      return { dns, ...routeResolversTo(dns) };
    };

    afterEach(() => {
      vi.restoreAllMocks();
      for (const dns of servers.splice(0)) {
        dns.close();
      }
    });

    it("an abort cancels the default lookup: nothing is left running", async () => {
      const { dns, spy4, spy6 } = await start({ A: "silent", AAAA: "silent" });
      const { dialed, fetch } = defaultHarness();
      const started = performance.now();
      await expect(
        fetch("http://hung.img.test/a.png", {
          signal: AbortSignal.timeout(100),
        })
      ).resolves.toStrictEqual({ ok: false, reason: "timeout" });
      expect(performance.now() - started).toBeLessThan(1000);
      expect([spy4.mock.calls, spy6.mock.calls]).toStrictEqual([
        [["hung.img.test"]],
        [["hung.img.test"]],
      ]);
      await expect(queriesAfterATry(dns)).resolves.toStrictEqual(
        NOTHING_LEFT_RUNNING
      );
      expect([dialed, seen]).toStrictEqual([[], []]);
    }, 10_000);

    it("the deadline cancels the default lookup: nothing is left running", async () => {
      const { dns, spy4 } = await start({ A: "silent", AAAA: "silent" });
      // The internal `AbortSignal.timeout` of the total deadline, no signal.
      const { dialed, fetch } = defaultHarness({ totalTimeoutMs: 100 });
      const started = performance.now();
      await expect(fetch("http://hung.img.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "timeout",
      });
      expect(performance.now() - started).toBeLessThan(1000);
      expect(spy4).toHaveBeenCalledWith("hung.img.test");
      await expect(queriesAfterATry(dns)).resolves.toStrictEqual(
        NOTHING_LEFT_RUNNING
      );
      expect([dialed, seen]).toStrictEqual([[], []]);
    }, 10_000);

    const zones = new Map([
      ["ok.img.test", { A: [PUBLIC] }],
      ["dual.img.test", { A: [PUBLIC], AAAA: ["::1"] }],
      ["v6only.img.test", { AAAA: ["::ffff:127.0.0.1"] }],
      ["redir.img.test", { A: [PUBLIC] }],
    ]);

    it("the default resolver's public answer is dialed", async () => {
      const { spy4 } = await start({ zones });
      route("/a.png", send("image/png", SAMPLES.png));
      const { dialed, fetch } = defaultHarness();
      const ok = okResult(await fetch("http://ok.img.test/a.png"));
      expect(ok.status).toBe(200);
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
      expect(spy4).toHaveBeenCalledWith("ok.img.test");
    });

    it("the default resolver's answers go through the full guard", async () => {
      const { spy4 } = await start({ zones });
      route("/to-dual", redirectTo("http://dual.img.test/"));
      const { dialed, fetch } = defaultHarness();
      const results = [];
      for (const url of [
        "http://dual.img.test/a.png",
        "http://v6only.img.test/a.png",
        "http://nx.img.test/a.png",
        // Every hop runs the full guard with the default resolver.
        "http://redir.img.test/to-dual",
      ]) {
        // oxlint-disable-next-line no-await-in-loop
        const result = await fetch(url);
        results.push(result.ok ? "ok" : result.reason);
      }
      expect(results).toStrictEqual([
        "private-ip",
        "private-ip",
        "dns",
        "private-ip",
      ]);
      // Only the public first hop of the redirect was dialed.
      expect(dialed).toStrictEqual([`${PUBLIC}:80`]);
      expect(spy4).toHaveBeenCalledWith("dual.img.test");
    });
  });

  describe("production defaults", () => {
    it("guardedFetch blocks loopback, by literal and by name, without connecting", async () => {
      await expect(
        guardedFetch(`http://127.0.0.1:${port}/a.png`)
      ).resolves.toStrictEqual({ ok: false, reason: "port" });
      await expect(
        guardedFetch("http://127.0.0.1/a.png")
      ).resolves.toStrictEqual({ ok: false, reason: "private-ip" });
      await expect(
        guardedFetch("http://[::ffff:127.0.0.1]/")
      ).resolves.toStrictEqual({ ok: false, reason: "private-ip" });
      // `localhost` is refused by name, before the OS resolver is asked.
      await expect(guardedFetch("http://localhost/")).resolves.toStrictEqual({
        ok: false,
        reason: "dns",
      });
      expect(seen).toHaveLength(0);
    });

    it("the default resolver fails closed without Resolver#cancel", () => {
      expect(() =>
        withoutResolverCancel(() =>
          createGuardedFetchForTests({ dial: dialTo([]) })
        )
      ).toThrow(/cancelable DNS resolver/u);
      // A resolver of the caller's own does not need it.
      expect(() =>
        withoutResolverCancel(() =>
          createGuardedFetchForTests({ dial: dialTo([]), resolver: fakeDns })
        )
      ).not.toThrow();
    });

    it("the dial override is refused outside Vitest", () => {
      const saved = process.env.VITEST;
      process.env.VITEST = "false";
      try {
        expect(() =>
          createGuardedFetchForTests({
            dial: () => ({ host: "127.0.0.1", port }),
          })
        ).toThrow(/only available in tests/u);
      } finally {
        process.env.VITEST = saved;
      }
    });
  });
});
