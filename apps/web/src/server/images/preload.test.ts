// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import {
  parseTicketKeys,
  signPreferenceRequest,
  verifyPreferenceRequest,
} from "@nebulamail/image-proxy/tickets";
import type { PreferenceRequest } from "@nebulamail/image-proxy/tickets";
import { beforeEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../../mail/image-policy";
import { SESSION_COOKIE } from "../auth/cookies";
import type { AuthConfig } from "../auth/handlers";
import { DEFAULT_LIMITS } from "../auth/handlers";
import { parseSealKeys, seal } from "../auth/seal";
import {
  ROTATED_REFRESH,
  failingFetch,
  postWithCookie,
  refreshingAuth,
  setSessionRefreshToken,
} from "../testing";
import type { PreloadPrefsConfig } from "./preload";
import { handlePreloadPreference } from "./preload";

const APP = new URL("https://mail.example.com");
const JMAP = "https://jmap.example.com";
const PROXY = "https://img.example.com";
const NOW = Date.now();
const KEY = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

const SESSION = {
  accounts: {
    acc1: {
      accountCapabilities: {
        "urn:ietf:params:jmap:mail": {},
        "urn:ietf:params:jmap:submission": {},
      },
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
    "urn:ietf:params:jmap:submission": {},
  },
  downloadUrl: `${JMAP}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${JMAP}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: {
    "urn:ietf:params:jmap:mail": "acc1",
    "urn:ietf:params:jmap:submission": "acc1",
  },
  state: "s1",
  uploadUrl: `${JMAP}/jmap/upload/{accountId}/`,
  username: "Alice@Example.com",
};

const JmapRequest = z.object({
  methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
});

describe(handlePreloadPreference, () => {
  let auth: AuthConfig;
  let cookie: string;
  let config: PreloadPrefsConfig;
  let proxyKeys: Awaited<ReturnType<typeof parseTicketKeys>>;

  beforeEach(async () => {
    const keys = await parseSealKeys(KEY);
    proxyKeys = await parseTicketKeys(KEY);
    auth = {
      appUrl: APP,
      jmapSessionUrl: new URL("/.well-known/jmap", JMAP),
      keys,
      limits: DEFAULT_LIMITS,
      verifiedSenderBadge: null,
      imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
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
    config = {
      proxyOrigin: PROXY,
      sign: async (input) => await signPreferenceRequest(proxyKeys, input),
    };
  });

  /** JMAP plus a proxy that checks the signature like the real one. */
  const network = (
    proxyStatus = 200,
    identities: "fail" | readonly { email: string; id: string }[] | null = null,
    /** The proxy's `full` (missing: a proxy older than the warning). */
    full?: unknown
  ) => {
    const received: PreferenceRequest[] = [];
    let stored = false;
    const fake: typeof fetch = async (input, init) => {
      const url = input instanceof Request ? input.url : String(input);
      if (url.endsWith("/.well-known/jmap")) {
        return Response.json(SESSION);
      }
      if (url.startsWith(PROXY)) {
        expect(url).toBe(`${PROXY}/preload/preferences`);
        expect(init?.redirect).toBe("manual");
        if (proxyStatus !== 200) {
          return Response.json({ error: "x" }, { status: proxyStatus });
        }
        const verdict = await verifyPreferenceRequest(
          proxyKeys,
          new Headers(init?.headers).get("authorization"),
          typeof init?.body === "string" ? init.body : "",
          Date.now()
        );
        if (!verdict.ok) {
          return Response.json({ error: "unauthorized" }, { status: 401 });
        }
        received.push(verdict.request);
        stored = verdict.request.optOut ?? stored;
        return Response.json(
          full === undefined ? { optOut: stored } : { full, optOut: stored }
        );
      }
      const body = JmapRequest.parse(
        JSON.parse(await new Response(init?.body).text())
      );
      const [name, , callId = ""] = body.methodCalls[0] ?? [];
      expect(name).toBe("Identity/get");
      if (identities === "fail") {
        return Response.json({
          methodResponses: [["error", { type: "serverFail" }, callId]],
          sessionState: "s1",
        });
      }
      if (identities !== null) {
        return Response.json({
          methodResponses: [
            [
              "Identity/get",
              { accountId: "acc1", list: identities, notFound: [], state: "s" },
              callId,
            ],
          ],
          sessionState: "s1",
        });
      }
      return Response.json({
        methodResponses: [
          [
            "Identity/get",
            {
              accountId: "acc1",
              list: [
                { email: "alice@example.com", id: "i1" },
                { email: "Alice <sales@example.com>", id: "i2" },
              ],
              notFound: [],
              state: "s",
            },
            callId,
          ],
        ],
        sessionState: "s1",
      });
    };
    return { fetch: fake, received };
  };

  const post = (body: unknown, site = "same-origin"): Request =>
    new Request(`${APP.origin}/api/images/preload`, {
      body: JSON.stringify(body),
      headers: { cookie, "sec-fetch-site": site },
      method: "POST",
    });

  it("sends the user's own addresses from JMAP, signed, and returns the state", async () => {
    const net = network();
    const off = await handlePreloadPreference(
      post({ optOut: true }),
      auth,
      config,
      net
    );
    await expect(off.json()).resolves.toStrictEqual({
      available: true,
      full: null,
      optOut: true,
    });
    const read = await handlePreloadPreference(post({}), auth, config, net);
    await expect(read.json()).resolves.toStrictEqual({
      available: true,
      full: null,
      optOut: true,
    });
    expect(net.received.map((r) => [r.addresses, r.optOut])).toStrictEqual([
      [["alice@example.com", "sales@example.com"], true],
      [["alice@example.com", "sales@example.com"], undefined],
    ]);
  });

  it("says when the user's preload quota is full, and until when", async () => {
    const full = await handlePreloadPreference(
      post({}),
      auth,
      config,
      network(200, null, { until: NOW + 1000 })
    );
    await expect(full.json()).resolves.toStrictEqual({
      available: true,
      full: { until: NOW + 1000 },
      optOut: false,
    });
    const unknown = await handlePreloadPreference(
      post({}),
      auth,
      config,
      network(200, null, { until: null })
    );
    await expect(unknown.json()).resolves.toMatchObject({
      full: { until: null },
    });
    const bad = await handlePreloadPreference(
      post({}),
      auth,
      config,
      network(200, null, { until: "soon" })
    );
    expect(bad.status).toBe(502);
  });

  it("never takes addresses from the browser", async () => {
    const net = network();
    const response = await handlePreloadPreference(
      post({ addresses: ["bob@example.com"], optOut: true }),
      auth,
      config,
      net
    );
    expect(response.status).toBe(400);
    expect(net.received).toHaveLength(0);
  });

  it("is unavailable without a proxy or when the proxy has no preload", async () => {
    const none = await handlePreloadPreference(
      post({}),
      auth,
      undefined,
      network()
    );
    await expect(none.json()).resolves.toStrictEqual({ available: false });
    const noPreload = await handlePreloadPreference(
      post({}),
      auth,
      config,
      network(404)
    );
    await expect(noPreload.json()).resolves.toStrictEqual({
      available: false,
    });
    const broken = await handlePreloadPreference(
      post({ optOut: true }),
      auth,
      config,
      network(500)
    );
    expect(broken.status).toBe(502);
  });

  it("signs nothing when Identity/get fails", async () => {
    const net = network(200, "fail");
    const response = await handlePreloadPreference(
      post({ optOut: true }),
      auth,
      config,
      net
    );

    expect(response.status).toBe(502);
    expect(net.received).toHaveLength(0);
  });

  it("refuses to opt out only some of more than 50 addresses", async () => {
    const many = Array.from({ length: 51 }, (_, i) => ({
      email: `alias${i}@example.org`,
      id: `i${i}`,
    }));
    const net = network(200, many);
    const response = await handlePreloadPreference(
      post({ optOut: true }),
      auth,
      config,
      net
    );

    expect(response.status).toBe(422);
    await expect(response.json()).resolves.toStrictEqual({
      error: "too-many-addresses",
    });
    expect(net.received).toHaveLength(0);
  });

  it("refuses cross-site requests and needs a session", async () => {
    const crossSite = await handlePreloadPreference(
      post({ optOut: true }, "cross-site"),
      auth,
      config,
      network()
    );
    expect(crossSite.status).toBe(403);
    const anonymous = await handlePreloadPreference(
      new Request(`${APP.origin}/api/images/preload`, {
        body: "{}",
        headers: { "sec-fetch-site": "same-origin" },
        method: "POST",
      }),
      auth,
      config,
      network()
    );
    expect(anonymous.status).toBe(401);
  });
});

describe("handlePreloadPreference: body limit and rotated cookie", () => {
  it("refuses a 2 KB body before touching the session", async () => {
    const { auth, cookie, tokenCalls } = await refreshingAuth(NOW);
    let jmapCalls = 0;
    const jmapFetch: typeof fetch = async () => {
      jmapCalls += 1;
      return await Promise.reject(new TypeError("not used"));
    };
    const response = await handlePreloadPreference(
      postWithCookie("/api/images/preload", cookie, "x".repeat(2048)),
      auth,
      {
        proxyOrigin: PROXY,
        sign: async () =>
          await Promise.resolve({ authorization: "x", body: "{}" }),
      },
      { fetch: jmapFetch }
    );

    expect(response.status).toBe(413);
    expect([tokenCalls(), jmapCalls]).toStrictEqual([0, 0]);
  });

  it("keeps the rotated session cookie when JMAP fails after a refresh", async () => {
    const { auth, cookie, keys, tokenCalls } = await refreshingAuth(NOW);
    const response = await handlePreloadPreference(
      postWithCookie(
        "/api/images/preload",
        cookie,
        JSON.stringify({ optOut: true })
      ),
      auth,
      {
        proxyOrigin: PROXY,
        sign: async () =>
          await Promise.resolve({ authorization: "x", body: "{}" }),
      },
      { fetch: failingFetch }
    );

    expect([response.status, tokenCalls()]).toStrictEqual([502, 1]);
    await expect(setSessionRefreshToken(response, keys)).resolves.toBe(
      ROTATED_REFRESH
    );
  });
});
