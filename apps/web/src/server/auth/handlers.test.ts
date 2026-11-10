// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../../mail/image-policy";
import {
  LOGIN_COOKIE,
  PROXY_COOKIE,
  SESSION_COOKIE,
  parseCookies,
} from "./cookies";
import type { AuthConfig } from "./handlers";
import {
  DEFAULT_LIMITS,
  handleCallback,
  handleLogin,
  handleLogout,
  handleToken,
  hasProxyBinding,
  hasSession,
  isSameOrigin,
} from "./handlers";
import type { ProviderMetadata } from "./oidc";
import { parseSealKeys, seal, unseal } from "./seal";

const APP = new URL("https://mail.example.com");
const ISSUER = "https://jmap.example.com";
const KEY_1 = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const KEY_2 = "2:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;
const START = Date.UTC(2026, 9, 23, 12);

const METADATA: ProviderMetadata = {
  authorization_endpoint: `${ISSUER}/login`,
  authorization_response_iss_parameter_supported: true,
  code_challenge_methods_supported: ["S256"],
  issuer: ISSUER,
  token_endpoint: `${ISSUER}/auth/token`,
};

const grantedTokens = (): Response =>
  Response.json({
    access_token: "access-1",
    expires_in: 900,
    refresh_token: "refresh-1",
    token_type: "Bearer",
  });

const REVOCATION_ENDPOINT = `${ISSUER}/oauth/revoke`;

interface TokenCall {
  readonly url: string;
  readonly params: URLSearchParams;
  readonly authorization: string | null;
}

/** A fake OIDC token endpoint whose next answer each test chooses. */
const createProvider = () => {
  const calls: TokenCall[] = [];
  let answer: () => Response = grantedTokens;
  const fakeFetch: typeof fetch = async (input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    if (url !== METADATA.token_endpoint && url !== REVOCATION_ENDPOINT) {
      throw new Error(`Unexpected request to ${url}`);
    }
    // Read the body like a server would, whatever type it was sent as.
    const body = await new Response(init?.body).text();
    calls.push({
      authorization: new Headers(init?.headers).get("authorization"),
      params: new URLSearchParams(body),
      url,
    });
    return answer();
  };
  return {
    calls,
    fetch: fakeFetch,
    setAnswer: (next: () => Response) => {
      answer = next;
    },
  };
};

const setCookies = (response: Response): Map<string, string> =>
  new Map(
    response.headers.getSetCookie().map((cookie) => {
      const [pair = ""] = cookie.split(";");
      const separator = pair.indexOf("=");
      return [pair.slice(0, separator), pair.slice(separator + 1)];
    })
  );

const cookieHeader = (cookies: ReadonlyMap<string, string>): string =>
  [...cookies].map(([name, value]) => `${name}=${value}`).join("; ");

const tokenRequest = (
  cookies: ReadonlyMap<string, string>,
  {
    background = false,
    refresh = false,
    site = "same-origin",
  }: { background?: boolean; refresh?: boolean; site?: string | null } = {}
): Request => {
  const headers = new Headers({ cookie: cookieHeader(cookies) });
  if (site !== null) {
    headers.set("sec-fetch-site", site);
  }
  const url = new URL("/auth/token", APP);
  if (refresh) {
    url.searchParams.set("refresh", "1");
  }
  if (background) {
    url.searchParams.set("background", "1");
  }
  return new Request(url, { headers, method: "POST" });
};

const postWith = (headers: Record<string, string>): Request =>
  new Request(`${APP.origin}/auth/token`, { headers, method: "POST" });

/** Where a callback sends the user, and whether it set a session. */
const outcome = (response: Response) => ({
  location: response.headers.get("location"),
  session: setCookies(response).get(SESSION_COOKIE),
  status: response.status,
});

const failure = (error: string) => ({
  location: `${APP.origin}/login?error=${error}`,
  session: undefined,
  status: 303,
});

