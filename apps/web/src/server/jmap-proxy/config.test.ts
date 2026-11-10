// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  JmapProxyConfigError,
  makeUpstream,
  parseInternalUrl,
  parseJmapProxy,
  toUpstream,
  upstreamFetch,
} from "./config";

/** An IPv4 address from its four numbers. */
const v4 = (...parts: readonly number[]): string => parts.join(".");

describe(parseJmapProxy, () => {
  it("is off unless it says on", () => {
    expect(parseJmapProxy(undefined)).toBeFalsy();
    expect(parseJmapProxy("")).toBeFalsy();
    expect(parseJmapProxy("off")).toBeFalsy();
    expect(parseJmapProxy("on")).toBeTruthy();
  });

  it("refuses anything else instead of guessing", () => {
    for (const value of ["true", "1", "ON", "yes", " on"]) {
      expect(() => parseJmapProxy(value)).toThrow(JmapProxyConfigError);
    }
  });
});

describe(parseInternalUrl, () => {
  it("is optional", () => {
    expect(parseInternalUrl(undefined)).toBeUndefined();
    expect(parseInternalUrl("")).toBeUndefined();
  });

  it("takes https anywhere and http only on internal hosts", () => {
    for (const [value, origin] of [
      ["http://stalwart:8080", "http://stalwart:8080"],
      ["http://stalwart:8080/", "http://stalwart:8080"],
      ["http://localhost:8080", "http://localhost:8080"],
      [`http://${v4(127, 0, 0, 1)}:8080`, `http://${v4(127, 0, 0, 1)}:8080`],
      ["http://[::1]:8080", "http://[::1]:8080"],
      [`http://${v4(10, 1, 2, 3)}`, `http://${v4(10, 1, 2, 3)}`],
      [
        `http://${v4(192, 168, 1, 20)}:8080`,
        `http://${v4(192, 168, 1, 20)}:8080`,
      ],
      [`http://${v4(100, 64, 0, 7)}`, `http://${v4(100, 64, 0, 7)}`],
      ["http://[fd00::5]:8080", "http://[fd00::5]:8080"],
      ["http://mail.internal", "http://mail.internal"],
      ["http://mail.lan:8080", "http://mail.lan:8080"],
      ["https://mail.example.com", "https://mail.example.com"],
      [`https://${v4(8, 8, 8, 8)}`, `https://${v4(8, 8, 8, 8)}`],
    ] as const satisfies readonly (readonly [string, string])[]) {
      expect(parseInternalUrl(value)?.origin).toBe(origin);
    }
  });

  it("refuses plain http to public or link-local hosts", () => {
    for (const value of [
      "http://mail.example.com",
      `http://${v4(8, 8, 4, 4)}`,
      "http://[2001:4860:4860::8888]",
      // Cloud metadata.
      `http://${v4(169, 254, 169, 254)}`,
      "http://[fe80::1]",
      `http://${v4(0, 0, 0, 0)}`,
    ]) {
      expect(() => parseInternalUrl(value)).toThrow(JmapProxyConfigError);
    }
  });

  it("never takes a cloud metadata service, not even over https", () => {
    for (const value of [
      `https://${v4(169, 254, 169, 254)}`,
      "https://[[IP]]",
      "http://metadata.google.internal",
      "https://metadata.google.internal",
      "http://METADATA.google.internal.",
      "http://metadata",
      "http://instance-data.ec2.internal",
    ]) {
      expect(() => parseInternalUrl(value)).toThrow(JmapProxyConfigError);
    }
    // Other internal names still work.
    expect(parseInternalUrl("http://stalwart.internal:8080")?.origin).toBe(
      "http://stalwart.internal:8080"
    );
  });

  it("refuses credentials, paths, queries, fragments and other schemes", () => {
    for (const value of [
      "http://user:pass@stalwart:8080",
      "http://user@stalwart:8080",
      "http://stalwart:8080/jmap",
      "http://stalwart:8080/?",
      "http://stalwart:8080?x=1",
      "http://stalwart:8080/#",
      "http://stalwart:8080#top",
      "ftp://stalwart",
      "file:///etc/passwd",
      "stalwart:8080",
      "not a url",
    ]) {
      expect(() => parseInternalUrl(value)).toThrow(JmapProxyConfigError);
    }
  });
});

describe(toUpstream, () => {
  const upstream = makeUpstream(
    new URL("https://mail.example.com"),
    new URL("http://stalwart:8080")
  );

  it("moves URLs on the public origin to the internal one", () => {
    expect(
      toUpstream(new URL("https://mail.example.com/jmap/?a=1"), upstream).href
    ).toBe("http://stalwart:8080/jmap/?a=1");
  });

  it("leaves every other URL alone", () => {
    for (const url of [
      "https://mail.example.com.evil.test/jmap/",
      "https://img.example.com/x",
      "http://mail.example.com/jmap/",
    ]) {
      expect(toUpstream(new URL(url), upstream).href).toBe(url);
    }
  });

  it("without an internal URL, the public one is the upstream", () => {
    const same = makeUpstream(new URL("https://mail.example.com"), undefined);
    expect(same).toStrictEqual({
      origin: "https://mail.example.com",
      publicOrigin: "https://mail.example.com",
    });
  });
});

describe(upstreamFetch, () => {
  it("is the same fetch without an internal URL", () => {
    const base: typeof fetch = Object.assign(
      async () => await Promise.resolve(new Response()),
      { preconnect: () => null }
    );
    expect(upstreamFetch(base, undefined)).toBe(base);
    expect(
      upstreamFetch(base, makeUpstream(new URL("https://m.example"), undefined))
    ).toBe(base);
  });

  it("sends the server's own calls to the internal origin", async () => {
    const seen: string[] = [];
    const base: typeof fetch = Object.assign(
      async (input: Parameters<typeof fetch>[0]) => {
        seen.push(input instanceof Request ? input.url : input.toString());
        return await Promise.resolve(new Response());
      },
      { preconnect: () => null }
    );
    const wrapped = upstreamFetch(
      base,
      makeUpstream(
        new URL("https://mail.example.com"),
        new URL("http://stalwart:8080")
      )
    );
    await wrapped("https://mail.example.com/auth/token");
    await wrapped(new URL("https://other.example.com/x"));
    expect(seen).toStrictEqual([
      "http://stalwart:8080/auth/token",
      "https://other.example.com/x",
    ]);
  });
});
