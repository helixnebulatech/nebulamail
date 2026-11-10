// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// getClient in the full BFF mode: the page knows the mode
// from its server render, so the proxy's session is asked for alongside
// the grant instead of one round trip after it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { JMAP_ORIGIN, session } from "../../e2e/fake-jmap";

/** The fake session with its URLs moved to the proxy, as the server does. */
const proxiedSession = (origin: string): unknown =>
  JSON.parse(
    JSON.stringify(session()).replaceAll(JMAP_ORIGIN, `${origin}/jmap-proxy`)
  );

const urlOf = (input: unknown): URL =>
  new URL(
    input instanceof Request ? input.url : String(input),
    globalThis.location.origin
  );

const stubServer = (grantSessionUrl: string) => {
  const { origin } = globalThis.location;
  const seen: string[] = [];
  const grant = Promise.withResolvers<null>();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown) => {
      const { pathname } = urlOf(input);
      seen.push(pathname);
      if (pathname === "/jmap-proxy/grant") {
        await grant.promise;
        return Response.json({
          expiresAt: Date.now() + 3_600_000,
          jmapProxy: true,
          sessionUrl: `${origin}${grantSessionUrl}`,
          verifiedSenderBadge: null,
        });
      }
      return Response.json(proxiedSession(origin));
    })
  );
  return {
    releaseGrant: () => {
      grant.resolve(null);
    },
    seen,
  };
};

/** A new page: the session layer and the client start empty. */
const load = async () => {
  const [{ setJmapProxyMode }, { getClient }] = await Promise.all([
    import("../lib/auth"),
    import("./client"),
  ]);
  return { getClient, setJmapProxyMode };
};

describe("getClient", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("asks for the proxy's session before the grant answers", async () => {
    const { getClient, setJmapProxyMode } = await load();
    const server = stubServer("/jmap-proxy/session");
    setJmapProxyMode(true);

    const client = getClient();
    await vi.waitFor(() => {
      expect(server.seen).toContain("/jmap-proxy/session");
    });
    expect(server.seen).toContain("/jmap-proxy/grant");
    server.releaseGrant();

    await expect(client).resolves.toBeDefined();
    // One session request: the early one was used.
    expect(
      server.seen.filter((path) => path === "/jmap-proxy/session")
    ).toHaveLength(1);
  });

  it("follows the grant when it names another session", async () => {
    const { getClient, setJmapProxyMode } = await load();
    const server = stubServer("/jmap-proxy/session?other=1");
    setJmapProxyMode(true);

    const client = getClient();
    server.releaseGrant();

    await expect(client).resolves.toBeDefined();
    expect(
      server.seen.filter((path) => path === "/jmap-proxy/session")
    ).toHaveLength(2);
  });

  it("waits for the grant when the mode isn't known", async () => {
    const { getClient } = await load();
    const server = stubServer("/jmap-proxy/session");

    const client = getClient();
    await Promise.resolve();
    // Without the server render's hint the grant goes first (to
    // /auth/token, which this fake answers like the proxy's session).
    expect(server.seen).not.toContain("/jmap-proxy/session");
    server.releaseGrant();
    await client.catch(() => null);
  });
});
