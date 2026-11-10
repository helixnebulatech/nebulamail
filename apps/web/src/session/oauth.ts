// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// OAuth 2.1 in the browser for the static build: public
// client, code + PKCE S256, `state`, `iss` (RFC 9207). Pure helpers, shared
// by the page (browser.ts) and the session worker (worker.ts).
//
// Every endpoint is used on the page's own origin. Stalwart serves the app
// and answers every path on every host name, but writes its public name in
// the metadata: keeping the requests here means no CORS and
// `connect-src 'self'`, and the login form shows the name the user typed.

import { z } from "zod";

import { OAUTH_SCOPE } from "../lib/oauth-scope";
import { ExpiresIn, tokenExpiresAt } from "../lib/token-lifetime";

export const MetadataSchema = z.object({
  authorization_endpoint: z.url(),
  authorization_response_iss_parameter_supported: z.boolean().optional(),
  code_challenge_methods_supported: z.array(z.string()).optional(),
  issuer: z.url(),
  /** RFC 7009; Stalwart has one since 1.0. */
  revocation_endpoint: z.url().optional(),
  token_endpoint: z.url(),
});

export type ProviderMetadata = z.infer<typeof MetadataSchema>;

const TIMEOUT_MS = 10_000;

export const base64Url = (bytes: Uint8Array): string =>
  btoa(String.fromCodePoint(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/[=]+$/u, "");

/** `bytes` random bytes in base64url. */
export const randomToken = (bytes = 32): string =>
  base64Url(crypto.getRandomValues(new Uint8Array(bytes)));

export const pkceChallenge = async (verifier: string): Promise<string> =>
  base64Url(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))
    )
  );

/** The same path on `origin` (see the header comment). */
export const onOrigin = (endpoint: string, origin: string): URL => {
  const url = new URL(endpoint);
  const local = new URL(origin);
  local.pathname = url.pathname;
  local.search = url.search;
  return local;
};

/** The server's OAuth metadata (RFC 8414), from this origin. */
export const fetchMetadata = async (
  origin: string,
  fetchImpl: typeof fetch = fetch
): Promise<ProviderMetadata> => {
  const response = await fetchImpl(
    new URL("/.well-known/oauth-authorization-server", origin),
    {
      credentials: "omit",
      headers: { accept: "application/json" },
      redirect: "error",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    }
  );
  if (!response.ok) {
    throw new Error(`OAuth discovery failed: HTTP ${response.status}`);
  }
  const metadata = MetadataSchema.parse(await response.json());
  if (metadata.code_challenge_methods_supported?.includes("S256") === false) {
    throw new Error("The server does not support PKCE S256");
  }
  return metadata;
};

export interface AuthorizationRequest {
  readonly clientId: string;
  readonly redirectUri: string;
  readonly state: string;
  readonly codeChallenge: string;
}

export const authorizationUrl = (
  metadata: ProviderMetadata,
  origin: string,
  request: AuthorizationRequest
): URL => {
  const url = onOrigin(metadata.authorization_endpoint, origin);
  url.search = new URLSearchParams({
    client_id: request.clientId,
    code_challenge: request.codeChallenge,
    code_challenge_method: "S256",
    redirect_uri: request.redirectUri,
    response_type: "code",
    // Chosen at build time (NEBULA_OAUTH_SERVICES, vite.config.ts); all
    // four when not set (tests).
    scope: import.meta.env.NEBULA_OAUTH_SCOPE || OAUTH_SCOPE,
    state: request.state,
  }).toString();
  return url;
};

/** Constant-time comparison of two short strings. */
const safeEqual = (a: string, b: string): boolean => {
  if (a.length !== b.length) {
    return false;
  }
  let difference = 0;
  for (let index = 0; index < a.length; index += 1) {
    // oxlint-disable-next-line no-bitwise -- constant-time comparison.
    difference |= (a.codePointAt(index) ?? 0) ^ (b.codePointAt(index) ?? 0);
  }
  return difference === 0;
};

export type CallbackResult =
  | { readonly ok: true; readonly code: string }
  | { readonly ok: false; readonly error: "denied" | "invalid" };

/**
 * Checks the authorization response: our `state`, the server's `iss` (RFC
 * 9207, mix-up attacks), then `error` or `code`. Same order as the BFF.
 */
