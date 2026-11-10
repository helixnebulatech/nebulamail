// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { guardUrl } from "../index";
import { createDohResolver } from "./index";
import type { WebFetch } from "./index";
import { DOH, fakeNet } from "./testing";

const answer =
  (body: unknown, init: ResponseInit = {}): WebFetch =>
  async () => {
    await Promise.resolve();
    return Response.json(body, init);
  };

describe(createDohResolver, () => {
  it("asks for A and AAAA as dns-json and skips CNAMEs", async () => {
    const seen: { url: string; init: RequestInit }[] = [];
    const resolve = createDohResolver({
      endpoint: DOH,
      fetch: async (url, init) => {
        seen.push({ init, url });
        const type = Number(new URL(url).searchParams.get("type"));
        await Promise.resolve();
        return Response.json({
          Answer: [
            { TTL: 60, data: "cdn.example.net.", name: "a", type: 5 },
            type === 1
              ? { TTL: 60, data: "93.184.215.14", name: "b", type: 1 }
              : { TTL: 60, data: "2606:2800::1", name: "b", type: 28 },
          ],
          Status: 0,
        });
      },
    });
    await expect(resolve("images.example.com.")).resolves.toStrictEqual({
      A: ["93.184.215.14"],
      AAAA: ["2606:2800::1"],
    });
    expect(seen.map((s) => s.url)).toStrictEqual([
      `${DOH}?name=images.example.com&type=1`,
      `${DOH}?name=images.example.com&type=28`,
    ]);
    expect(seen[0]?.init).toMatchObject({
      headers: { accept: "application/dns-json" },
      redirect: "manual",
    });
  });

  it("treats NXDOMAIN as an empty answer, which the guard blocks", async () => {
    const resolve = createDohResolver({
      endpoint: DOH,
      fetch: answer({ Status: 3 }),
    });
    await expect(resolve("nope.example")).resolves.toStrictEqual({
      A: [],
      AAAA: [],
    });
    await expect(
      guardUrl("https://nope.example/", resolve)
    ).resolves.toStrictEqual({
      ok: false,
      reason: "dns",
    });
  });

  it.each([
    ["SERVFAIL", answer({ Status: 2 })],
    ["HTTP error", answer({ Status: 0 }, { status: 500 })],
    [
      "redirect (not followed)",
      answer(
        { Status: 0 },
        { headers: { location: "https://evil.example/" }, status: 302 }
      ),
    ],
    ["no Status", answer({ Answer: [] })],
    ["bad Answer", answer({ Answer: "x", Status: 0 })],
    ["bad record", answer({ Answer: [{ data: 1, type: 1 }], Status: 0 })],
    [
      "not JSON",
      (async () => {
        await Promise.resolve();
        return new Response("<html>");
      }) satisfies WebFetch,
    ],
    [
      "too big",
      (async () => {
        await Promise.resolve();
        return new Response("x".repeat(70 * 1024));
      }) satisfies WebFetch,
    ],
    [
      "network",
      (async () => {
        await Promise.resolve();
        throw new TypeError("fetch failed");
      }) satisfies WebFetch,
    ],
  ])("throws on %s (the guard answers dns)", async (_name, fetch) => {
    const resolve = createDohResolver({ endpoint: DOH, fetch });
    await expect(resolve("images.example.com")).rejects.toThrow(Error);
    await expect(
      guardUrl("https://images.example.com/", resolve)
    ).resolves.toStrictEqual({
      ok: false,
      reason: "dns",
    });
  });

  it("caches answers for min(TTL, maxCacheMs)", async () => {
    let now = 0;
    const net = fakeNet(() => ({ A: ["93.184.215.14"] }));
    const resolve = createDohResolver({
      endpoint: DOH,
      fetch: net.fetch,
      maxCacheMs: 1000,
      now: () => now,
    });
    await resolve("images.example.com");
    await resolve("images.example.com");
    expect(net.queries).toHaveLength(2);
    now = 1001;
    await resolve("images.example.com");
    expect(net.queries).toHaveLength(4);
  });

  it("does not cache empty answers", async () => {
    const net = fakeNet(() => {});
    const resolve = createDohResolver({ endpoint: DOH, fetch: net.fetch });
    await resolve("nope.example");
    await resolve("nope.example");
    expect(net.queries).toHaveLength(4);
  });

  it.each(["http://doh.test/dns-query", "https://u:p@doh.test/", "nope"])(
    "refuses the endpoint %s",
    (endpoint) => {
      expect(() => createDohResolver({ endpoint })).toThrow(TypeError);
    }
  );
});
