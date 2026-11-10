// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `sameOriginCookies`: a BFF proxy on the page's own origin holds the token
// and the browser only sends its cookies.

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId, BlobId } from "./ids";
import {
  fakeFetch,
  methodResponses,
  rejection,
  stalwartSession,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const APP = "https://app.example.com";
const PROXY_SESSION = `${APP}/jmap-proxy/session`;
const accountId = AccountId.parse("d");

const proxySession = (overrides: Record<string, unknown> = {}) =>
  stalwartSession({
    apiUrl: `${APP}/jmap-proxy/api`,
    downloadUrl: `${APP}/jmap-proxy/download/{accountId}/{blobId}/{name}?type={type}`,
    eventSourceUrl: `${APP}/jmap-proxy/eventsource?types={types}&closeafter={closeafter}&ping={ping}`,
    uploadUrl: `${APP}/jmap-proxy/upload/{accountId}`,
    ...overrides,
  });

const echo = methodResponses([["Core/echo", {}, "c0"]]);

const setup = async (
  api: Handler,
  session: Record<string, unknown> = proxySession()
) => {
  const fake = fakeFetch(async (call, index) =>
    call.url === PROXY_SESSION ? Response.json(session) : await api(call, index)
  );
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    headers: { "X-Nebula-Proxy": "1" },
    sameOriginCookies: true,
    sessionUrl: PROXY_SESSION,
  });
  return { calls: fake.calls, client };
};

describe("same-origin cookies (BFF proxy)", () => {
  it("sends cookies and the extra header, never Authorization", async () => {
    const { calls, client } = await setup(() => echo.clone());
    await client.request((b) => b.call("Core/echo", {}));
    await client.downloadBlob({ accountId, blobId: BlobId.parse("Gb") });
    expect(calls.length).toBeGreaterThanOrEqual(3);
    for (const call of calls) {
      expect(call.authorization).toBeUndefined();
      expect(call.init.credentials).toBe("same-origin");
      expect(new Headers(call.init.headers).get("x-nebula-proxy")).toBe("1");
      expect(call.init.redirect).not.toBe("follow");
      expect(new URL(call.url).origin).toBe(APP);
    }
  });

  it("refuses a session that points somewhere else", async () => {
    const error = await rejection(
      setup(
        () => echo.clone(),
        proxySession({ apiUrl: "https://evil.example/jmap" })
      )
    );
    expect(error.kind).toBe("crossOrigin");
  });

  it("can't be combined with other origins", async () => {
    const fake = fakeFetch(() => Response.json(proxySession()));
    const aliases = await rejection(
      JmapClient.connect({
        fetch: fake.fetch,
        sameOriginCookies: true,
        serverAliases: ["https://mail.example.com"],
        sessionUrl: PROXY_SESSION,
      })
    );
    expect(aliases.kind).toBe("invalidUrl");
    const crossOrigin = await rejection(
      JmapClient.connect({
        allowCrossOriginUrls: true,
        fetch: fake.fetch,
        sameOriginCookies: true,
        sessionUrl: PROXY_SESSION,
      })
    );
    expect(crossOrigin.kind).toBe("invalidUrl");
    expect(fake.calls).toHaveLength(0);
  });

  it("tries once more after a 401, then reports unauthorized", async () => {
    const { calls, client } = await setup(
      () => new Response("{}", { status: 401 })
    );
    const error = await rejection(
      client.request((b) => b.call("Core/echo", {}))
    );
    expect(error.kind).toBe("unauthorized");
    // Session, then the API call twice.
    expect(calls).toHaveLength(3);
  });

  it("a 401 then success goes through", async () => {
    const answers = [new Response("{}", { status: 401 }), echo.clone()];
    const { client } = await setup(() => answers.shift() ?? echo.clone());
    await expect(
      client.request((b) => b.call("Core/echo", {}))
    ).resolves.toBeDefined();
  });
});

describe("bearer tokens (the default) are unchanged", () => {
  it("still omit cookies and send the extra header with the token", async () => {
    const fake = fakeFetch((call) =>
      call.url === "https://mail.example.com/.well-known/jmap"
        ? Response.json(stalwartSession())
        : echo.clone()
    );
    const auth = tokens();
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      getAccessToken: auth.getAccessToken,
      headers: { "X-Extra": "yes" },
      sessionUrl: "https://mail.example.com/.well-known/jmap",
    });
    await client.request((b) => b.call("Core/echo", {}));
    for (const call of fake.calls) {
      expect(call.init.credentials).toBe("omit");
      expect(call.authorization).toBe("Bearer old-token");
      expect(new Headers(call.init.headers).get("x-extra")).toBe("yes");
    }
  });
});
