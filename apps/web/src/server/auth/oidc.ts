// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// OAuth 2.1 against the mail server's OIDC provider (Stalwart), as a
// confidential client with PKCE.
//
// The server is fixed by the admin (JMAP_SERVER), not typed by users, so these
// requests don't go through net-guard. If a "choose your server" screen is
// ever added, discovery must go through net-guard first.

import { z } from "zod";

import { OAUTH_SCOPE } from "../../lib/oauth-scope";
import { ExpiresIn, tokenExpiresAt } from "../../lib/token-lifetime";

const TIMEOUT_MS = 10_000;

const Metadata = z.object({
  authorization_endpoint: z.url(),
  authorization_response_iss_parameter_supported: z.boolean().optional(),
  code_challenge_methods_supported: z.array(z.string()).optional(),
  issuer: z.url(),
  /** RFC 7009. Stalwart has one since 1.0, the bridge too. */
  revocation_endpoint: z.url().optional(),
  token_endpoint: z.url(),
});

export type ProviderMetadata = z.infer<typeof Metadata>;

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

export interface OidcClient {
  /** The issuer as a URL (its origin is the JMAP server's). */
  readonly issuer: URL;
  /**
   * The issuer identifier exactly as configured (JMAP_SERVER, as written):
   * what discovery's `issuer` must equal, character for character.
   */
  readonly issuerId: string;
  readonly clientId: string;
  readonly clientSecret: string;
  readonly redirectUri: URL;
  /** The `scope` to ask for (lib/oauth-scope.ts); all four when missing. */
  readonly scope?: string;
  readonly fetch: typeof fetch;
  readonly now: () => number;
}

const isAllowedEndpoint = (url: URL, issuer: URL): boolean =>
  url.protocol === "https:" ||
  // Plain http only for a local development server.
  (url.protocol === "http:" &&
    issuer.protocol === "http:" &&
    ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname));

/**
 * OIDC discovery. The returned issuer must be identical to the configured
 * issuer identifier (OpenID Connect Discovery 1.0 §4.3; RFC 8414 §3.3): a
 * different value means a misconfigured or spoofed provider. No
 * normalisation: `https://mail.example.com/` and `https://mail.example.com`
 * are different issuers, so JMAP_SERVER must be written exactly as the
 * server publishes it (Stalwart: its public URL without a trailing slash).
 *
 * First RFC 8414's document (§3: `/.well-known/oauth-authorization-server`
 * inserted between the host and the issuer's path; this is OAuth, no ID
 * tokens are used), then, only on a 404, OpenID Connect's
 * (`/.well-known/openid-configuration` after the issuer with any final
 * "/" removed, Discovery §4.1) for providers that publish only that one.
 */
