// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { inProcessExtractor } from "@nebulamail/mail-render/extract";
import type { ImageUrlExtractor } from "@nebulamail/mail-render/extract";
import { JSDOM } from "jsdom";
import { beforeEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../../mail/image-policy";
import { SESSION_COOKIE } from "../auth/cookies";
import type { AuthConfig } from "../auth/handlers";
import { DEFAULT_LIMITS } from "../auth/handlers";
import { parseSealKeys, seal } from "../auth/seal";
import type { RateLimiter } from "../rate-limit";
import { createRateLimiter } from "../rate-limit";
import {
  ROTATED_REFRESH,
  failingFetch,
  postWithCookie,
  refreshingAuth,
  setSessionRefreshToken,
} from "../testing";
import type { ImageTicketConfig, TicketClaims } from "./tickets";
import { handleImageTickets, remotePhotoUrls } from "./tickets";

const APP = new URL("https://mail.example.com");
const JMAP = "https://jmap.example.com";
const NOW = Date.UTC(2026, 9, 23, 12);
const KEY = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

const SESSION = {
  accounts: {
    acc1: {
      accountCapabilities: {
        "urn:ietf:params:jmap:contacts": {},
        "urn:ietf:params:jmap:mail": {},
      },
      isPersonal: true,
      isReadOnly: false,
      name: "alice@example.com",
    },
    // A folder bob shared with alice (mail:share).
    acc2: {
      accountCapabilities: { "urn:ietf:params:jmap:mail": {} },
      isPersonal: false,
      isReadOnly: false,
      name: "bob@example.com",
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
    "urn:ietf:params:jmap:contacts": {},
    "urn:ietf:params:jmap:mail": {},
  },
  downloadUrl: `${JMAP}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${JMAP}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: {
    "urn:ietf:params:jmap:contacts": "acc1",
    "urn:ietf:params:jmap:mail": "acc1",
  },
  state: "s1",
  uploadUrl: `${JMAP}/jmap/upload/{accountId}/`,
  username: "alice@example.com",
};

const HTML = [
  '<p>Hola</p><img src="https://cdn.example.com/banner.png" width="600" height="200">',
  '<img src="https://cdn.example.com/banner.png">',
  '<div style="background-image:url(https://cdn.example.com/bg.jpg)">x</div>',
  '<img src="https://tracking.example.com/open.gif" width="1" height="1">',
  '<img src="cid:logo@x"><img src="/relative.png">',
].join("");

const JmapRequest = z.object({
  methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
});

const TicketsSchema = z.object({
  images: z.array(
    z.object({ src: z.string(), ticket: z.string(), url: z.string() })
  ),
  omitted: z.number(),
  trackersBlocked: z.number(),
});

interface Recorded {
  readonly authorization: string | null;
  readonly body: unknown;
}

const createJmap = (html: string | null) => {
  const calls: Recorded[] = [];
  const fakeFetch: typeof fetch = async (input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    const authorization = new Headers(init?.headers).get("authorization");
    if (url.endsWith("/.well-known/jmap")) {
      calls.push({ authorization, body: null });
      return Response.json(SESSION);
    }
    const body = JmapRequest.parse(
      JSON.parse(await new Response(init?.body).text())
    );
    calls.push({ authorization, body });
    const [name, args, callId = ""] = body.methodCalls[0] ?? [];
    const { accountId } = z.object({ accountId: z.string() }).parse(args);
    if (name === "ContactCard/get") {
      return Response.json({
        methodResponses: [
          [
            "ContactCard/get",
            {
              accountId,
              list: [
                {
                  id: "k1",
                  media: {
                    a: {
                      kind: "photo",
                      uri: "https://photos.example.com/a.jpg",
                    },
                    b: { kind: "photo", uri: "data:image/png;base64,AA" },
                    c: {
                      kind: "logo",
                      uri: "https://photos.example.com/l.png",
                    },
                    d: {
                      kind: "photo",
                      uri: ["javascript", "alert(1)"].join(":"),
                    },
                  },
                },
              ],
              notFound: [],
              state: "s",
            },
            callId,
          ],
        ],
        sessionState: "s1",
      });
    }
    const list =
      html === null
        ? []
        : [
            {
              bodyValues: {
                "1": {
                  isEncodingProblem: false,
                  isTruncated: false,
                  value: html,
                },
              },
              htmlBody: [{ partId: "1", type: "text/html" }],
              id: "e1",
            },
          ];
    return Response.json({
      methodResponses: [
        [
          "Email/get",
          {
            accountId,
            list,
            notFound: html === null ? ["e1"] : [],
            state: "s",
          },
          callId,
        ],
      ],
      sessionState: "s1",
    });
  };
  return { calls, fetch: fakeFetch };
};

const IMAGES: ImageTicketConfig = {
  imageUrl: (origin, ticket) => `${origin}/i/${ticket}`,
  maxImages: 60,
  proxyOrigin: "https://img.example.com",
  sign: async (claims: TicketClaims) =>
    await Promise.resolve(
      `t.${btoa(JSON.stringify(claims)).replaceAll("=", "")}`
    ),
  ttlMs: 10 * 60_000,
};

describe(handleImageTickets, () => {
  let auth: AuthConfig;
  let cookie: string;

  beforeEach(async () => {
    const keys = await parseSealKeys(KEY);
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
  });

  const post = (body: unknown, site = "same-origin"): Request =>
    new Request(`${APP.origin}/api/images/tickets`, {
      body: typeof body === "string" ? body : JSON.stringify(body),
      headers: { cookie, "sec-fetch-site": site },
      method: "POST",
    });

  const run = async (
    request: Request,
    html: string | null = HTML,
    images: ImageTicketConfig | null = IMAGES,
    extractor: ImageUrlExtractor = inProcessExtractor(
      new new JSDOM("").window.DOMParser()
    ),
    rateLimit: RateLimiter = createRateLimiter(60, 60_000)
  ) => {
    const jmap = createJmap(html);
    const response = await handleImageTickets(
      request,
      auth,
      images ?? undefined,
      { extractor, fetch: jmap.fetch, rateLimit }
    );
    return { calls: jmap.calls, response };
  };

  it("an email the parser cannot read in time gets no tickets (422)", async () => {
    const { response } = await run(post({ emailId: "e1" }), HTML, IMAGES, {
      extract: async () => await Promise.resolve(null),
    });
    expect(response.status).toBe(422);
    await expect(response.json()).resolves.toStrictEqual({
      error: "unparsable",
    });
  });

  it("tickets the email's own images, once each, without trackers", async () => {
    const { response } = await run(post({ emailId: "e1" }));
    const body = TicketsSchema.parse(await response.json());

    expect(response.status).toBe(200);
    expect(body.images.map((image) => image.url)).toStrictEqual([
      "https://cdn.example.com/banner.png",
      "https://cdn.example.com/bg.jpg",
    ]);
    expect(body.trackersBlocked).toBeGreaterThanOrEqual(1);
  });

  it("signs claims bound to the account, the email and a short expiry", async () => {
    const { response } = await run(post({ emailId: "e1" }));
    const body = TicketsSchema.parse(await response.json());
    const [first] = body.images;
    const claims: unknown = JSON.parse(atob(first?.ticket.slice(2) ?? ""));

    expect(claims).toStrictEqual({
      accountId: "acc1",
      emailId: "e1",
      expiresAt: NOW + IMAGES.ttlMs,
      url: "https://cdn.example.com/banner.png",
    });
    expect(first?.src).toBe(`https://img.example.com/i/${first?.ticket}`);
  });

  it("downloads the email with the session's token, never trusting the browser's list", async () => {
    const { calls } = await run(post({ emailId: "e1" }));

    expect(calls.map((call) => call.authorization)).toStrictEqual([
      "Bearer access-1",
      "Bearer access-1",
    ]);
    expect(JSON.stringify(calls[1]?.body)).toContain('"ids":["e1"]');
  });

  it("caps the number of images", async () => {
    const many = Array.from(
      { length: 5 },
      (_, i) => `<img src="https://cdn.example.com/${i}.png">`
    ).join("");
    const { response } = await run(post({ emailId: "e1" }), many, {
      ...IMAGES,
      maxImages: 3,
    });
    const body = TicketsSchema.parse(await response.json());

    expect([body.images.length, body.omitted]).toStrictEqual([3, 2]);
  });

  it("skips an image it can't sign instead of failing the request", async () => {
    const picky: ImageTicketConfig = {
      ...IMAGES,
      sign: async (claims) =>
        claims.url.endsWith("bg.jpg")
          ? await Promise.reject(new Error("URL too long"))
          : await IMAGES.sign(claims),
    };
    const { response } = await run(post({ emailId: "e1" }), HTML, picky);
    const body = TicketsSchema.parse(await response.json());

    expect([body.images.map((image) => image.url), body.omitted]).toStrictEqual(
      [["https://cdn.example.com/banner.png"], 1]
    );
  });

  it.each([
    ["an extra field", { emailId: "e1", urls: ["https://evil.com/x"] }],
    ["a bad id", { emailId: "../x" }],
    ["no id", {}],
    ["not JSON", "nope"],
  ])("rejects %s", async (_name, body) => {
    const { calls, response } = await run(post(body));

    expect(response.status).toBe(400);
    expect(calls).toHaveLength(0);
  });

  it("refuses a huge body as too large", async () => {
    const { calls, response } = await run(post({ emailId: "e".repeat(2000) }));

    expect(response.status).toBe(413);
    expect(calls).toHaveLength(0);
  });

  it("refuses cross-site requests and works only when the proxy is configured", async () => {
    const crossSite = await run(post({ emailId: "e1" }, "cross-site"));
    const disabled = await run(post({ emailId: "e1" }), HTML, null);

    expect([crossSite.response.status, disabled.response.status]).toStrictEqual(
      [403, 404]
    );
  });

  it("answers 404 for an email the user can't see", async () => {
    const { response } = await run(post({ emailId: "e1" }), null);

    expect(response.status).toBe(404);
  });

  it("tickets an email of a shared account with that account", async () => {
    const { calls, response } = await run(
      post({ accountId: "acc2", emailId: "e1" })
    );
    const body = TicketsSchema.parse(await response.json());
    const claims: unknown = JSON.parse(
      atob(body.images[0]?.ticket.slice(2) ?? "")
    );

    expect(JSON.stringify(calls[1]?.body)).toContain('"accountId":"acc2"');
    expect(claims).toMatchObject({ accountId: "acc2", emailId: "e1" });
  });

  it("refuses an account that isn't in the session", async () => {
    const { calls, response } = await run(
      post({ accountId: "other", emailId: "e1" })
    );

    expect(response.status).toBe(404);
    expect(calls).toHaveLength(1);
  });

  it("tickets only a contact's own remote photos", async () => {
    const { calls, response } = await run(post({ contactId: "k1" }));
    const body = TicketsSchema.parse(await response.json());
    const claims: unknown = JSON.parse(
      atob(body.images[0]?.ticket.slice(2) ?? "")
    );

    expect(JSON.stringify(calls[1]?.body)).toContain('"ContactCard/get"');
    expect(body.images.map((image) => image.url)).toStrictEqual([
      "https://photos.example.com/a.jpg",
    ]);
    expect(claims).toMatchObject({ accountId: "acc1", emailId: "card-k1" });
  });

  it("needs a session", async () => {
    const request = new Request(`${APP.origin}/api/images/tickets`, {
      body: JSON.stringify({ emailId: "e1" }),
      headers: { "sec-fetch-site": "same-origin" },
      method: "POST",
    });
    const { calls, response } = await run(request);

    expect(response.status).toBe(401);
    expect(calls).toHaveLength(0);
  });
});