export const parseCallback = (
  params: URLSearchParams,
  expectedState: string,
  metadata: ProviderMetadata
): CallbackResult => {
  if (!safeEqual(params.get("state") ?? "", expectedState)) {
    return { error: "invalid", ok: false };
  }
  const iss = params.get("iss");
  if (
    (iss !== null ||
      metadata.authorization_response_iss_parameter_supported === true) &&
    iss !== metadata.issuer
  ) {
    return { error: "invalid", ok: false };
  }
  if (params.has("error")) {
    return {
      error: params.get("error") === "access_denied" ? "denied" : "invalid",
      ok: false,
    };
  }
  const code = params.get("code");
  return code === null || code === ""
    ? { error: "invalid", ok: false }
    : { code, ok: true };
};

// `returnTo` only accepts paths inside the app: one check,
// shared with the BFF.
export { safeReturnTo } from "../lib/return-to";

const TokenResponse = z.object({
  access_token: z.string().min(1),
  // RFC 6749 §5.1: RECOMMENDED; missing means our default (lib/token-lifetime.ts).
  expires_in: ExpiresIn,
  refresh_token: z.string().min(1).optional(),
  token_type: z.string().refine((type) => type.toLowerCase() === "bearer"),
});

const ErrorResponse = z.object({ error: z.string() });

export interface Tokens {
  readonly accessToken: string;
  /** Unix time in ms. */
  readonly expiresAt: number;
  readonly refreshToken: string | undefined;
}

export type TokenResult =
  | { readonly ok: true; readonly tokens: Tokens }
  /** `invalid_grant` and friends: the session is over, log in again. */
  | { readonly ok: false; readonly kind: "rejected"; readonly error: string }
  /** Network error, timeout, 5xx or a malformed answer: try again later. */
  | { readonly ok: false; readonly kind: "unavailable" };

/** A token request as a public client (`client_id` in the body, no secret). */
export const tokenRequest = async (
  endpoint: URL,
  params: Readonly<Record<string, string>>,
  now: () => number = Date.now,
  fetchImpl: typeof fetch = fetch
): Promise<TokenResult> => {
  let response: Response;
  try {
    response = await fetchImpl(endpoint, {
      body: new URLSearchParams(params),
      credentials: "omit",
      headers: {
        accept: "application/json",
        "content-type": "application/x-www-form-urlencoded",
      },
      method: "POST",
      // Credentials must never follow a redirect.
      redirect: "error",
      referrerPolicy: "no-referrer",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch {
    return { kind: "unavailable", ok: false };
  }
  const body: unknown = await response.json().catch(() => null);
  if (response.ok) {
    const parsed = TokenResponse.safeParse(body);
    return parsed.success
      ? {
          ok: true,
          tokens: {
            accessToken: parsed.data.access_token,
            expiresAt: tokenExpiresAt(now(), parsed.data.expires_in),
            refreshToken: parsed.data.refresh_token,
          },
        }
      : { kind: "unavailable", ok: false };
  }
  const error = ErrorResponse.safeParse(body);
  return response.status >= 400 && response.status < 500 && error.success
    ? { error: error.data.error, kind: "rejected", ok: false }
    : { kind: "unavailable", ok: false };
};

/**
 * Signing out waits for the revocation (a dedicated worker dies with its
 * page), but not long.
 */
const REVOKE_TIMEOUT_MS = 5000;

/**
 * Revokes a refresh token as a public client (RFC 7009: `client_id` in the
 * body, no secret). Stalwart 1.0 then deletes the whole grant, so the access
 * tokens a page still holds stop working too. Best effort: signing out must
 * work even if this fails, and the answer doesn't matter (always 200).
 */
export const revokeRequest = async (
  endpoint: URL,
  params: { readonly clientId: string; readonly refreshToken: string },
  fetchImpl: typeof fetch = fetch
): Promise<void> => {
  try {
    await fetchImpl(endpoint, {
      body: new URLSearchParams({
        client_id: params.clientId,
        token: params.refreshToken,
        token_type_hint: "refresh_token",
      }),
      credentials: "omit",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      method: "POST",
      redirect: "error",
      referrerPolicy: "no-referrer",
      signal: AbortSignal.timeout(REVOKE_TIMEOUT_MS),
    });
  } catch {
    // Nothing to do: the worker forgets the session anyway.
  }
};