export const discover = async (
  client: Pick<OidcClient, "issuer" | "issuerId" | "fetch">
): Promise<ProviderMetadata> => {
  const issuer = new URL(client.issuerId);
  const path =
    issuer.pathname === "/" ? "" : issuer.pathname.replace(/\/$/u, "");
  const get = async (url: URL): Promise<Response> =>
    await client.fetch(url, {
      headers: { accept: "application/json" },
      redirect: "error",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  let response = await get(
    new URL(`/.well-known/oauth-authorization-server${path}`, issuer.origin)
  );
  if (response.status === 404) {
    await response.body?.cancel();
    response = await get(
      new URL(
        `${client.issuerId.replace(/\/$/u, "")}/.well-known/openid-configuration`
      )
    );
  }
  if (!response.ok) {
    throw new Error(`OIDC discovery failed: HTTP ${response.status}`);
  }
  const metadata = Metadata.parse(await response.json());
  if (metadata.issuer !== client.issuerId) {
    throw new Error(
      `OIDC discovery: the server's issuer ${JSON.stringify(metadata.issuer)} is not JMAP_SERVER ${JSON.stringify(client.issuerId)}; set JMAP_SERVER to exactly the issuer`
    );
  }
  for (const endpoint of [
    metadata.authorization_endpoint,
    metadata.token_endpoint,
    ...(metadata.revocation_endpoint === undefined
      ? []
      : [metadata.revocation_endpoint]),
  ]) {
    if (!isAllowedEndpoint(new URL(endpoint), client.issuer)) {
      throw new Error("OIDC discovery: endpoints must use https");
    }
  }
  if (metadata.code_challenge_methods_supported?.includes("S256") === false) {
    throw new Error("OIDC provider does not support PKCE S256");
  }
  return metadata;
};

export interface AuthorizationRequest {
  readonly state: string;
  readonly codeChallenge: string;
}

export const authorizationUrl = (
  client: OidcClient,
  metadata: ProviderMetadata,
  request: AuthorizationRequest
): URL => {
  const url = new URL(metadata.authorization_endpoint);
  url.search = new URLSearchParams({
    client_id: client.clientId,
    code_challenge: request.codeChallenge,
    code_challenge_method: "S256",
    redirect_uri: client.redirectUri.href,
    response_type: "code",
    scope: client.scope ?? OAUTH_SCOPE,
    state: request.state,
  }).toString();
  return url;
};

const formEncode = (value: string): string =>
  encodeURIComponent(value).replaceAll("%20", "+");

// RFC 6749 §2.3.1: client_secret_basic, with both parts form-encoded first.
const basicAuth = (client: OidcClient): string =>
  `Basic ${btoa(`${formEncode(client.clientId)}:${formEncode(client.clientSecret)}`)}`;

const tokenRequest = async (
  client: OidcClient,
  metadata: ProviderMetadata,
  params: Record<string, string>
): Promise<TokenResult> => {
  let response: Response;
  try {
    response = await client.fetch(metadata.token_endpoint, {
      body: new URLSearchParams(params),
      headers: {
        accept: "application/json",
        authorization: basicAuth(client),
        "content-type": "application/x-www-form-urlencoded",
      },
      method: "POST",
      // Credentials must never follow a redirect.
      redirect: "error",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch {
    return { kind: "unavailable", ok: false };
  }
  const body: unknown = await response.json().catch(() => null);
  if (response.ok) {
    const parsed = TokenResponse.safeParse(body);
    if (!parsed.success) {
      return { kind: "unavailable", ok: false };
    }
    return {
      ok: true,
      tokens: {
        accessToken: parsed.data.access_token,
        expiresAt: tokenExpiresAt(client.now(), parsed.data.expires_in),
        refreshToken: parsed.data.refresh_token,
      },
    };
  }
  const error = ErrorResponse.safeParse(body);
  if (response.status >= 400 && response.status < 500 && error.success) {
    return { error: error.data.error, kind: "rejected", ok: false };
  }
  return { kind: "unavailable", ok: false };
};

export const exchangeCode = async (
  client: OidcClient,
  metadata: ProviderMetadata,
  code: string,
  codeVerifier: string
): Promise<TokenResult> =>
  await tokenRequest(client, metadata, {
    code,
    code_verifier: codeVerifier,
    grant_type: "authorization_code",
    redirect_uri: client.redirectUri.href,
  });

export const refresh = async (
  client: OidcClient,
  metadata: ProviderMetadata,
  refreshToken: string
): Promise<TokenResult> =>
  await tokenRequest(client, metadata, {
    grant_type: "refresh_token",
    refresh_token: refreshToken,
  });

/**
 * Revokes a refresh token (RFC 7009) when the provider has an endpoint for
 * it: the server ends the session (the bridge also deletes the stored mail
 * password). Best effort: logout must work even if this fails.
 */
export const revoke = async (
  client: OidcClient,
  metadata: ProviderMetadata,
  refreshToken: string
): Promise<void> => {
  if (metadata.revocation_endpoint === undefined) {
    return;
  }
  try {
    await client.fetch(metadata.revocation_endpoint, {
      body: new URLSearchParams({
        token: refreshToken,
        token_type_hint: "refresh_token",
      }),
      headers: {
        authorization: basicAuth(client),
        "content-type": "application/x-www-form-urlencoded",
      },
      method: "POST",
      // Credentials must never follow a redirect.
      redirect: "error",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch {
    // Nothing to do: the cookie is cleared anyway.
  }
};
