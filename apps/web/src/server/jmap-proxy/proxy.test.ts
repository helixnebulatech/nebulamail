// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The JMAP proxy end to end against a fake JMAP server
// what reaches the server, what reaches the browser.

import { setTimeout as sleep } from "node:timers/promises";

import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../../mail/image-policy";
import { PROXY_COOKIE, SESSION_COOKIE } from "../auth/cookies";
import type { AuthConfig } from "../auth/handlers";
import { DEFAULT_LIMITS, handleLogout } from "../auth/handlers";
import { parseSealKeys, seal } from "../auth/seal";
import type { SealKeys } from "../auth/seal";
import {
  ROTATED_REFRESH,
  TEST_ISSUER,
  TEST_METADATA,
  rotatingTokenFetch,
  setSessionRefreshToken,
} from "../testing";
import { makeUpstream } from "./config";
import { handleJmapProxy, resetEndpointCache } from "./proxy";
import type { ProxyDeps } from "./proxy";
import { SupersededStreams } from "./push-clients";
import { StreamLimits } from "./streams";
import { stalwartSession } from "./test-fixtures";

const APP = new URL("https://nebulamail.example.com");
const INTERNAL = "http://stalwart:8080";
const KEY = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const NOW = 1_800_000_000_000;
const PROXY_KEY = "the-proxy-key";

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const record = (value: unknown): Record<string, unknown> => {
  if (!isRecord(value)) {
    throw new TypeError("expected an object");
  }
  return value;
};

/** Stalwart's session, on the issuer's public name. */
const session = (): Record<string, unknown> =>
  record(
    JSON.parse(
      JSON.stringify(stalwartSession()).replaceAll(
        "https://mail.example.com",
        TEST_ISSUER
      )
    )
  );

interface Seen {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly redirect: RequestRedirect | undefined;
  readonly credentials: RequestCredentials | undefined;
  readonly body: string | undefined;
  readonly signal: AbortSignal | undefined;
}

type Answer = (seen: Seen) => Response | Promise<Response>;

/** A fake JMAP server on INTERNAL: answers by path, records every call. */
const fakeServer = (routes: Record<string, Answer | Answer[]>) => {
  const seen: Seen[] = [];
  const fetch = async (
    input: Parameters<typeof globalThis.fetch>[0],
    init?: RequestInit
  ): Promise<Response> => {
    const url = new URL(input instanceof Request ? input.url : input);
    // A body stream that errors makes fetch reject, as undici does.
    const body =
      init?.body === undefined || init.body === null
        ? undefined
        : await new Response(init.body).text();
    const call: Seen = {
      body,
      credentials: init?.credentials,
      headers: Object.fromEntries(new Headers(init?.headers)),
      method: init?.method ?? "GET",
      redirect: init?.redirect,
      signal: init?.signal ?? undefined,
      url: url.href,
    };
    seen.push(call);
    if (url.origin !== INTERNAL) {
      throw new Error(`fetch to ${url.href}: not the upstream`);
    }
    const route = routes[url.pathname];
    const answer = Array.isArray(route) ? route.shift() : route;
    if (answer === undefined) {
      return new Response("not found", { status: 404 });
    }
    return await answer(call);
  };
  return { fetch, seen };
};

/** An open push stream that never says anything. */
const eventStream = (): Response =>
  new Response(new ReadableStream<Uint8Array>(), {
    headers: { "content-type": "text/event-stream" },
  });

const SESSION_ROUTES = {
  "/.well-known/jmap": (): Response => Response.json(session()),
};

let tokenCalls = 0;
let keys: SealKeys;

const authConfig = (overrides: Partial<AuthConfig> = {}): AuthConfig => ({
  appUrl: APP,
  imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
  jmapProxy: true,
  jmapSessionUrl: new URL("/.well-known/jmap", TEST_ISSUER),
  keys,
  limits: DEFAULT_LIMITS,
  metadata: async () => await Promise.resolve(TEST_METADATA),
  oidc: {
    clientId: "c",
    clientSecret: "s",
    fetch: async (input, init) => {
      tokenCalls += 1;
      return await rotatingTokenFetch(input, init);
    },
    issuer: new URL(TEST_ISSUER),
    issuerId: TEST_ISSUER,
    now: () => NOW,
    redirectUri: new URL("/auth/callback", APP),
  },
  upstream: makeUpstream(new URL(TEST_ISSUER), new URL(INTERNAL)),
  verifiedSenderBadge: null,
  ...overrides,
});

