// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { guardUrl, isNonPublicName } from "./index";
import type { Resolver } from "./index";
import { later } from "./test-fixtures";

describe(isNonPublicName, () => {
  it.each([
    "localhost",
    "localhost.",
    "LOCALHOST",
    "x.localhost.",
    "intranet",
    "intranet.",
    "",
    ".",
    "grafana.internal",
    "metadata.google.internal",
    "printer.local",
    "foo.local.",
    "Foo.LOCAL..",
    "abc.onion",
    "x.alt",
    "nas.home.arpa",
    "1.0.0.127.in-addr.arpa",
    "a.invalid",
    "fileserver.corp",
    "router.home",
    "box.mail",
    "nas.lan",
    "wiki.intranet",
    "pc.localdomain",
  ])("refuses %j", (name) => {
    expect(isNonPublicName(name)).toBeTruthy();
  });

  it.each([
    "images.example.com",
    "images.example.com.",
    "local.example.com",
    "localhost.example.com",
    "internal.example.org",
    "onion.example",
    "img.test",
    "mail.example",
    "xn--bcher-kva.example",
    "a.b.c.d.co.uk",
  ])("accepts %j", (name) => {
    expect(isNonPublicName(name)).toBeFalsy();
  });
});

describe("name rule in the guard", () => {
  it("never asks the resolver about a non-public name", async () => {
    const asked: string[] = [];
    const resolver: Resolver = async (name) => {
      asked.push(name);
      return await later({ A: ["93.184.215.14"], AAAA: [] });
    };
    await expect(
      guardUrl("https://grafana.internal/", resolver)
    ).resolves.toStrictEqual({ ok: false, reason: "dns" });
    await expect(
      guardUrl("http://intranet./", resolver)
    ).resolves.toStrictEqual({ ok: false, reason: "dns" });
    expect(asked).toStrictEqual([]);
    await expect(
      guardUrl("https://images.example.com/", resolver)
    ).resolves.toMatchObject({ ok: true });
    expect(asked).toStrictEqual(["images.example.com"]);
  });
});
