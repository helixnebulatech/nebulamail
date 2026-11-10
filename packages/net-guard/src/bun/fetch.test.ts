// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Bun adapter. The shared vectors (rebinding, redirects to
// private addresses, sizes, bombs, timeouts, POST…) run in
// `../node/fetch.test.ts`, which uses this adapter when run under Bun
// (`bun run test:bun`). This file covers what only real TLS and Bun's
// `fetch` options can show: SNI and certificate name, a test CA, IPv6,
// literal IPs, trying the next address, no connection reuse, and proxy
// variables in the environment. Run under Node or Deno it only checks that
// the adapter refuses to start there.
//
// Servers: one HTTPS server per certificate (`img.test`, `other.test` and
// `v6.test`, all
// from a CA made for the test run with openssl) behind a TCP relay that
// reads the SNI from the TLS ClientHello and counts connections, because
// Bun's HTTPS server has no `SNICallback`. The test-only `dial` hook maps
// the validated public IPs to these local servers.

import { execFileSync } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { connect, createServer as createTcpServer } from "node:net";
import type { AddressInfo, Server as TcpServer, Socket } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { FetchResult } from "../fetch-types";
import { withoutResolverCancel } from "../http/fake-dns";
import type { DnsAnswer, Resolver } from "../index";
import { fail, later, SAMPLES } from "../test-fixtures";
import { createBunFetch, isBun, supportedBunVersion } from "./fetch";
import { createBunFetchForTests } from "./testing";

const PUBLIC = "93.184.215.14";
const PUBLIC_RESET = "93.184.215.98";
const PUBLIC_DEAD = "93.184.215.99";
const PUBLIC6 = "2606:4700::1111";

const okResult = (result: FetchResult) => {
  if (!result.ok) {
    throw new Error(`unexpected block: ${result.reason}`);
  }
  return result;
};

const portOf = (server: Server | TcpServer): number => {
  const address: AddressInfo | string | null = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("server is not listening on TCP");
  }
  return address.port;
};

/** Server Name Indication of a TLS ClientHello, `""` when absent. */
export const sniOf = (hello: Buffer): string => {
  // Record header (5) + handshake header (4) + version (2) + random (32).
  let at = 5 + 4 + 2 + 32;
  if (hello[0] !== 0x16 || hello[5] !== 0x01) {
    return "";
  }
  // Session id, cipher suites, compression methods.
  at += 1 + (hello[at] ?? 0);
  at += 2 + hello.readUInt16BE(at);
  at += 1 + (hello[at] ?? 0);
  const end = at + 2 + hello.readUInt16BE(at);
  at += 2;
  while (at + 4 <= end) {
    const type = hello.readUInt16BE(at);
    const length = hello.readUInt16BE(at + 2);
    if (type === 0) {
      // List length (2), name type (1), name length (2), name.
      const nameLength = hello.readUInt16BE(at + 7);
      return hello.toString("latin1", at + 9, at + 9 + nameLength);
    }
    at += 4 + length;
  }
  return "";
};

interface Relay {
  readonly server: TcpServer;
  readonly sni: string[];
  connections: number;
}

/** TCP relay in front of `targetPort` that records SNI and connections. */
const relay = async (host: string, targetPort: number): Promise<Relay> => {
  const state: Relay = {
    connections: 0,
    server: createTcpServer((socket: Socket) => {
      state.connections += 1;
      socket.on("error", () => {
        // The client may drop the connection (failed handshake).
      });
      socket.once("data", (hello: Buffer) => {
        state.sni.push(sniOf(hello));
        const upstream = connect(targetPort, host);
        upstream.on("error", () => {
          socket.destroy();
        });
        upstream.write(hello);
        socket.pipe(upstream).pipe(socket);
        socket.on("close", () => {
          upstream.destroy();
        });
      });
    }),
    sni: [],
  };
  state.server.listen(0, host);
  await once(state.server, "listening");
  return state;
};

