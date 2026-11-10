// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { OidcClient, ProviderMetadata } from "./oidc";
import { discover, refresh } from "./oidc";

const ISSUER = "https://mail.example.com";

const metadataOf = (issuer: string): ProviderMetadata => ({
  authorization_endpoint: `${ISSUER}/login`,
  code_challenge_methods_supported: ["S256"],
  issuer,
  token_endpoint: `${ISSUER}/auth/token`,
});

/** A fetch that answers every request with this JSON, and records URLs. */
const answering = (status: number, body: unknown) => {
  const urls: string[] = [];
  const fetchImpl: typeof fetch = async (input) => {
    urls.push(input instanceof Request ? input.url : input.toString());
    return await Promise.resolve(Response.json(body, { status }));
  };
  return { fetchImpl, urls };
};

const clientFor = (issuerId: string, fetchImpl: typeof fetch): OidcClient => ({
  clientId: "c",
  clientSecret: "s",
  fetch: fetchImpl,
  issuer: new URL(issuerId),
  issuerId,
  now: () => 1000,
  redirectUri: new URL("https://nebulamail.example.com/auth/callback"),
});

describe(discover, () => {
  it("accepts the issuer it was configured with, exactly", async () => {
    const { fetchImpl, urls } = answering(200, metadataOf(ISSUER));
    await expect(discover(clientFor(ISSUER, fetchImpl))).resolves.toMatchObject(
      { issuer: ISSUER }
    );
    // RFC 8414 §3 first.
    expect(urls).toStrictEqual([
      "https://mail.example.com/.well-known/oauth-authorization-server",
    ]);
  });

  it("falls back to OpenID Connect's document only on a 404", async () => {
    const urls: string[] = [];
    const fetchImpl: typeof fetch = async (input) => {
      const url = input instanceof Request ? input.url : input.toString();
      urls.push(url);
      return await Promise.resolve(
        url.endsWith("/oauth-authorization-server")
          ? new Response(null, { status: 404 })
          : Response.json(metadataOf(ISSUER))
      );
    };
    await expect(discover(clientFor(ISSUER, fetchImpl))).resolves.toMatchObject(
      { issuer: ISSUER }
    );
    expect(urls).toStrictEqual([
      "https://mail.example.com/.well-known/oauth-authorization-server",
      "https://mail.example.com/.well-known/openid-configuration",
    ]);
    // Another failure is not a reason to try elsewhere.
    const { fetchImpl: failing, urls: tried } = answering(500, {});
    await expect(discover(clientFor(ISSUER, failing))).rejects.toThrow(
      "HTTP 500"
    );
    expect(tried).toHaveLength(1);
  });

  it("puts the well-known part before the issuer's path (RFC 8414 §3.1)", async () => {
    const issuer = "https://example.com/tenant";
    const { fetchImpl, urls } = answering(200, metadataOf(issuer));
    await discover(clientFor(issuer, fetchImpl));
    expect(urls).toStrictEqual([
      "https://example.com/.well-known/oauth-authorization-server/tenant",
    ]);
  });

  it.each([
    // RFC 8414 §3.3 / OIDC Discovery §4.3: identical, no normalisation.
    [ISSUER, `${ISSUER}/`],
    [`${ISSUER}/`, ISSUER],
    [ISSUER, "https://MAIL.example.com"],
    [ISSUER, "https://mail.example.com:443"],
    [ISSUER, "https://evil.example"],
  ])("configured %s refuses issuer %s", async (configured, published) => {
    const { fetchImpl, urls } = answering(200, metadataOf(published));
    await expect(discover(clientFor(configured, fetchImpl))).rejects.toThrow(
      "is not JMAP_SERVER"
    );
    // The issuer's path (none, or just "/") goes after the well-known part.
    expect(urls).toStrictEqual([
      "https://mail.example.com/.well-known/oauth-authorization-server",
    ]);
  });
});

describe(refresh, () => {
  it("assumes 5 minutes without expires_in (RFC 6749 §5.1)", async () => {
    const { fetchImpl } = answering(200, {
      access_token: "a",
      refresh_token: "r",
      token_type: "Bearer",
    });
    await expect(
      refresh(clientFor(ISSUER, fetchImpl), metadataOf(ISSUER), "r0")
    ).resolves.toStrictEqual({
      ok: true,
      tokens: { accessToken: "a", expiresAt: 301_000, refreshToken: "r" },
    });
  });

  it("uses expires_in when given, and refuses a bad one", async () => {
    const good = answering(200, {
      access_token: "a",
      expires_in: 60,
      token_type: "Bearer",
    });
    await expect(
      refresh(clientFor(ISSUER, good.fetchImpl), metadataOf(ISSUER), "r0")
    ).resolves.toMatchObject({ tokens: { expiresAt: 61_000 } });
    const bad = answering(200, {
      access_token: "a",
      expires_in: -5,
      token_type: "Bearer",
    });
    await expect(
      refresh(clientFor(ISSUER, bad.fetchImpl), metadataOf(ISSUER), "r0")
    ).resolves.toStrictEqual({ kind: "unavailable", ok: false });
  });
});
