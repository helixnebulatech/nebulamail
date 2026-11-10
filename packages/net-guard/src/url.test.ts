// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { guardUrl, HTTPS_POLICY, parseTargetUrl } from "./index";
import type { Resolver, UrlPolicy } from "./index";
import { fail, later } from "./test-fixtures";

const PUBLIC = "93.184.215.14";
const resolver: Resolver = async () => await later({ A: [PUBLIC], AAAA: [] });

describe(parseTargetUrl, () => {
  it("normalizes and splits the URL", () => {
    const result = parseTargetUrl(
      "HTTPS://Images.EXAMPLE.com:443/a%20b.png?x=1#frag"
    );
    expect(result).toMatchObject({
      ok: true,
      target: {
        authority: "images.example.com",
        host: { kind: "name", name: "images.example.com" },
        hostname: "images.example.com",
        href: "https://images.example.com/a%20b.png?x=1",
        path: "/a%20b.png?x=1",
        port: 443,
        scheme: "https:",
      },
    });
  });

  it("converts IDN to punycode before anything else", () => {
    expect(parseTargetUrl("https://bücher.example/")).toMatchObject({
      target: { host: { kind: "name", name: "xn--bcher-kva.example" } },
    });
  });

  it("does not keep the caller's URL object", () => {
    const url = new URL("https://images.example.com/a.png");
    const result = parseTargetUrl(url);
    url.hostname = "127.0.0.1";
    expect(result).toMatchObject({
      target: { hostname: "images.example.com" },
    });
  });

  it("rejects unparseable input as scheme", () => {
    for (const input of [
      "",
      "not a url",
      "http://[::1",
      "//images.example.com/a.png",
    ]) {
      expect(parseTargetUrl(input)).toStrictEqual({
        ok: false,
        reason: "scheme",
      });
    }
  });

  it("rejects empty userinfo markers only when they carry data", () => {
    expect(parseTargetUrl("http://:pass@images.example.com/")).toStrictEqual({
      ok: false,
      reason: "userinfo",
    });
    expect(parseTargetUrl("http://user@images.example.com/")).toStrictEqual({
      ok: false,
      reason: "userinfo",
    });
  });

  it("validates bracketed IPv6 literals", () => {
    expect(parseTargetUrl("https://[2606:4700:4700::1111]/")).toMatchObject({
      ok: true,
      target: {
        host: { family: 6, kind: "ip" },
        hostname: "2606:4700:4700::1111",
      },
    });
    expect(parseTargetUrl("https://[::ffff:169.254.169.254]/")).toStrictEqual({
      ok: false,
      reason: "private-ip",
    });
  });

  it("supports other port lists (bridge/BFF)", () => {
    const bridge: UrlPolicy = {
      ports: [143, 993, 465, 587, 4190, 443],
      schemes: ["https:"],
    };
    expect(
      parseTargetUrl("https://mail.example.com:993/", bridge)
    ).toMatchObject({ ok: true });
    expect(parseTargetUrl("https://mail.example.com/", bridge)).toMatchObject({
      ok: true,
    });
    expect(
      parseTargetUrl("https://mail.example.com:8443/", bridge)
    ).toStrictEqual({ ok: false, reason: "port" });
    expect(
      parseTargetUrl("http://mail.example.com:993/", bridge)
    ).toStrictEqual({ ok: false, reason: "scheme" });
  });

  it("HTTPS_POLICY refuses plain http", () => {
    expect(
      parseTargetUrl("http://jmap.example.com/", HTTPS_POLICY)
    ).toStrictEqual({ ok: false, reason: "scheme" });
  });
});

describe(guardUrl, () => {
  it("returns validated addresses, IPv4 first and deduplicated", async () => {
    const result = await guardUrl(
      "https://images.example.com/",
      async () =>
        await later({ A: [PUBLIC, PUBLIC], AAAA: ["2606:4700:4700::1111"] })
    );
    expect(result).toMatchObject({
      ok: true,
      target: { addresses: [PUBLIC, "2606:4700:4700::1111"] },
    });
  });

  it("does not resolve IP literals", async () => {
    const calls: string[] = [];
    const result = await guardUrl(`http://${PUBLIC}/`, async (hostname) => {
      calls.push(hostname);
      return await fail("unused");
    });
    expect(result).toMatchObject({ ok: true, target: { addresses: [PUBLIC] } });
    expect(calls).toStrictEqual([]);
  });

  it("treats resolver errors and garbage as dns", async () => {
    await expect(
      guardUrl("https://a.example/", async () => await fail("ENOTFOUND"))
    ).resolves.toStrictEqual({ ok: false, reason: "dns" });
    await expect(
      guardUrl(
        "https://a.example/",
        async () => await later({ A: ["0x7f.1"], AAAA: [] })
      )
    ).resolves.toStrictEqual({ ok: false, reason: "dns" });
    await expect(
      guardUrl(
        "https://a.example/",
        async () => await later({ A: [], AAAA: ["fe80::1%eth0"] })
      )
    ).resolves.toStrictEqual({ ok: false, reason: "dns" });
  });

  it("uses the injected resolver", async () => {
    await expect(
      guardUrl("https://a.example/", resolver)
    ).resolves.toMatchObject({ ok: true });
  });
});