describe("auth handlers", () => {
  let provider: ReturnType<typeof createProvider>;
  let now: number;
  let config: AuthConfig;

  beforeEach(async () => {
    provider = createProvider();
    now = START;
    config = {
      appUrl: APP,
      jmapSessionUrl: new URL("/.well-known/jmap", ISSUER),
      keys: await parseSealKeys(KEY_1),
      limits: DEFAULT_LIMITS,
      verifiedSenderBadge: null,
      imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
      metadata: async () => await Promise.resolve(METADATA),
      oidc: {
        clientId: "nebulamail",
        clientSecret: "s3cret value",
        fetch: provider.fetch,
        issuer: new URL(ISSUER),
        issuerId: ISSUER,
        now: () => now,
        redirectUri: new URL("/auth/callback", APP),
      },
    };
  });

  /** Runs /auth/login: the provider URL and the login cookie. */
  const startLogin = async (returnTo = "/inbox") => {
    const response = await handleLogin(
      new Request(
        `${APP.origin}/auth/login?returnTo=${encodeURIComponent(returnTo)}`
      ),
      config
    );
    const location = new URL(response.headers.get("location") ?? "");
    return {
      cookies: setCookies(response),
      location,
      response,
      state: location.searchParams.get("state") ?? "",
    };
  };

  const runCallback = async (
    query: Record<string, string>,
    cookies: ReadonlyMap<string, string>
  ): Promise<Response> =>
    await handleCallback(
      new Request(
        `${APP.origin}/auth/callback?${new URLSearchParams(query).toString()}`,
        { headers: { cookie: cookieHeader(cookies) } }
      ),
      config
    );

  /** Login → callback with whatever the token endpoint answers now. */
  const loginOutcome = async () => {
    const { cookies, state } = await startLogin();
    return outcome(
      await runCallback({ code: "c", iss: ISSUER, state }, cookies)
    );
  };

  /** Full login; returns the session cookie. */
  const login = async (): Promise<Map<string, string>> => {
    const { cookies, state } = await startLogin();
    const response = await runCallback(
      { code: "the-code", iss: ISSUER, state },
      cookies
    );
    return setCookies(response);
  };

  const tokenStatus = async (request: Request): Promise<number> => {
    const response = await handleToken(request, config);
    return response.status;
  };

  describe(seal, () => {
    it("round-trips and rejects other purposes, keys and tampering", async () => {
      const keys = await parseSealKeys(KEY_1);
      const token = await seal(keys, "session", { a: 1 });
      const [version, iv, data = ""] = token.split(".");
      // Change one character of the ciphertext, always to a different one.
      const at = data.length - 2;
      const flipped = `${data.slice(0, at)}${data[at] === "A" ? "B" : "A"}${data.slice(at + 1)}`;
      const otherKeys = await parseSealKeys(KEY_2);

      const results = await Promise.all([
        unseal(keys, "login", token),
        unseal(keys, "session", `${version}.${iv}.${flipped}`),
        unseal(keys, "session", `9.${iv}.${data}`),
        unseal(keys, "session", "garbage"),
        unseal(otherKeys, "session", token),
      ]);

      await expect(unseal(keys, "session", token)).resolves.toStrictEqual({
        a: 1,
      });
      expect(results).toStrictEqual([
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
      ]);
    });

    it("encrypts with the first key and still decrypts with older ones", async () => {
      const old = await seal(await parseSealKeys(KEY_1), "session", "old");
      const rotated = await parseSealKeys(`${KEY_2},${KEY_1}`);
      const fresh = await seal(rotated, "session", "new");

      await expect(unseal(rotated, "session", old)).resolves.toBe("old");
      expect(fresh.startsWith("2.")).toBeTruthy();
    });

    it.each([
      ["", "SESSION_KEYS: each entry"],
      ["1:short", "SESSION_KEYS: each entry"],
      [`${KEY_1},${KEY_1}`, "appears twice"],
      [
        "x:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "SESSION_KEYS: each entry",
      ],
    ])("refuses %j", async (value, message) => {
      await expect(parseSealKeys(value)).rejects.toThrow(message);
    });
  });

  describe(isSameOrigin, () => {
    it.each([
      [{ "sec-fetch-site": "same-origin" }, true],
      [{ "sec-fetch-site": "same-site" }, false],
      [{ origin: APP.origin, "sec-fetch-site": "cross-site" }, false],
      [{ origin: APP.origin }, true],
      [{ origin: "https://evil.com" }, false],
      [{}, false],
    ])("%j → %s", (headers, expected) => {
      expect(isSameOrigin(postWith(headers), APP)).toBe(expected);
    });
  });

  describe("login", () => {
    it("redirects to the provider with PKCE S256", async () => {
      const { location, response } = await startLogin();

      expect(response.status).toBe(303);
      expect(location.origin + location.pathname).toBe(
        METADATA.authorization_endpoint
      );
      const params = Object.fromEntries(location.searchParams);
      expect(params).toMatchObject({
        client_id: "nebulamail",
        code_challenge_method: "S256",
        redirect_uri: `${APP.origin}/auth/callback`,
        response_type: "code",
      });
      // 32 random bytes in base64url.
      expect(`${params.code_challenge} ${params.state}`).toMatch(
        /^[\w-]{43} [\w-]{43}$/u
      );
    });

    it("sets a sealed, HttpOnly, Secure, SameSite=Lax login cookie", async () => {
      const { cookies, response } = await startLogin();
      const header = response.headers.getSetCookie().join("\n");

      expect(cookies.has(LOGIN_COOKIE)).toBeTruthy();
      expect(header).toMatch(/HttpOnly.*Secure|Secure.*HttpOnly/u);
      expect(header).toContain("SameSite=Lax");
      expect(response.headers.get("cache-control")).toBe("no-store");
    });
  });

  describe("callback", () => {
    it("exchanges the code and sets the session cookie", async () => {
      const { cookies, state } = await startLogin("/inbox?x=1");
      const response = await runCallback(
        { code: "the-code", iss: ISSUER, state },
        cookies
      );
      const set = setCookies(response);

      expect(response.headers.get("location")).toBe(`${APP.origin}/inbox?x=1`);
      expect(set.get(LOGIN_COOKIE)).toBe("");
      expect(set.get(SESSION_COOKIE)?.length).toBeGreaterThan(20);
    });

    it("sends the code, the PKCE verifier and client_secret_basic", async () => {
      await login();
      const [call] = provider.calls;

      const params = Object.fromEntries(call?.params ?? []);
      expect(params).toMatchObject({
        code: "the-code",
        grant_type: "authorization_code",
        redirect_uri: `${APP.origin}/auth/callback`,
      });
      expect(params.code_verifier).toMatch(/^[\w-]{43}$/u);
      // Both parts form-encoded first (RFC 6749 §2.3.1).
      expect(call?.authorization).toBe(
        `Basic ${btoa("nebulamail:s3cret+value")}`
      );
    });

    it.each(["/.//evil.com", "/..//evil.com", "/%2e//evil.com"])(
      "never redirects off the app for returnTo %s",
      async (returnTo) => {
        const { cookies, state } = await startLogin(returnTo);
        const response = await runCallback(
          { code: "c", iss: ISSUER, state },
          cookies
        );

        expect(response.headers.get("location")).toBe(`${APP.origin}/`);
      }
    );

    it("rejects a missing or expired login cookie", async () => {
      const { cookies, state } = await startLogin();
      const missing = outcome(
        await runCallback({ code: "c", iss: ISSUER, state }, new Map())
      );
      now += 11 * MINUTE;
      const expired = outcome(
        await runCallback({ code: "c", iss: ISSUER, state }, cookies)
      );

      expect([missing, expired]).toStrictEqual([
        failure("expired"),
        failure("expired"),
      ]);
      expect(provider.calls).toHaveLength(0);
    });

    it("rejects a wrong or missing state", async () => {
      const { cookies } = await startLogin();
      const results = [
        outcome(
          await runCallback({ code: "c", iss: ISSUER, state: "wrong" }, cookies)
        ),
        outcome(await runCallback({ code: "c", iss: ISSUER }, cookies)),
      ];

      expect(results).toStrictEqual([failure("invalid"), failure("invalid")]);
      expect(provider.calls).toHaveLength(0);
    });

    it("rejects another or a missing issuer (RFC 9207)", async () => {
      const { cookies, state } = await startLogin();
      const results = [
        outcome(
          await runCallback(
            { code: "c", iss: "https://evil.com", state },
            cookies
          )
        ),
        outcome(await runCallback({ code: "c", state }, cookies)),
      ];

      expect(results).toStrictEqual([failure("invalid"), failure("invalid")]);
      expect(provider.calls).toHaveLength(0);
    });

    it("maps provider errors to fixed codes", async () => {
      const { cookies, state } = await startLogin();
      const results = [
        outcome(
          await runCallback(
            { error: "access_denied", iss: ISSUER, state },
            cookies
          )
        ),
        outcome(
          await runCallback(
            { error: "<script>", error_description: "x", iss: ISSUER, state },
            cookies
          )
        ),
      ];

      expect(results).toStrictEqual([failure("denied"), failure("invalid")]);
    });

    it.each([
      [
        "a refused code",
        () => Response.json({ error: "invalid_grant" }, { status: 400 }),
        "invalid",
      ],
      [
        "a server error",
        () => new Response("oops", { status: 502 }),
        "unavailable",
      ],
      [
        "no refresh token",
        () =>
          Response.json({
            access_token: "a",
            expires_in: 900,
            token_type: "Bearer",
          }),
        "invalid",
      ],
      [
        "a non-Bearer token",
        () =>
          Response.json({
            access_token: "a",
            expires_in: 900,
            refresh_token: "r",
            token_type: "mac",
          }),
        "unavailable",
      ],
    ])("fails on %s", async (_name, answer, error) => {
      provider.setAnswer(answer);
      const result = await loginOutcome();

      expect(result).toStrictEqual(failure(error));
    });
  });

  describe("token", () => {
    it("returns the current access token without calling the provider", async () => {
      const session = await login();
      const response = await handleToken(tokenRequest(session), config);
      const body: unknown = await response.json();

      expect(response.status).toBe(200);
      expect(body).toStrictEqual({
        accessToken: "access-1",
        expiresAt: START + 900_000,
        imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
        sessionUrl: `${ISSUER}/.well-known/jmap`,
        verifiedSenderBadge: null,
      });
      expect(response.headers.get("cache-control")).toBe("no-store");
      expect(provider.calls).toHaveLength(1);
    });

    it("refuses cross-site requests, requests without origin and GET", async () => {
      const session = await login();
      const get = new Request(`${APP.origin}/auth/token`, {
        headers: {
          cookie: cookieHeader(session),
          "sec-fetch-site": "same-origin",
        },
      });
      const statuses = await Promise.all([
        tokenStatus(tokenRequest(session, { site: "cross-site" })),
        tokenStatus(tokenRequest(session, { site: null })),
        tokenStatus(get),
      ]);

      expect(statuses).toStrictEqual([403, 403, 403]);
    });

    it("refreshes near expiry and keeps the refresh token Stalwart didn't replace", async () => {
      const session = await login();
      now += 14 * MINUTE;
      provider.setAnswer(() =>
        Response.json({
          access_token: "access-2",
          expires_in: 900,
          token_type: "bearer",
        })
      );
      const first = await handleToken(tokenRequest(session), config);
      const body: unknown = await first.json();
      now += 14 * MINUTE;
      await handleToken(tokenRequest(setCookies(first)), config);

      expect(body).toMatchObject({ accessToken: "access-2" });
      expect(
        provider.calls.map((call) => call.params.get("refresh_token"))
      ).toStrictEqual([null, "refresh-1", "refresh-1"]);
    });

    it("keeps the refresh token Stalwart 1.0 rotates on every refresh", async () => {
      const session = await login();
      now += 14 * MINUTE;
      provider.setAnswer(() =>
        Response.json({
          access_token: "access-2",
          expires_in: 900,
          refresh_token: "refresh-2",
          token_type: "bearer",
        })
      );
      const first = await handleToken(tokenRequest(session), config);
      now += 14 * MINUTE;
      await handleToken(tokenRequest(setCookies(first)), config);

      expect(
        provider.calls.map((call) => call.params.get("refresh_token"))
      ).toStrictEqual([null, "refresh-1", "refresh-2"]);
    });

    it("doesn't rewrite the cookie without a refresh, except to move lastSeenAt hourly", async () => {
      // A long access token, so that no refresh happens in between.
      provider.setAnswer(() =>
        Response.json({
          access_token: "access-1",
          expires_in: 3 * 60 * 60,
          refresh_token: "refresh-1",
          token_type: "bearer",
        })
      );
      const session = await login();
      now += 30 * MINUTE;
      const early = await handleToken(tokenRequest(session), config);
      now += 31 * MINUTE;
      const late = await handleToken(tokenRequest(session), config);

      expect(early.status).toBe(200);
      // A slow answer with a cookie read before a parallel refresh must not
      // put back the refresh token that refresh replaced.
      expect(early.headers.getSetCookie()).toStrictEqual([]);
      expect(late.status).toBe(200);
      expect(setCookies(late).has(SESSION_COOKIE)).toBeTruthy();
      expect(provider.calls).toHaveLength(1);
    });

    it("parallel requests near expiry share one refresh", async () => {
      const session = await login();
      now += 15 * MINUTE;
      const before = provider.calls.length;
      const responses = await Promise.all(
        Array.from(
          { length: 5 },
          async () => await handleToken(tokenRequest(session), config)
        )
      );
      expect(responses.map((response) => response.status)).toStrictEqual([
        200, 200, 200, 200, 200,
      ]);
      // One refresh, and every answer carries its result.
      expect(provider.calls.length - before).toBe(1);
      const tokens = await Promise.all(
        responses.map(async (response) => {
          const body: unknown = await response.json();
          return typeof body === "object" &&
            body !== null &&
            "accessToken" in body
            ? body.accessToken
            : undefined;
        })
      );
      expect(new Set(tokens).size).toBe(1);
      expect(
        responses.every((response) => setCookies(response).has(SESSION_COOKIE))
      ).toBeTruthy();
    });

    it("never writes back a cookie whose refresh token was just replaced", async () => {
      provider.setAnswer(() =>
        Response.json({
          access_token: "[SECRET]",
          expires_in: 3 * 60 * 60,
          refresh_token: "[SECRET]",
          token_type: "bearer",
        })
      );
      const session = await login();
      // An hour without use: the next request would move lastSeenAt.
      now += 61 * MINUTE;
      // One request forced a refresh with this cookie (a 401)…
      await handleToken(tokenRequest(session, { refresh: true }), config);
      // …and a parallel one still carries the old cookie: writing it would
      // put back the replaced refresh token.
      const late = await handleToken(tokenRequest(session), config);
      expect(late.status).toBe(200);
      expect(late.headers.getSetCookie()).toStrictEqual([]);
    });

    it("forces a refresh with ?refresh=1", async () => {
      const session = await login();
      await handleToken(tokenRequest(session, { refresh: true }), config);

      expect(provider.calls.at(-1)?.params.get("grant_type")).toBe(
        "refresh_token"
      );
    });

    it("keeps the session when the server is down", async () => {
      const session = await login();
      now += 15 * MINUTE;
      provider.setAnswer(() => new Response(null, { status: 503 }));
      const response = await handleToken(tokenRequest(session), config);

      expect(response.status).toBe(503);
      expect(setCookies(response).has(SESSION_COOKIE)).toBeFalsy();
    });

    it("ends the session when the refresh token is rejected", async () => {
      const session = await login();
      now += 15 * MINUTE;
      provider.setAnswer(() =>
        Response.json({ error: "invalid_grant" }, { status: 400 })
      );
      const response = await handleToken(tokenRequest(session), config);

      expect(response.status).toBe(401);
      expect(setCookies(response).get(SESSION_COOKIE)).toBe("");
    });

    it.each([
      [429, "rate_limited"],
      [400, "invalid_request"],
      [400, "unsupported_grant_type"],
    ])(
      "keeps the session on a %i %s from the token endpoint",
      async (status, error) => {
        const session = await login();
        now += 15 * MINUTE;
        provider.setAnswer(() => Response.json({ error }, { status }));
        const response = await handleToken(tokenRequest(session), config);

        expect(response.status).toBe(503);
        expect(response.headers.getSetCookie()).toStrictEqual([]);
      }
    );

    it.each(["invalid_grant", "invalid_client", "unauthorized_client"])(
      "ends the session on %s",
      async (code) => {
        const session = await login();
        now += 15 * MINUTE;
        provider.setAnswer(() =>
          Response.json({ error: code }, { status: 400 })
        );
        const response = await handleToken(tokenRequest(session), config);

        expect(response.status).toBe(401);
        expect(setCookies(response).get(SESSION_COOKIE)).toBe("");
      }
    );

    it("doesn't move lastSeenAt for the service worker's background requests", async () => {
      provider.setAnswer(() =>
        Response.json({
          access_token: "access-1",
          expires_in: 3 * 60 * 60,
          refresh_token: "refresh-1",
          token_type: "bearer",
        })
      );
      const session = await login();
      now += 2 * 60 * MINUTE;
      const response = await handleToken(
        tokenRequest(session, { background: true }),
        config
      );

      expect(response.status).toBe(200);
      expect(response.headers.getSetCookie()).toStrictEqual([]);
    });

    it("rotates the refresh token but keeps lastSeenAt in the background", async () => {
      const session = await login();
      now += 15 * MINUTE;
      provider.setAnswer(() =>
        Response.json({
          access_token: "access-2",
          expires_in: 900,
          refresh_token: "refresh-2",
          token_type: "bearer",
        })
      );
      const response = await handleToken(
        tokenRequest(session, { background: true }),
        config
      );
      const sealed = setCookies(response).get(SESSION_COOKIE) ?? "";

      expect(response.status).toBe(200);
      await expect(
        unseal(config.keys, "session", sealed)
      ).resolves.toMatchObject({
        lastSeenAt: START,
        refreshToken: "refresh-2",
      });
    });

    it("expires idle sessions also for background requests", async () => {
      const session = await login();
      now += DEFAULT_LIMITS.idleMs + 1;

      await expect(
        tokenStatus(tokenRequest(session, { background: true }))
      ).resolves.toBe(401);
    });

    it("expires idle sessions", async () => {
      const session = await login();
      now += DEFAULT_LIMITS.idleMs + 1;

      await expect(tokenStatus(tokenRequest(session))).resolves.toBe(401);
    });

    it("expires sessions after maxAgeMs even if used every day", async () => {
      let cookies = await login();
      const statuses: number[] = [];
      for (let day = 1; day <= 31; day += 1) {
        now = START + day * DAY;
        // oxlint-disable-next-line no-await-in-loop -- each step uses the previous cookie.
        const response = await handleToken(tokenRequest(cookies), config);
        statuses.push(response.status);
        cookies = response.status === 200 ? setCookies(response) : cookies;
      }

      // Day 30 is exactly maxAgeMs: still valid. Day 31 is past it.
      expect(DEFAULT_LIMITS.maxAgeMs).toBe(30 * DAY);
      expect(statuses).toStrictEqual([
        ...Array.from({ length: 30 }, () => 200),
        401,
      ]);
    });

    it("ignores forged, foreign and swapped cookies", async () => {
      const forged = new Map([[SESSION_COOKIE, "1.AAAAAAAAAAAAAAAA.AAAA"]]);
      const session = await login();
      const otherKeys = { ...config, keys: await parseSealKeys(KEY_2) };
      const { cookies } = await startLogin();
      // A login cookie can't be used as a session cookie.
      const swapped = new Map([
        [SESSION_COOKIE, cookies.get(LOGIN_COOKIE) ?? ""],
      ]);
      const foreign = await handleToken(tokenRequest(session), otherKeys);
      const statuses = await Promise.all([
        tokenStatus(tokenRequest(forged)),
        tokenStatus(tokenRequest(swapped)),
      ]);

      expect([...statuses, foreign.status]).toStrictEqual([401, 401, 401]);
    });
  });

  describe("full BFF mode (JMAP_PROXY=on)", () => {
    beforeEach(() => {
      config = { ...config, jmapProxy: true };
    });

    it("sets a SameSite=Strict proxy cookie bound to the session", async () => {
      const { cookies, state } = await startLogin();
      const response = await runCallback(
        { code: "the-code", iss: ISSUER, state },
        cookies
      );
      const proxy = response.headers
        .getSetCookie()
        .find((cookie) => cookie.startsWith(`${PROXY_COOKIE}=`));
      expect(proxy).toMatch(/; SameSite=Strict$/u);
      expect(proxy).toMatch(/; HttpOnly; /u);
      // The session cookie stays Lax: links from other sites still work.
      const session = response.headers
        .getSetCookie()
        .find((cookie) => cookie.startsWith(`${SESSION_COOKIE}=`));
      expect(session).toMatch(/; SameSite=Lax$/u);
      const all = setCookies(response);
      // Without the Strict cookie, or with another value: no.
      const withoutProxy = new Map(all);
      withoutProxy.delete(PROXY_COOKIE);
      const forged = new Map([...withoutProxy, [PROXY_COOKIE, "forged"]]);
      const bindings = await Promise.all(
        [all, withoutProxy, forged].map(
          async (jar) =>
            await hasProxyBinding(
              new Request(APP, { headers: { cookie: cookieHeader(jar) } }),
              config
            )
        )
      );
      expect(bindings).toStrictEqual([true, false, false]);
    });

    it("never hands out a token: /auth/token is 404", async () => {
      const session = await login();
      const response = await handleToken(tokenRequest(session), config);
      expect(response.status).toBe(404);
      const body: unknown = await response.json();
      expect(body).toStrictEqual({ error: "jmap-proxy" });
      expect(response.headers.getSetCookie()).toStrictEqual([]);
    });

    it("a session from before the proxy has no binding", async () => {
      config = { ...config, jmapProxy: false };
      const session = await login();
      config = { ...config, jmapProxy: true };
      await expect(
        hasProxyBinding(tokenRequest(session), config)
      ).resolves.toBeFalsy();
      // …and SSR sees no session, so /login shows the form instead of
      // sending the user back to an app that can't load (a loop).
      await expect(
        hasSession(tokenRequest(session), config)
      ).resolves.toBeFalsy();
      // A session made with the proxy on still counts, even when a link
      // from another site leaves out the Strict cookie.
      const bound = await login();
      await expect(
        hasSession(tokenRequest(bound), config)
      ).resolves.toBeTruthy();
    });

    it("logout clears both cookies, and the binding is gone", async () => {
      const session = await login();
      const response = await handleLogout(tokenRequest(session), config);
      const cleared = setCookies(response);
      expect(cleared.get(SESSION_COOKIE)).toBe("");
      expect(cleared.get(PROXY_COOKIE)).toBe("");
      expect(
        response.headers
          .getSetCookie()
          .find((cookie) => cookie.startsWith(`${PROXY_COOKIE}=`))
      ).toMatch(/Max-Age=0; .*SameSite=Strict$/u);
      // The browser applies them: nothing left to authorize the proxy.
      await expect(
        hasProxyBinding(new Request(APP), config)
      ).resolves.toBeFalsy();
    });
  });

  describe("logout and hasSession", () => {
    it("clears the cookie only for same-origin POST", async () => {
      const session = await login();
      const crossSite = await handleLogout(
        tokenRequest(session, { site: "cross-site" }),
        config
      );
      const response = await handleLogout(tokenRequest(session), config);

      await expect(
        hasSession(tokenRequest(session), config)
      ).resolves.toBeTruthy();
      expect(crossSite.status).toBe(403);
      expect(response.status).toBe(200);
      expect(setCookies(response).get(SESSION_COOKIE)).toBe("");
      await expect(hasSession(new Request(APP), config)).resolves.toBeFalsy();
    });

    it("revokes the refresh token when the provider supports it (RFC 7009)", async () => {
      const session = await login();
      // Without a revocation endpoint (Stalwart before 1.0): only the cookie goes.
      await handleLogout(tokenRequest(session), config);
      expect(
        provider.calls.some((c) => c.url === REVOCATION_ENDPOINT)
      ).toBeFalsy();

      config = {
        ...config,
        metadata: async () =>
          await Promise.resolve({
            ...METADATA,
            revocation_endpoint: REVOCATION_ENDPOINT,
          }),
      };
      const response = await handleLogout(tokenRequest(session), config);
      const revocation = provider.calls.find(
        (c) => c.url === REVOCATION_ENDPOINT
      );
      expect({
        basicAuth: revocation?.authorization?.startsWith("Basic ") ?? false,
        hint: revocation?.params.get("token_type_hint"),
        status: response.status,
        token: revocation?.params.get("token"),
      }).toStrictEqual({
        basicAuth: true,
        hint: "refresh_token",
        status: 200,
        token: "refresh-1",
      });

      // A failing revocation doesn't block the logout.
      provider.setAnswer(() => {
        throw new Error("network down");
      });
      const offline = await handleLogout(tokenRequest(session), config);
      expect([
        offline.status,
        setCookies(offline).get(SESSION_COOKIE),
      ]).toStrictEqual([200, ""]);
    });

    it("parses cookie headers, first value wins", () => {
      expect(parseCookies("a=1; b=2; a=3").get("a")).toBe("1");
      expect(parseCookies(null).size).toBe(0);
    });
  });
});
