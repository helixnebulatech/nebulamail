// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  authorizationUrl,
  fetchMetadata,
  onOrigin,
  parseCallback,
  pkceChallenge,
  safeReturnTo,
  revokeRequest,
  tokenRequest,
} from "./oauth";
import type { ProviderMetadata } from "./oauth";

const METADATA: ProviderMetadata = {
  authorization_endpoint: "https://mail.example.com/authorize/code",
  authorization_response_iss_parameter_supported: true,
  code_challenge_methods_supported: ["S256"],
  issuer: "https://mail.example.com",
  token_endpoint: "https://mail.example.com/auth/token",
};

/** A `fetch` that always answers `body` as JSON with `status`. */
const answering =
  (status: number, body: unknown): typeof fetch =>
  async () =>
    await Promise.resolve(Response.json(body, { status }));

const search = (values: Record<string, string>): URLSearchParams =>
  new URLSearchParams(values);

describe(pkceChallenge, () => {
  it("matches the RFC 7636 example", async () => {
    await expect(
      pkceChallenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
    ).resolves.toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  });
});

describe(onOrigin, () => {
  it("keeps the path and moves it to the page's origin", () => {
    expect(
      onOrigin(
        "https://mail.example.com/authorize/code?x=1",
        "https://nebulamail.example.com"
      ).href
    ).toBe("https://nebulamail.example.com/authorize/code?x=1");
  });
});

describe(authorizationUrl, () => {
  it("asks for code + PKCE S256 on our origin", () => {
    const url = authorizationUrl(METADATA, "https://nebulamail.example.com", {
      clientId: "nebulamail",
      codeChallenge: "challenge",
      redirectUri: "https://nebulamail.example.com/nebulamail/login",
      state: "s1",
    });
    expect(url.origin).toBe("https://nebulamail.example.com");
    expect(url.pathname).toBe("/authorize/code");
    expect(Object.fromEntries(url.searchParams)).toStrictEqual({
      client_id: "nebulamail",
      code_challenge: "challenge",
      code_challenge_method: "S256",
      redirect_uri: "https://nebulamail.example.com/nebulamail/login",
      response_type: "code",
      scope:
        "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts urn:ietf:params:oauth:scope:calendars urn:ietf:params:oauth:scope:files",
      state: "s1",
    });
  });
});

describe(parseCallback, () => {
  it("accepts our state, the right issuer and a code", () => {
    expect(
      parseCallback(
        search({ code: "c", iss: METADATA.issuer, state: "s1" }),
        "s1",
        METADATA
      )
    ).toStrictEqual({ code: "c", ok: true });
  });

  it("rejects another state", () => {
    expect(
      parseCallback(
        search({ code: "c", iss: METADATA.issuer, state: "s2" }),
        "s1",
        METADATA
      )
    ).toStrictEqual({ error: "invalid", ok: false });
  });

  it("rejects another issuer, or none when the server sends one", () => {
    expect(
      parseCallback(
        search({ code: "c", iss: "https://evil.example", state: "s1" }),
        "s1",
        METADATA
      )
    ).toStrictEqual({ error: "invalid", ok: false });
    expect(
      parseCallback(search({ code: "c", state: "s1" }), "s1", METADATA)
    ).toStrictEqual({ error: "invalid", ok: false });
  });

  it("tells a refusal apart", () => {
    expect(
      parseCallback(
        search({ error: "access_denied", iss: METADATA.issuer, state: "s1" }),
        "s1",
        METADATA
      )
    ).toStrictEqual({ error: "denied", ok: false });
  });
});

describe(safeReturnTo, () => {
  it.each([
    [undefined, "/"],
    ["/mail/inbox?x=1#y", "/mail/inbox?x=1#y"],
    ["//evil.example/", "/"],
    ["/.//evil.example/", "/"],
    ["/..//evil.example/", "/"],
    ["/%2e//evil.example/", "/"],
    ["/\\evil.example", "/"],
    ["https://evil.example/", "/"],
    ["mail", "/"],
  ])("%s → %s", (value, expected) => {
    expect(safeReturnTo(value)).toBe(expected);
  });
});

describe(fetchMetadata, () => {
  it("refuses a server without PKCE S256", async () => {
    const fetchImpl = answering(200, {
      ...METADATA,
      code_challenge_methods_supported: ["plain"],
    });
    await expect(
      fetchMetadata("https://nebulamail.example.com", fetchImpl)
    ).rejects.toThrow("S256");
  });
});

describe(tokenRequest, () => {
  const endpoint = new URL("https://nebulamail.example.com/auth/token");

  it("reads the tokens", async () => {
    const fetchImpl = answering(200, {
      access_token: "a",
      expires_in: 60,
      refresh_token: "r",
      token_type: "Bearer",
    });
    await expect(
      tokenRequest(endpoint, {}, () => 1000, fetchImpl)
    ).resolves.toStrictEqual({
      ok: true,
      tokens: { accessToken: "a", expiresAt: 61_000, refreshToken: "r" },
    });
  });

  it("assumes 5 minutes without expires_in (RFC 6749 §5.1: RECOMMENDED)", async () => {
    const fetchImpl = answering(200, {
      access_token: "a",
      token_type: "Bearer",
    });
    await expect(
      tokenRequest(endpoint, {}, () => 1000, fetchImpl)
    ).resolves.toStrictEqual({
      ok: true,
      tokens: { accessToken: "a", expiresAt: 301_000, refreshToken: undefined },
    });
  });

  it("tells a rejected grant from an outage", async () => {
    const rejected = answering(400, { error: "invalid_grant" });
    const outage = answering(503, {});
    await expect(
      tokenRequest(endpoint, {}, Date.now, rejected)
    ).resolves.toStrictEqual({
      error: "invalid_grant",
      kind: "rejected",
      ok: false,
    });
    await expect(
      tokenRequest(endpoint, {}, Date.now, outage)
    ).resolves.toStrictEqual({
      kind: "unavailable",
      ok: false,
    });
  });
});

const offline: typeof fetch = async () =>
  await Promise.reject(new TypeError("offline"));

describe(revokeRequest, () => {
  const endpoint = new URL("https://nebulamail.example.com/auth/revoke");

  it("sends the refresh token as a public client", async () => {
    const calls: { url: string; init: RequestInit | undefined }[] = [];
    const fetchImpl: typeof fetch = async (input, init) => {
      calls.push({
        init,
        url: input instanceof Request ? input.url : input.toString(),
      });
      return await Promise.resolve(new Response(null, { status: 200 }));
    };
    await revokeRequest(
      endpoint,
      { clientId: "nebulamail", refreshToken: "r1" },
      fetchImpl
    );
    const [call] = calls;
    expect(calls).toHaveLength(1);
    expect(call).toMatchObject({
      init: { credentials: "omit", method: "POST", redirect: "error" },
      url: endpoint.href,
    });
    const body = call?.init?.body;
    expect(body).toBeInstanceOf(URLSearchParams);
    expect(
      Object.fromEntries(body instanceof URLSearchParams ? body : [])
    ).toStrictEqual({
      client_id: "nebulamail",
      token: "r1",
      token_type_hint: "refresh_token",
    });
  });

  it("never fails: signing out goes on", async () => {
    await expect(
      revokeRequest(endpoint, { clientId: "c", refreshToken: "r" }, offline)
    ).resolves.toBeUndefined();
  });
});