/** The browser's cookies for a session whose token is good for 10 min. */
const cookies = async ({
  expiresIn = 10 * 60_000,
  proxyKey = PROXY_KEY,
  sendProxyCookie = true,
}: {
  readonly expiresIn?: number;
  /** null: a session from before the proxy (no key). */
  readonly proxyKey?: string | null;
  readonly sendProxyCookie?: boolean;
} = {}): Promise<string> => {
  const sealed = await seal(keys, "session", {
    accessToken: "access-1",
    accessTokenExpiresAt: NOW + expiresIn,
    createdAt: NOW,
    lastSeenAt: NOW,
    refreshToken: "refresh-1",
    v: 1,
    ...(proxyKey === null ? {} : { proxyKey }),
  });
  return [
    `${SESSION_COOKIE}=${sealed}`,
    ...(sendProxyCookie ? [`${PROXY_COOKIE}=${PROXY_KEY}`] : []),
  ].join("; ");
};

const SAME_ORIGIN = {
  "sec-fetch-mode": "cors",
  "sec-fetch-site": "same-origin",
  "x-nebula-proxy": "1",
};

const proxyRequest = (
  path: string,
  cookie: string,
  init: {
    readonly method?: string;
    readonly headers?: Record<string, string>;
    readonly body?: BodyInit;
    readonly signal?: AbortSignal;
  } = {}
): Request =>
  new Request(new URL(path, APP), {
    headers: { ...SAME_ORIGIN, cookie, ...init.headers },
    method: init.method ?? "GET",
    ...(init.body === undefined
      ? {}
      : { body: init.body, duplex: "half" as const }),
    ...(init.signal === undefined ? {} : { signal: init.signal }),
  });

const run = async (
  request: Request,
  server: ReturnType<typeof fakeServer>,
  auth: AuthConfig = authConfig(),
  extra: Partial<ProxyDeps> = {}
): Promise<Response> =>
  await handleJmapProxy(request, auth, {
    fetch: server.fetch,
    now: () => NOW,
    ...extra,
  });