const seen: IncomingMessage[] = [];
/** Request methods in arrival order; `dropped` for the server that drops. */
const methods: string[] = [];
const handler = (req: IncomingMessage, res: ServerResponse): void => {
  seen.push(req);
  methods.push(req.method ?? "");
  const url = new URL(req.url ?? "/", "http://x");
  if (url.pathname === "/redirect-private") {
    res.writeHead(302, { location: "http://169.254.169.254/" }).end();
    return;
  }
  if (url.pathname === "/redirect-relative") {
    res.writeHead(302, { location: "/a.png" }).end();
    return;
  }
  req.resume();
  res.writeHead(200, { "content-type": "image/png" }).end(SAMPLES.png);
};

const DNS: Readonly<Record<string, DnsAnswer>> = {
  "dual.test": { A: [PUBLIC_DEAD, PUBLIC], AAAA: [] },
  "evil.test": { A: [PUBLIC], AAAA: [] },
  "img.test": { A: [PUBLIC], AAAA: [] },
  "other.test": { A: [PUBLIC], AAAA: [] },
  "reset.test": { A: [PUBLIC_RESET, PUBLIC], AAAA: [] },
  "v6.test": { A: [], AAAA: [PUBLIC6] },
};

const fakeDns: Resolver = async (hostname) => {
  const answer = DNS[hostname];
  return answer === undefined ? await fail() : await later(answer);
};