describe(remotePhotoUrls, () => {
  it("keeps http(s) photos and drops data:, logos, scripts and credentials", () => {
    expect(
      remotePhotoUrls({
        a: { kind: "photo", uri: " http://x.example/p.jpg " },
        b: { kind: "photo", uri: "data:image/png;base64,AA" },
        c: { kind: "photo", uri: "https://user:pw@x.example/p.jpg" },
        d: { kind: "photo", uri: "not a url" },
        e: { kind: "logo", uri: "https://x.example/logo.png" },
        f: { kind: "photo" },
        g: { kind: "photo", uri: `https://x.example/${"a".repeat(2100)}` },
      })
    ).toStrictEqual(["http://x.example/p.jpg"]);
    expect(remotePhotoUrls(null)).toStrictEqual([]);
  });
});

describe("handleImageTickets: body limit and rotated cookie", () => {
  it("refuses a 2 KB body before touching the session", async () => {
    const { auth, cookie, tokenCalls } = await refreshingAuth(NOW);
    let jmapCalls = 0;
    const jmapFetch: typeof fetch = async () => {
      jmapCalls += 1;
      return await Promise.reject(new TypeError("not used"));
    };
    const response = await handleImageTickets(
      postWithCookie("/api/images/tickets", cookie, "x".repeat(2048)),
      auth,
      IMAGES,
      {
        fetch: jmapFetch,
        extractor: inProcessExtractor(new new JSDOM("").window.DOMParser()),
        rateLimit: createRateLimiter(60, 60_000),
      }
    );

    expect(response.status).toBe(413);
    expect([tokenCalls(), jmapCalls]).toStrictEqual([0, 0]);
  });

  it("keeps the rotated session cookie when JMAP fails after a refresh", async () => {
    const { auth, cookie, keys, tokenCalls } = await refreshingAuth(NOW);
    const response = await handleImageTickets(
      postWithCookie(
        "/api/images/tickets",
        cookie,
        JSON.stringify({ emailId: "e1" })
      ),
      auth,
      IMAGES,
      {
        fetch: failingFetch,
        extractor: inProcessExtractor(new new JSDOM("").window.DOMParser()),
        rateLimit: createRateLimiter(60, 60_000),
      }
    );

    expect([response.status, tokenCalls()]).toStrictEqual([502, 1]);
    await expect(setSessionRefreshToken(response, keys)).resolves.toBe(
      ROTATED_REFRESH
    );
  });

  it("limits each user to 60 calls a minute, keeping the rotated cookie", async () => {
    const rateLimit = createRateLimiter(60, 60_000);
    const { auth, cookie, keys } = await refreshingAuth(NOW);
    const jmap = createJmap(HTML);
    const call = async () =>
      await handleImageTickets(
        postWithCookie(
          "/api/images/tickets",
          cookie,
          JSON.stringify({ emailId: "e1" })
        ),
        auth,
        IMAGES,
        {
          fetch: jmap.fetch,
          extractor: inProcessExtractor(new new JSDOM("").window.DOMParser()),
          rateLimit,
        }
      );
    for (let i = 0; i < 60; i += 1) {
      // oxlint-disable-next-line no-await-in-loop -- each call counts in order.
      const ok = await call();
      expect(ok.status).toBe(200);
    }
    const before = jmap.calls.length;
    const limited = await call();

    expect(limited.status).toBe(429);
    // Only the session was fetched: no Email/get.
    expect(jmap.calls.length - before).toBe(1);
    await expect(setSessionRefreshToken(limited, keys)).resolves.toBe(
      ROTATED_REFRESH
    );
  });
});
