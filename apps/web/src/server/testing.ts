// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test helpers for the BFF endpoints that call JMAP with the session's
// token: a session about to expire (so the request refreshes it) and a fake
// token endpoint that rotates the refresh token. Only imported by tests.

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../mail/image-policy";
import { SESSION_COOKIE, parseCookies } from "./auth/cookies";
import type { AuthConfig } from "./auth/handlers";
import { DEFAULT_LIMITS } from "./auth/handlers";
import type { ProviderMetadata } from "./auth/oidc";
import type { SealKeys } from "./auth/seal";
import { parseSealKeys, seal, unseal } from "./auth/seal";

export const TEST_ISSUER = "https://jmap.example.com";

export const TEST_METADATA: ProviderMetadata = {
  authorization_endpoint: `${TEST_ISSUER}/login`,
  issuer: TEST_ISSUER,
  token_endpoint: `${TEST_ISSUER}/auth/token`,
};

/** The refresh token the fake provider hands out on refresh. */
export const ROTATED_REFRESH = "refresh-2";

/** A token endpoint that answers every refresh with a rotated token. */
export const rotatingTokenFetch: typeof fetch = async (input) => {
  const url = input instanceof Request ? input.url : String(input);
  if (url !== TEST_METADATA.token_endpoint) {
    throw new Error(`Unexpected request to ${url}`);
  }
  return await Promise.resolve(
    Response.json({
      access_token: "access-2",
      expires_in: 900,
      refresh_token: ROTATED_REFRESH,
      token_type: "Bearer",
    })
  );
};

/** A session cookie whose access token expires within the refresh margin. */
export const expiringSessionCookie = async (
  keys: SealKeys,
  now: number
): Promise<string> =>
  `${SESSION_COOKIE}=${await seal(keys, "session", {
    accessToken: "access-1",
    accessTokenExpiresAt: now + 60_000,
    createdAt: now,
    lastSeenAt: now,
    refreshToken: "refresh-1",
    v: 1,
  })}`;

/** The refresh token in the session cookie a response sets, if any. */
export const setSessionRefreshToken = async (
  response: Response,
  keys: SealKeys
): Promise<unknown> => {
  const header = response.headers
    .getSetCookie()
    .map((cookie) => cookie.split(";")[0] ?? "")
    .join("; ");
  const value = parseCookies(header).get(SESSION_COOKIE);
  if (value === undefined || value === "") {
    return undefined;
  }
  const session: unknown = await unseal(keys, "session", value);
  return typeof session === "object" && session !== null
    ? (session as { readonly refreshToken?: unknown }).refreshToken
    : undefined;
};

export const TEST_APP = new URL("https://mail.example.com");
const TEST_KEY = "1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/**
 * An AuthConfig whose session is about to expire, with a token endpoint that
 * rotates the refresh token and counts its calls.
 */
export const refreshingAuth = async (now: number) => {
  const keys = await parseSealKeys(TEST_KEY);
  let tokenCalls = 0;
  const auth: AuthConfig = {
    appUrl: TEST_APP,
    imagePolicy: DEFAULT_ADMIN_IMAGE_POLICY,
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
      now: () => now,
      redirectUri: new URL("/auth/callback", TEST_APP),
    },
    verifiedSenderBadge: null,
  };
  const cookie = await expiringSessionCookie(keys, now);
  return { auth, cookie, keys, tokenCalls: () => tokenCalls };
};

/** A same-origin POST to the app with that cookie. */
export const postWithCookie = (
  path: string,
  cookie: string,
  body: string
): Request =>
  new Request(new URL(path, TEST_APP), {
    body,
    headers: { cookie, "sec-fetch-site": "same-origin" },
    method: "POST",
  });

/** A fetch that always fails like a network error. */
export const failingFetch: typeof fetch = async () =>
  await Promise.reject(new TypeError("network down"));