describe.runIf(isBun())("bun adapter", () => {
  let dir = "";
  let ca = "";
  const servers: (Server | TcpServer)[] = [];
  let img: Relay;
  let other: Relay;
  let img6: Relay;
  let http4 = 0;
  let http6 = 0;
  let reset = 0;

  const httpsServer = async (name: string, host: string): Promise<Relay> => {
    const server = createHttpsServer(
      {
        cert: readFileSync(path.join(dir, `${name}.pem`), "utf-8"),
        key: readFileSync(path.join(dir, `${name}.key`), "utf-8"),
      },
      handler
    );
    server.listen(0, host);
    await once(server, "listening");
    servers.push(server);
    const front = await relay(host, portOf(server));
    servers.push(front.server);
    return front;
  };

  const httpServer = async (host: string): Promise<number> => {
    const server = createHttpServer(handler);
    server.listen(0, host);
    await once(server, "listening");
    servers.push(server);
    return portOf(server);
  };

  beforeAll(async () => {
    // A throw-away CA and two certificates, only for this run.
    dir = mkdtempSync(path.join(tmpdir(), "net-guard-bun-"));
    const run = (...args: string[]) =>
      execFileSync("openssl", args, { cwd: dir, stdio: "ignore" });
    const ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256"];
    run(
      "req",
      "-x509",
      ...ec,
      "-nodes",
      "-keyout",
      "ca.key",
      "-out",
      "ca.pem",
      "-days",
      "1",
      "-subj",
      "/CN=net-guard test CA"
    );
    for (const name of ["img.test", "other.test", "v6.test"]) {
      run(
        "req",
        ...ec,
        "-nodes",
        "-keyout",
        `${name}.key`,
        "-out",
        `${name}.csr`,
        "-subj",
        `/CN=${name}`
      );
      writeFileSync(
        path.join(dir, `${name}.ext`),
        `subjectAltName=DNS:${name}\n`
      );
      run(
        "x509",
        "-req",
        "-in",
        `${name}.csr`,
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-out",
        `${name}.pem`,
        "-days",
        "1",
        "-extfile",
        `${name}.ext`
      );
    }
    ca = readFileSync(path.join(dir, "ca.pem"), "utf-8");
    img = await httpsServer("img.test", "127.0.0.1");
    other = await httpsServer("other.test", "127.0.0.1");
    img6 = await httpsServer("v6.test", "::1");
    http4 = await httpServer("127.0.0.1");
    http6 = await httpServer("::1");
    // Reads the request, then drops the connection without answering.
    const dropper = createTcpServer((socket) => {
      socket.on("error", () => {
        // Expected.
      });
      socket.once("data", () => {
        methods.push("dropped");
        socket.destroy();
      });
    });
    dropper.listen(0, "127.0.0.1");
    await once(dropper, "listening");
    servers.push(dropper);
    reset = portOf(dropper);
  });

  beforeEach(() => {
    seen.length = 0;
    methods.length = 0;
    for (const r of [img, other, img6]) {
      r.sni.length = 0;
      r.connections = 0;
    }
  });

  afterAll(() => {
    for (const server of servers) {
      server.close();
    }
    rmSync(dir, { force: true, recursive: true });
  });

  /**
   * `PUBLIC` reaches the server for `cert` (TLS) or plain HTTP; `PUBLIC6`
   * the IPv6 ones; `PUBLIC_RESET` drops the connection after the request;
   * anything else is refused.
   */
  const harness = (
    options: {
      readonly cert?: "img.test" | "other.test";
      readonly ca?: string | null;
      readonly resolver?: Resolver;
    } = {}
  ) => {
    const dialed: string[] = [];
    const lookups: string[] = [];
    const caOption = options.ca === null ? {} : { ca: options.ca ?? ca };
    const fetch = createBunFetchForTests({
      ...caOption,
      dial: (address, port) => {
        dialed.push(`${address}:${port}`);
        if (address === PUBLIC) {
          const front = options.cert === "other.test" ? other : img;
          return {
            host: "127.0.0.1",
            port: port === 443 ? portOf(front.server) : http4,
          };
        }
        if (address === PUBLIC6) {
          return {
            host: "::1",
            port: port === 443 ? portOf(img6.server) : http6,
          };
        }
        if (address === PUBLIC_RESET) {
          return { host: "127.0.0.1", port: reset };
        }
        return { host: "127.0.0.1", port: 1 };
      },
      resolver: async (hostname) => {
        lookups.push(hostname);
        return await (options.resolver ?? fakeDns)(hostname);
      },
    });
    return { dialed, fetch, lookups };
  };

  describe("TLS", () => {
    it("dials the validated IP, sends the name as SNI and Host, and verifies the certificate for it", async () => {
      const { fetch, dialed } = harness();
      const result = okResult(
        await fetch("https://img.test/a.png", { expect: "image" })
      );
      expect(result).toMatchObject({ imageType: "image/png", status: 200 });
      expect(dialed).toStrictEqual([`${PUBLIC}:443`]);
      expect(img.sni).toStrictEqual(["img.test"]);
      expect(seen[0]?.headers.host).toBe("img.test");
      expect(Object.keys(seen[0]?.headers ?? {}).toSorted()).toStrictEqual([
        "accept",
        "accept-encoding",
        "host",
        "user-agent",
      ]);
    });

    it("refuses a certificate for another name (network)", async () => {
      const { fetch } = harness();
      await expect(fetch("https://evil.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      // The handshake was attempted with the requested name, and failed.
      expect(img.sni).toStrictEqual(["evil.test"]);
      expect(seen).toHaveLength(0);
    });

    it("refuses an unknown CA (network)", async () => {
      const { fetch } = harness({ ca: null });
      await expect(fetch("https://img.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      expect(seen).toHaveLength(0);
    });

    it("keeps verifying with NODE_TLS_REJECT_UNAUTHORIZED=0", async () => {
      const saved = process.env.NODE_TLS_REJECT_UNAUTHORIZED;
      process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
      try {
        await expect(
          harness().fetch("https://evil.test/a.png")
        ).resolves.toStrictEqual({ ok: false, reason: "network" });
        await expect(
          harness({ ca: null }).fetch("https://img.test/a.png")
        ).resolves.toStrictEqual({ ok: false, reason: "network" });
        expect(seen).toHaveLength(0);
      } finally {
        if (saved === undefined) {
          delete process.env.NODE_TLS_REJECT_UNAUTHORIZED;
        } else {
          process.env.NODE_TLS_REJECT_UNAUTHORIZED = saved;
        }
      }
    });

    it("an https IP literal gets no SNI and is checked against the IP", async () => {
      const { fetch, dialed, lookups } = harness();
      await expect(fetch(`https://${PUBLIC}/a.png`)).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      expect(lookups).toStrictEqual([]);
      expect(dialed).toStrictEqual([`${PUBLIC}:443`]);
      expect(img.sni).toStrictEqual([""]);
    });

    it("never reuses a connection, not even for the same name", async () => {
      const { fetch } = harness();
      okResult(await fetch("https://img.test/a.png"));
      okResult(await fetch("https://img.test/a.png"));
      // Same IP, another name: a new connection with its own SNI.
      await expect(fetch("https://other.test/a.png")).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      expect(img.connections).toBe(3);
      expect(img.sni).toStrictEqual(["img.test", "img.test", "other.test"]);
      const right = harness({ cert: "other.test" });
      okResult(await right.fetch("https://other.test/a.png"));
      expect(other.sni).toStrictEqual(["other.test"]);
    });

    it("resolves a relative redirect against the name, not the IP", async () => {
      const { fetch, dialed } = harness();
      const result = okResult(
        await fetch("https://img.test/redirect-relative", { expect: "image" })
      );
      expect(result).toMatchObject({
        redirects: 1,
        url: "https://img.test/a.png",
      });
      expect(img.sni).toStrictEqual(["img.test", "img.test"]);
      expect(seen.map((req) => req.headers.host)).toStrictEqual([
        "img.test",
        "img.test",
      ]);
      expect(dialed).toStrictEqual([`${PUBLIC}:443`, `${PUBLIC}:443`]);
    });

    it("blocks a redirect to a private address before dialing it", async () => {
      const { fetch, dialed } = harness();
      await expect(
        fetch("https://img.test/redirect-private")
      ).resolves.toStrictEqual({ ok: false, reason: "private-ip" });
      expect(dialed).toStrictEqual([`${PUBLIC}:443`]);
    });
  });

  describe("IPv6", () => {
    it("dials an AAAA answer in brackets, over TLS", async () => {
      const { fetch, dialed } = harness();
      okResult(await fetch("https://v6.test/a.png", { expect: "image" }));
      expect(dialed).toStrictEqual([`${PUBLIC6}:443`]);
      expect(img6.sni).toStrictEqual(["v6.test"]);
      expect(seen[0]?.headers.host).toBe("v6.test");
    });

    it("an IPv6 literal keeps its brackets in Host and is not resolved", async () => {
      const { fetch, dialed, lookups } = harness();
      okResult(await fetch(`http://[${PUBLIC6}]/a.png`, { expect: "image" }));
      expect(lookups).toStrictEqual([]);
      expect(dialed).toStrictEqual([`${PUBLIC6}:80`]);
      expect(seen[0]?.headers.host).toBe(`[${PUBLIC6}]`);
    });

    it.each([
      "[::1]",
      "[::ffff:127.0.0.1]",
      "[fe80::1]",
      "[64:ff9b::a9fe:a9fe]",
    ])("blocks the literal %s without dialing", async (host) => {
      const { fetch, dialed } = harness();
      await expect(fetch(`http://${host}/`)).resolves.toStrictEqual({
        ok: false,
        reason: "private-ip",
      });
      expect(dialed).toStrictEqual([]);
    });
  });

  describe("next address", () => {
    it("a GET moves on after a refused connection", async () => {
      const { fetch, dialed } = harness();
      okResult(await fetch("http://dual.test/a.png"));
      expect(dialed).toStrictEqual([`${PUBLIC_DEAD}:80`, `${PUBLIC}:80`]);
    });

    it("a GET moves on after a dropped connection", async () => {
      const { fetch, dialed } = harness();
      okResult(await fetch("http://reset.test/a.png"));
      expect(dialed).toStrictEqual([`${PUBLIC_RESET}:80`, `${PUBLIC}:80`]);
    });

    const post = {
      body: { data: "List-Unsubscribe=One-Click", type: "text/plain" },
      method: "POST",
    } as const;

    it("a POST moves on after a refused connection (nothing was sent)", async () => {
      const { fetch, dialed } = harness();
      okResult(await fetch("http://dual.test/unsub", post));
      expect(dialed).toStrictEqual([`${PUBLIC_DEAD}:80`, `${PUBLIC}:80`]);
      expect(methods).toStrictEqual(["POST"]);
    });

    it("a POST is never sent twice: a dropped connection is network", async () => {
      const { fetch, dialed } = harness();
      await expect(
        fetch("http://reset.test/unsub", post)
      ).resolves.toStrictEqual({
        ok: false,
        reason: "network",
      });
      expect(dialed).toStrictEqual([`${PUBLIC_RESET}:80`]);
      expect(methods).toStrictEqual(["dropped"]);
    });
  });

  describe("vector: env-proxy-ignored", () => {
    const KEYS = [
      "HTTP_PROXY",
      "HTTPS_PROXY",
      "ALL_PROXY",
      "http_proxy",
      "https_proxy",
      "all_proxy",
    ];

    it("ignores HTTP_PROXY, HTTPS_PROXY and ALL_PROXY (Bun's own fetch would use them)", async () => {
      let proxied = 0;
      const proxy = createHttpServer((_req, res) => {
        proxied += 1;
        res.writeHead(200, { "content-type": "image/png" }).end(SAMPLES.png);
      });
      proxy.on("connect", (_req, socket: Socket) => {
        proxied += 1;
        socket.destroy();
      });
      proxy.listen(0, "127.0.0.1");
      await once(proxy, "listening");
      const saved = { ...process.env };
      for (const key of KEYS) {
        process.env[key] = `http://127.0.0.1:${portOf(proxy)}`;
      }
      try {
        const { fetch } = harness();
        okResult(await fetch("http://img.test/a.png", { expect: "image" }));
        okResult(await fetch("https://img.test/a.png", { expect: "image" }));
        expect(proxied).toBe(0);
        expect(seen).toHaveLength(2);
        // Sanity check: the variables are live, plain `fetch` does use them.
        const raw = await globalThis.fetch(`http://127.0.0.1:${http4}/a.png`);
        await raw.arrayBuffer();
        expect(proxied).toBe(1);
      } finally {
        process.env = saved;
        proxy.close();
      }
    });
  });

  it("refuses the test hooks outside Vitest", () => {
    const saved = process.env.VITEST;
    process.env.VITEST = "false";
    try {
      expect(() =>
        createBunFetchForTests({
          dial: () => ({ host: "127.0.0.1", port: 1 }),
        })
      ).toThrow(/only available in tests/u);
    } finally {
      process.env.VITEST = saved;
    }
  });

  it("refuses to start without a cancelable DNS resolver", () => {
    // Past the Bun and version checks: the error comes from the new one.
    expect(() =>
      withoutResolverCancel(() =>
        createBunFetchForTests({
          dial: () => ({ host: "127.0.0.1", port: 1 }),
        })
      )
    ).toThrow(/cancelable DNS resolver/u);
    // Restored afterwards.
    expect(() => createBunFetch()).not.toThrow();
  });

  it("production defaults block loopback by literal and by name", async () => {
    const fetch = createBunFetch();
    await expect(fetch("http://127.0.0.1/")).resolves.toStrictEqual({
      ok: false,
      reason: "private-ip",
    });
    // Refused by name, before resolving.
    await expect(fetch("http://localhost/")).resolves.toStrictEqual({
      ok: false,
      reason: "dns",
    });
  });
});

describe.skipIf(isBun())("bun adapter outside Bun", () => {
  it("refuses to start", () => {
    expect(() => createBunFetch()).toThrow(/only runs on Bun/u);
  });
});

describe(supportedBunVersion, () => {
  it.each([
    ["1.4.3", true],
    ["1.4.4", true],
    ["1.10.0", true],
    ["2.0.0", true],
    ["1.4.3-canary.1", true],
    ["1.4.2", false],
    ["1.3.9", false],
    ["0.9.9", false],
    ["", false],
    ["garbage", false],
    [undefined, false],
  ])("%s → %s", (version, expected) => {
    expect(supportedBunVersion(version)).toBe(expected);
  });
});
