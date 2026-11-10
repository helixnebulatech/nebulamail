// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import type { FetchRequest, FetchResult } from "@nebulamail/net-guard/node";
import { beforeEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../mail/image-policy";
import { SESSION_COOKIE } from "./auth/cookies";
import type { AuthConfig } from "./auth/handlers";
import { DEFAULT_LIMITS } from "./auth/handlers";
import { parseSealKeys, seal } from "./auth/seal";
import {
  ROTATED_REFRESH,
  failingFetch,
  postWithCookie,
  refreshingAuth,
  setSessionRefreshToken,
} from "./testing";
import type { RateLimiter } from "./unsubscribe";
import { createRateLimiter, handleUnsubscribe } from "./unsubscribe";

const APP = new URL("https://mail.example.com");
const JMAP = "https://jmap.example.com";
const NOW = Date.UTC(2026, 9, 24, 12);
const KEY = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

const SESSION = {
  accounts: {
    acc1: {
      accountCapabilities: { "urn:ietf:params:jmap:mail": {} },
      isPersonal: true,
      isReadOnly: false,
      name: "alice@example.com",
    },
  },
  apiUrl: `${JMAP}/jmap/`,
  capabilities: {
    "urn:ietf:params:jmap:core": {
      collationAlgorithms: [],
      maxCallsInRequest: 16,
      maxConcurrentRequests: 4,
      maxConcurrentUpload: 4,
      maxObjectsInGet: 500,
      maxObjectsInSet: 500,
      maxSizeRequest: 10_000_000,
      maxSizeUpload: 50_000_000,
    },
    "urn:ietf:params:jmap:mail": {},
  },
  downloadUrl: `${JMAP}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${JMAP}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: { "urn:ietf:params:jmap:mail": "acc1" },
  state: "s1",
  uploadUrl: `${JMAP}/jmap/upload/{accountId}/`,
  username: "alice@example.com",
};

const SIGNATURE =
  "v=1; a=rsa-sha256; d=shop.example; s=sel1; h=From:Subject:List-Unsubscribe:List-Unsubscribe-Post; bh=x; b=AbCdEf";

const ELIGIBLE = {
  from: [{ email: "news@shop.example", name: "Shop" }],
  "header:Authentication-Results:asText:all": [
    "jmap.example.com; dkim=pass header.d=shop.example header.s=sel1; spf=pass",
    // Written by the sender, below ours: ignored.
    "jmap.example.com; dkim=pass header.d=evil.example",
  ],
  "header:DKIM-Signature:asText:all": [SIGNATURE],
  "header:List-Unsubscribe-Post:asText": "List-Unsubscribe=One-Click",
  "header:List-Unsubscribe:asURLs": [
    "mailto:leave@shop.example",
    "https://shop.example/u/abc",
  ],
  id: "e1",
};

const JmapRequest = z.object({
  methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
});

const createJmap = (email: Record<string, unknown> | null) => {
  const calls: unknown[] = [];
  const fakeFetch: typeof fetch = async (input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    if (url.endsWith("/.well-known/jmap")) {
      return Response.json(SESSION);
    }
    const body = JmapRequest.parse(
      JSON.parse(await new Response(init?.body).text())
    );
    calls.push(body);
    return Response.json({
      methodResponses: [
        [
          "Email/get",
          {
            accountId: "acc1",
            list: email === null ? [] : [email],
            notFound: email === null ? ["e1"] : [],
            state: "s",
          },
          body.methodCalls[0]?.[2] ?? "",
        ],
      ],
      sessionState: "s1",
    });
  };
  return { calls, fetch: fakeFetch };
};

interface Sent {
  readonly url: string;
  readonly request: FetchRequest | undefined;
}

const fakeGuard = (result: FetchResult) => {
  const sent: Sent[] = [];
  return {
    guardedFetch: async (
      url: string | URL,
      request?: FetchRequest
    ): Promise<FetchResult> => {
      sent.push({ request, url: String(url) });
      return await Promise.resolve(result);
    },
    sent,
  };
};

const OK: FetchResult = {
  body: new Uint8Array(0),
  headers: {},
  ok: true,
  redirects: 0,
  status: 200,
  url: "https://shop.example/u/abc",
};

describe(handleUnsubscribe, () => {
  let auth: AuthConfig;
  let cookie: string;

  beforeEach(async () => {
    const keys = await parseSealKeys(KEY);
    auth = {
      appUrl: APP,
      jmapSessionUrl: new URL("/.well-known/jmap", JMAP),
      keys,
      limits: DEFAULT_LIMITS,
      metadata: async () => await Promise.reject(new Error("not used")),
      oidc: {
        clientId: "c",
        clientSecret: "s",
        fetch: async () => await Promise.reject(new Error("not used")),
        issuer: new URL(JMAP),
        issuerId: JMAP,
        now: () => NOW,
        redirectUri: new URL("/auth/callback", APP),
      },
      verifiedSenderBadge: null,
      imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
    };
    const sealed = await seal(keys, "session", {
      accessToken: "access-1",
      accessTokenExpiresAt: NOW + 10 * 60_000,
      createdAt: NOW,
      lastSeenAt: NOW,
      refreshToken: "refresh-1",
      v: 1,
    });
    cookie = `${SESSION_COOKIE}=${sealed}`;
  });

  const post = (body: unknown, site = "same-origin"): Request =>
    new Request(`${APP.origin}/api/unsubscribe`, {
      body: typeof body === "string" ? body : JSON.stringify(body),
      headers: { cookie, "sec-fetch-site": site },
      method: "POST",
    });

  const run = async (
    request: Request,
    {
      email = ELIGIBLE,
      result = OK,
      rateLimit = createRateLimiter(20, 60_000),
    }: {
      readonly email?: Record<string, unknown> | null;
      readonly result?: FetchResult;
      readonly rateLimit?: RateLimiter;
    } = {}
  ) => {
    const jmap = createJmap(email);
    const guard = fakeGuard(result);
    const response = await handleUnsubscribe(request, auth, {
      fetch: jmap.fetch,
      guardedFetch: guard.guardedFetch,
      rateLimit,
    });
    return {
      body: (await response.json()) as unknown,
      calls: jmap.calls,
      response,
      sent: guard.sent,
    };
  };

  it("POSTs List-Unsubscribe=One-Click to the email's own https URL", async () => {
    const { body, calls, response, sent } = await run(post({ emailId: "e1" }));

    expect([response.status, body]).toStrictEqual([200, { ok: true }]);
    expect(sent).toStrictEqual([
      {
        request: {
          body: {
            data: "List-Unsubscribe=One-Click",
            type: "application/x-www-form-urlencoded",
          },
          method: "POST",
          readBody: false,
          redirects: "none",
        },
        url: "https://shop.example/u/abc",
      },
    ]);
    expect(JSON.stringify(calls[0])).toContain('"ids":["e1"]');
  });

  it("never takes a URL from the browser", async () => {
    const { response, sent } = await run(
      post({ emailId: "e1", url: "https://evil.example/" })
    );
    expect(response.status).toBe(400);
    expect(sent).toHaveLength(0);
  });

  it.each([
    [
      "DKIM not from our server",
      {
        "header:Authentication-Results:asText:all": [
          "mx.evil.example; dkim=pass header.d=shop.example",
        ],
      },
    ],
    [
      "no List-Unsubscribe-Post",
      { "header:List-Unsubscribe-Post:asText": null },
    ],
    [
      "only mailto",
      { "header:List-Unsubscribe:asURLs": ["mailto:leave@shop.example"] },
    ],
    [
      "unsigned List-Unsubscribe",
      {
        "header:DKIM-Signature:asText:all": [
          SIGNATURE.replace(":List-Unsubscribe:List-Unsubscribe-Post", ""),
        ],
      },
    ],
  ])("re-checks on the server: refuses %s", async (_name, change) => {
    const { body, response, sent } = await run(post({ emailId: "e1" }), {
      email: { ...ELIGIBLE, ...change },
    });
    expect([response.status, body]).toStrictEqual([
      422,
      { error: "not-allowed", ok: false },
    ]);
    expect(sent).toHaveLength(0);
  });

  it.each([
    [{ ok: false, reason: "private-ip" } as const, "blocked"],
    [{ ok: false, reason: "too-many-redirects" } as const, "blocked"],
    [{ ok: false, reason: "timeout" } as const, "unreachable"],
    [{ ...OK, status: 301 }, "redirected"],
    [{ ...OK, status: 302 }, "redirected"],
    [{ ...OK, status: 303 }, "redirected"],
    [{ ...OK, status: 307 }, "redirected"],
    [{ ...OK, status: 308 }, "redirected"],
    [{ ...OK, status: 500 }, "rejected"],
  ])("reports %o as %s", async (result, error) => {
    const { body, response } = await run(post({ emailId: "e1" }), { result });
    expect([response.status, body]).toStrictEqual([502, { error, ok: false }]);
  });

  it("refuses cross-site requests and needs a session", async () => {
    const crossSite = await run(post({ emailId: "e1" }, "cross-site"));
    const anonymous = await run(
      new Request(`${APP.origin}/api/unsubscribe`, {
        body: JSON.stringify({ emailId: "e1" }),
        headers: { "sec-fetch-site": "same-origin" },
        method: "POST",
      })
    );
    expect([
      crossSite.response.status,
      anonymous.response.status,
    ]).toStrictEqual([403, 401]);
    expect([...crossSite.sent, ...anonymous.sent]).toHaveLength(0);
  });

  it("answers 404 for an email the user can't see", async () => {
    const { response, sent } = await run(post({ emailId: "e1" }), {
      email: null,
    });
    expect(response.status).toBe(404);
    expect(sent).toHaveLength(0);
  });

  it("rate-limits per user", async () => {
    const rateLimit = createRateLimiter(2, 60_000);
    const statuses: number[] = [];
    for (let i = 0; i < 3; i += 1) {
      // oxlint-disable-next-line no-await-in-loop -- the order is the point.
      const { response } = await run(post({ emailId: "e1" }), { rateLimit });
      statuses.push(response.status);
    }
    expect(statuses).toStrictEqual([200, 200, 429]);
  });
});

describe(createRateLimiter, () => {
  it("frees slots after the window", () => {
    const limiter = createRateLimiter(1, 1000);
    expect([
      limiter.take("a", 0),
      limiter.take("a", 500),
      limiter.take("b", 500),
      limiter.take("a", 1000),
    ]).toStrictEqual([true, false, true, true]);
  });
});

describe("handleUnsubscribe: body limit and rotated cookie", () => {
  it("refuses a 2 KB body before touching the session", async () => {
    const { auth, cookie, tokenCalls } = await refreshingAuth(NOW);
    let jmapCalls = 0;
    const jmapFetch: typeof fetch = async () => {
      jmapCalls += 1;
      return await Promise.reject(new TypeError("not used"));
    };
    const response = await handleUnsubscribe(
      postWithCookie("/api/unsubscribe", cookie, "x".repeat(2048)),
      auth,
      {
        fetch: jmapFetch,
        guardedFetch: async () => await Promise.reject(new Error("not used")),
        rateLimit: createRateLimiter(20, 60_000),
      }
    );

    expect(response.status).toBe(413);
    expect([tokenCalls(), jmapCalls]).toStrictEqual([0, 0]);
  });

  it("keeps the rotated session cookie when JMAP fails after a refresh", async () => {
    const { auth, cookie, keys, tokenCalls } = await refreshingAuth(NOW);
    const response = await handleUnsubscribe(
      postWithCookie(
        "/api/unsubscribe",
        cookie,
        JSON.stringify({ emailId: "e1" })
      ),
      auth,
      {
        fetch: failingFetch,
        guardedFetch: async () => await Promise.reject(new Error("not used")),
        rateLimit: createRateLimiter(20, 60_000),
      }
    );

    expect([response.status, tokenCalls()]).toStrictEqual([502, 1]);
    await expect(setSessionRefreshToken(response, keys)).resolves.toBe(
      ROTATED_REFRESH
    );
  });
});