describe("JMAP proxy", () => {
  beforeEach(async () => {
    keys = await parseSealKeys(KEY);
    tokenCalls = 0;
    resetEndpointCache();
  });

  describe("JMAP proxy: who may use it", () => {
    it("doesn't exist with JMAP_PROXY off", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const response = await run(
        proxyRequest("/jmap-proxy/session", await cookies()),
        server,
        authConfig({ jmapProxy: false })
      );
      expect(response.status).toBe(404);
      expect(server.seen).toHaveLength(0);
    });

    it("refuses cross-site requests and requests without its header", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const cookie = await cookies();
      for (const headers of [
        { "sec-fetch-site": "cross-site" },
        { "sec-fetch-site": "same-site" },
        { "x-nebula-proxy": "" },
        { "sec-fetch-mode": "navigate" },
      ]) {
        // oxlint-disable-next-line no-await-in-loop -- one at a time, same server
        const response = await run(
          proxyRequest("/jmap-proxy/session", cookie, { headers }),
          server
        );
        expect(response.status).toBe(403);
      }
      // A plain cross-site form POST: no header, other origin.
      const form = new Request(new URL("/jmap-proxy/api", APP), {
        body: "x",
        headers: { cookie, origin: "https://evil.example" },
        method: "POST",
      });
      const refused = await run(form, server);
      expect(refused.status).toBe(403);
      expect(server.seen).toHaveLength(0);
      expect(tokenCalls).toBe(0);
    });

    it("needs the session and its SameSite=Strict proxy cookie", async () => {
      const server = fakeServer(SESSION_ROUTES);
      for (const cookie of [
        "",
        await cookies({ sendProxyCookie: false }),
        await cookies({ proxyKey: null }),
        await cookies({ proxyKey: "another" }),
        `${PROXY_COOKIE}=${PROXY_KEY}`,
      ]) {
        // oxlint-disable-next-line no-await-in-loop -- one at a time
        const response = await run(
          proxyRequest("/jmap-proxy/session", cookie),
          server
        );
        expect(response.status).toBe(401);
        // The session ends in this browser too: otherwise /login would
        // see it and send the user back to the app (a loop).
        expect(
          response.headers
            .getSetCookie()
            .map((value) => value.slice(0, value.indexOf("=")))
            .toSorted()
        ).toStrictEqual([PROXY_COOKIE, SESSION_COOKIE].toSorted());
      }
      expect(server.seen).toHaveLength(0);
    });

    it("logout cuts access at once", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const cookie = await cookies();
      const before = await run(
        proxyRequest("/jmap-proxy/session", cookie),
        server
      );
      expect(before.status).toBe(200);
      const logout = await handleLogout(
        new Request(new URL("/auth/logout", APP), {
          headers: { cookie, "sec-fetch-site": "same-origin" },
          method: "POST",
        }),
        authConfig()
      );
      // The browser keeps what logout set: both cookies empty.
      const left = logout.headers
        .getSetCookie()
        .map((line) => line.split(";")[0] ?? "")
        .join("; ");
      expect(left).toBe(`${SESSION_COOKIE}=; ${PROXY_COOKIE}=`);
      const after = await run(
        proxyRequest("/jmap-proxy/session", left),
        server
      );
      expect(after.status).toBe(401);
      expect(server.seen).toHaveLength(1);
    });
  });

  describe("JMAP proxy: the session", () => {
    it("asks the configured server with the token, no cookies, no redirects", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const response = await run(
        proxyRequest("/jmap-proxy/session", await cookies(), {
          headers: { "x-forwarded-for": "6.6.6.6" },
        }),
        server
      );
      expect([
        response.status,
        response.headers.get("cache-control"),
      ]).toStrictEqual([200, "no-store"]);
      const body = record(await response.json());
      expect(body.apiUrl).toBe(`${APP.origin}/jmap-proxy/api`);
      // Neither the JMAP server's address nor the token reach the page.
      expect(JSON.stringify(body)).not.toMatch(/jmap\.example\.com|access-1/u);
      const [call] = server.seen;
      // Only the allowlisted headers and our own: no cookie, no
      // x-forwarded-for, no x-nebula-proxy.
      expect(call).toMatchObject({
        credentials: "omit",
        headers: {
          accept: "application/json",
          "accept-encoding": "identity",
          authorization: "Bearer access-1",
        },
        redirect: "manual",
        url: `${INTERNAL}/.well-known/jmap`,
      });
      expect(Object.keys(call?.headers ?? {}).toSorted()).toStrictEqual([
        "accept",
        "accept-encoding",
        "authorization",
      ]);
    });

    it("follows the discovery redirect only within the server", async () => {
      const inside = fakeServer({
        "/.well-known/jmap": () =>
          new Response(null, {
            headers: { location: `${TEST_ISSUER}/jmap/session` },
            status: 307,
          }),
        "/jmap/session": () => Response.json(session()),
      });
      const ok = await run(
        proxyRequest("/jmap-proxy/session", await cookies()),
        inside
      );
      expect(ok.status).toBe(200);
      expect(inside.seen.map((call) => call.url)).toStrictEqual([
        `${INTERNAL}/.well-known/jmap`,
        `${INTERNAL}/jmap/session`,
      ]);

      const outside = fakeServer({
        "/.well-known/jmap": () =>
          new Response(null, {
            headers: { location: "https://evil.example/session" },
            status: 302,
          }),
      });
      const refused = await run(
        proxyRequest("/jmap-proxy/session", await cookies()),
        outside
      );
      expect(refused.status).toBe(502);
      expect(outside.seen).toHaveLength(1);
    });

    it("refuses a hostile session with foreign URLs", async () => {
      const server = fakeServer({
        "/.well-known/jmap": () =>
          Response.json({ ...session(), apiUrl: "https://evil.example/jmap/" }),
      });
      const response = await run(
        proxyRequest("/jmap-proxy/session", await cookies()),
        server
      );
      expect(response.status).toBe(502);
      await expect(response.text()).resolves.not.toContain("evil");
    });

    it("gives the page what it needs, never a token", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const response = await run(
        proxyRequest("/jmap-proxy/grant", await cookies()),
        server
      );
      await expect(response.json()).resolves.toStrictEqual({
        expiresAt: NOW + 5 * 60_000,
        imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
        jmapProxy: true,
        serverHostname: new URL(TEST_ISSUER).hostname,
        sessionUrl: `${APP.origin}/jmap-proxy/session`,
        verifiedSenderBadge: null,
      });
      expect(server.seen).toHaveLength(0);
    });
  });

  const API_BODY = JSON.stringify({
    methodCalls: [["Core/echo", {}, "0"]],
    using: ["urn:ietf:params:jmap:core"],
  });

  const apiRequest = async (cookie?: string): Promise<Request> =>
    proxyRequest("/jmap-proxy/api", cookie ?? (await cookies()), {
      body: API_BODY,
      headers: { "content-type": "application/json" },
      method: "POST",
    });

  describe("JMAP proxy: API", () => {
    it("forwards the request and only the allowed answer headers", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/": () =>
          Response.json(
            { methodResponses: [], sessionState: "s1" },
            {
              headers: {
                "access-control-allow-origin": "*",
                "set-cookie": "stalwart=1; Path=/",
              },
            }
          ),
      });
      const response = await run(await apiRequest(), server);
      expect(response.status).toBe(200);
      expect(response.headers.getSetCookie()).toStrictEqual([]);
      expect(response.headers.has("access-control-allow-origin")).toBeFalsy();
      expect(server.seen.at(-1)).toMatchObject({
        body: API_BODY,
        headers: { "content-type": "application/json" },
        redirect: "manual",
        url: `${INTERNAL}/jmap/`,
      });
    });

    it("never follows a redirect", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/": () =>
          new Response(null, {
            headers: { location: `${INTERNAL}/elsewhere` },
            status: 302,
          }),
        "/elsewhere": () => Response.json({ leaked: true }),
      });
      const response = await run(await apiRequest(), server);
      expect(response.status).toBe(502);
      expect(
        server.seen.map((call) => new URL(call.url).pathname)
      ).toStrictEqual(["/.well-known/jmap", "/jmap/"]);
    });

    it("on a 401 refreshes once, retries, and sends the new cookie", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/": [
          () => new Response("{}", { status: 401 }),
          () => Response.json({ methodResponses: [], sessionState: "s1" }),
        ],
      });
      const response = await run(await apiRequest(), server);
      expect(response.status).toBe(200);
      expect(tokenCalls).toBe(1);
      const tokens = server.seen
        .filter((call) => call.url === `${INTERNAL}/jmap/`)
        .map((call) => call.headers.authorization);
      expect(tokens).toStrictEqual(["Bearer access-1", "Bearer access-2"]);
      await expect(setSessionRefreshToken(response, keys)).resolves.toBe(
        ROTATED_REFRESH
      );
    });

    it("answers 401 when the refreshed token is refused too", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/": () => new Response("upstream text", { status: 401 }),
      });
      const response = await run(await apiRequest(), server);
      expect(response.status).toBe(401);
      await expect(response.json()).resolves.toStrictEqual({
        error: "unauthorized",
      });
      expect(tokenCalls).toBe(1);
      // The rotated refresh token still reaches the browser.
      await expect(setSessionRefreshToken(response, keys)).resolves.toBe(
        ROTATED_REFRESH
      );
    });

    it("refreshes a token about to expire before asking", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/": () => Response.json({ methodResponses: [] }),
      });
      const response = await run(
        await apiRequest(await cookies({ expiresIn: 30_000 })),
        server
      );
      expect(response.status).toBe(200);
      expect(server.seen.at(-1)?.headers.authorization).toBe("Bearer access-2");
    });

    it("refuses bodies over maxSizeRequest", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const response = await run(
        proxyRequest("/jmap-proxy/api", await cookies(), {
          body: "x".repeat(10_000_001),
          method: "POST",
        }),
        server
      );
      expect(response.status).toBe(413);
      expect(server.seen).toHaveLength(0);
    });
  });

  describe("JMAP proxy: blobs", () => {
    it("downloads with the values re-encoded and safe headers", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/download/c/G1/..%2Fa%20b.html": () =>
          new Response("<script>alert(1)</script>", {
            headers: {
              "content-disposition": "inline",
              "content-type": "text/html",
              "set-cookie": "x=1",
            },
          }),
      });
      const response = await run(
        proxyRequest(
          "/jmap-proxy/download/c/G1/..%2Fa%20b.html?type=text/html",
          await cookies()
        ),
        server
      );
      expect(response.status).toBe(200);
      expect(server.seen.at(-1)?.url).toBe(
        `${INTERNAL}/jmap/download/c/G1/..%2Fa%20b.html?accept=text%2Fhtml`
      );
      expect(response.headers.get("content-disposition")).toMatch(
        /^attachment;/u
      );
      expect(response.headers.get("content-security-policy")).toBe(
        "sandbox; default-src 'none'"
      );
      expect(response.headers.getSetCookie()).toStrictEqual([]);
    });

    it("refuses traversal and CRLF before asking the server", async () => {
      const server = fakeServer(SESSION_ROUTES);
      const cookie = await cookies();
      for (const path of [
        "/jmap-proxy/download/c/..%2F..%2Fadmin/x",
        "/jmap-proxy/download/c/G1/%2E%2E",
        "/jmap-proxy/download/c/G1/a%0D%0ASet-Cookie:%20x",
        "/jmap-proxy/download/c%2F..%2Fx/G1/a",
      ]) {
        // oxlint-disable-next-line no-await-in-loop -- one at a time
        const response = await run(proxyRequest(path, cookie), server);
        expect([400, 404]).toContain(response.status);
      }
      expect(server.seen).toHaveLength(0);
    });

    it("uploads by streaming, and refuses more than maxSizeUpload", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/upload/c/": (call) =>
          Response.json(
            { accountId: "c", blobId: "G2", size: call.body?.length ?? 0 },
            { status: 201 }
          ),
      });
      const cookie = await cookies();
      const ok = await run(
        proxyRequest("/jmap-proxy/upload/c", cookie, {
          body: "hello",
          headers: { "content-type": "text/plain" },
          method: "POST",
        }),
        server
      );
      expect(ok.status).toBe(201);
      expect(server.seen.at(-1)?.body).toBe("hello");

      const declared = await run(
        proxyRequest("/jmap-proxy/upload/c", cookie, {
          body: "x",
          headers: { "content-length": "50000001" },
          method: "POST",
        }),
        server
      );
      expect(declared.status).toBe(413);

      // No Content-Length (a stream): cut once it passes the limit.
      const small = authConfig();
      resetEndpointCache();
      const tiny = fakeServer({
        "/.well-known/jmap": () => {
          const doc = session();
          const capabilities = record(doc.capabilities);
          return Response.json({
            ...doc,
            capabilities: {
              ...capabilities,
              "urn:ietf:params:jmap:core": { maxSizeUpload: 4 },
            },
          });
        },
        "/jmap/upload/c/": (call) =>
          new Response(call.body ?? "", { status: 201 }),
      });
      const stream = new ReadableStream<Uint8Array>({
        start: (controller) => {
          controller.enqueue(new TextEncoder().encode("12345678"));
          controller.close();
        },
      });
      const streamed = await run(
        proxyRequest("/jmap-proxy/upload/c", cookie, {
          body: stream,
          method: "POST",
        }),
        tiny,
        small
      );
      expect([413, 502]).toContain(streamed.status);
      await expect(streamed.text()).resolves.not.toContain("12345678");
    });
  });

  describe("JMAP proxy: push", () => {
    it("streams events and ends the connection after maxPushMs", async () => {
      let upstreamSignal: AbortSignal | undefined;
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/eventsource/": (call) => {
          upstreamSignal = call.signal;
          return new Response(
            new ReadableStream<Uint8Array>({
              start: (controller) => {
                controller.enqueue(
                  new TextEncoder().encode("event: state\ndata: {}\n\n")
                );
                // Never closes by itself, like a real push stream.
              },
            }),
            { headers: { "content-type": "text/event-stream" } }
          );
        },
      });
      const response = await run(
        proxyRequest(
          "/jmap-proxy/eventsource?types=*&closeafter=no&ping=30",
          await cookies(),
          { headers: { "last-event-id": "7" } }
        ),
        server,
        authConfig(),
        { maxPushMs: 50 }
      );
      expect(response.headers.get("content-type")).toBe("text/event-stream");
      expect(server.seen.at(-1)?.url).toBe(
        `${INTERNAL}/jmap/eventsource/?types=%2A&closeafter=no&ping=30`
      );
      expect(server.seen.at(-1)?.headers["last-event-id"]).toBe("7");
      await expect(response.text()).resolves.toBe("event: state\ndata: {}\n\n");
      expect(upstreamSignal?.aborted).toBeTruthy();
    });

    it("cancels the upstream connection when the browser goes away", async () => {
      let upstreamSignal: AbortSignal | undefined;
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/eventsource/": (call) => {
          upstreamSignal = call.signal;
          return new Response(new ReadableStream<Uint8Array>(), {
            headers: { "content-type": "text/event-stream" },
          });
        },
      });
      const browser = new AbortController();
      const response = await run(
        proxyRequest("/jmap-proxy/eventsource?types=*", await cookies(), {
          signal: browser.signal,
        }),
        server
      );
      expect(upstreamSignal?.aborted).toBeFalsy();
      browser.abort();
      await response.body?.cancel();
      expect(upstreamSignal?.aborted).toBeTruthy();
    });

    it("caps the open streams of a session, and frees them when they end", async () => {
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/eventsource/": () =>
          new Response(new ReadableStream<Uint8Array>(), {
            headers: { "content-type": "text/event-stream" },
          }),
      });
      const streams = new StreamLimits(2, 100);
      const cookie = await cookies();
      const open = async (): Promise<Response> =>
        await run(
          proxyRequest("/jmap-proxy/eventsource?types=*", cookie),
          server,
          authConfig(),
          { streams }
        );
      const first = await open();
      const second = await open();
      const third = await open();
      expect([first.status, second.status, third.status]).toStrictEqual([
        200, 200, 429,
      ]);
      expect(third.headers.get("retry-after")).toBe("5");
      // The API isn't a stream: never capped.
      const api = await run(
        proxyRequest("/jmap-proxy/session", cookie),
        server,
        authConfig(),
        { streams }
      );
      expect(api.status).toBe(200);
      await first.body?.cancel();
      const again = await open();
      expect(again.status).toBe(200);
      await Promise.all([second.body?.cancel(), again.body?.cancel()]);
      expect(streams.openFor(PROXY_KEY)).toBe(0);
    });

    it("a tab's new push stream ends its old one (the host never said it left)", async () => {
      const upstreams: AbortSignal[] = [];
      const server = fakeServer({
        ...SESSION_ROUTES,
        "/jmap/eventsource/": (call) => {
          if (call.signal !== undefined) {
            upstreams.push(call.signal);
          }
          return new Response(new ReadableStream<Uint8Array>(), {
            headers: { "content-type": "text/event-stream" },
          });
        },
      });
      const streams = new StreamLimits(2, 100);
      const pushClients = new SupersededStreams();
      const cookie = await cookies();
      const open = async (client: string, types = "*"): Promise<Response> =>
        await run(
          proxyRequest(`/jmap-proxy/eventsource?types=${types}`, cookie, {
            headers: { "X-Nebula-Client": client },
          }),
          server,
          authConfig(),
          { pushClients, streams }
        );
      const status = async (client: string, types = "*"): Promise<number> => {
        const response = await open(client, types);
        return response.status;
      };
      // Like Vercel after network drops: the browser's side is gone but
      // nothing is cancelled. Twenty reconnects of one tab never run out.
      const tab = "tab-aaaaaaaa";
      const statuses: number[] = [];
      for (let attempt = 0; attempt < 20; attempt += 1) {
        // oxlint-disable-next-line no-await-in-loop -- one reconnect after another.
        statuses.push(await status(tab));
      }
      const reconnected = {
        live: upstreams.at(-1)?.aborted,
        open: streams.openFor(PROXY_KEY),
        statuses: [...new Set(statuses)],
        superseded: upstreams.slice(0, -1).every((signal) => signal.aborted),
      };
      // Another type list (the calendar's stream) or another tab: its own;
      // an id that isn't one doesn't take part.
      const calendar = await status(tab, "Calendar");
      const others = [await status("tab-bbbbbbbb"), await status("x")];
      expect({
        calendar,
        clients: pushClients.size,
        open: streams.openFor(PROXY_KEY),
        others,
        reconnected,
      }).toStrictEqual({
        calendar: 200,
        clients: 2,
        open: 2,
        others: [429, 429],
        reconnected: {
          live: false,
          open: 1,
          statuses: [200],
          superseded: true,
        },
      });
    });

    // The old request may still be reading or refreshing its token when the
    // new one ends it and takes its slot: it must never open upstream.
    it("a superseded stream still waiting for its token never opens upstream", async () => {
      const opened: string[] = [];
      const scenario = async (firstAnswers: Answer[]) => {
        opened.length = 0;
        resetEndpointCache();
        const gate = Promise.withResolvers<undefined>();
        const answers: Answer[] = [...firstAnswers, eventStream, eventStream];
        const server = fakeServer({
          ...SESSION_ROUTES,
          "/jmap/eventsource/": async (call) => {
            opened.push(call.headers.authorization ?? "");
            const next = answers.shift() ?? eventStream;
            return await next(call);
          },
        });
        const slowRefresh = authConfig({
          oidc: {
            ...authConfig().oidc,
            fetch: async (input, init) => {
              await gate.promise;
              return await rotatingTokenFetch(input, init);
            },
          },
        });
        const streams = new StreamLimits(1, 100);
        const pushClients = new SupersededStreams();
        const open = async (cookie: string): Promise<Response> =>
          await run(
            proxyRequest("/jmap-proxy/eventsource?types=*", cookie, {
              headers: { "X-Nebula-Client": "tab-aaaaaaaa" },
            }),
            server,
            slowRefresh,
            { pushClients, streams }
          );
        const first = open(
          await cookies({
            expiresIn: firstAnswers.length === 0 ? 30_000 : 600_000,
          })
        );
        // Let the first one reach its (blocked) token refresh.
        await sleep(20);
        const second = await open(await cookies());
        const whileBlocked = streams.openFor(PROXY_KEY);
        gate.resolve(undefined);
        const old = await first;
        const result = {
          firstStatus: old.status,
          open: streams.openFor(PROXY_KEY),
          secondStatus: second.status,
          upstreamStreams: opened.length - firstAnswers.length,
          whileBlocked,
        };
        await Promise.all([old.body?.cancel(), second.body?.cancel()]);
        return { ...result, after: streams.openFor(PROXY_KEY) };
      };
      const expected = {
        after: 0,
        firstStatus: 409,
        open: 1,
        secondStatus: 200,
        upstreamStreams: 1,
        whileBlocked: 1,
      };
      // Slow refresh before the first try.
      await expect(scenario([])).resolves.toStrictEqual(expected);
      // Slow refresh after the JMAP server's 401, before the retry.
      await expect(
        scenario([() => new Response("expired", { status: 401 })])
      ).resolves.toStrictEqual(expected);
    });
  });
});
