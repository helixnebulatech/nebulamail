// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { AdminImagePolicy } from "./image-policy";
import {
  DEFAULT_ADMIN_IMAGE_POLICY,
  IMAGE_LOAD_MODES,
  decideRemoteImages,
  isTrustedSender,
  resolveImagePolicy,
  withTrustedSender,
} from "./image-policy";

// An admin who allows direct too (REMOTE_IMAGES_ALLOW_DIRECT=true): both
// routes to choose from. The defaults alone don't allow it with a proxy.
const admin = (changes: Partial<AdminImagePolicy> = {}): AdminImagePolicy => ({
  ...DEFAULT_ADMIN_IMAGE_POLICY,
  direct: true,
  ...changes,
});

describe(resolveImagePolicy, () => {
  it("offers only the proxy with the server's defaults", () => {
    const policy = resolveImagePolicy(DEFAULT_ADMIN_IMAGE_POLICY, {});
    expect(policy).toMatchObject({ route: "proxy", routes: ["proxy"] });
  });

  it("asks, through the proxy, by default", () => {
    expect(resolveImagePolicy(admin(), {})).toStrictEqual({
      forced: false,
      load: "ask",
      loadModes: IMAGE_LOAD_MODES,
      route: "proxy",
      routeChosen: false,
      routes: ["proxy", "direct"],
    });
  });

  it("follows the admin's defaults until the user chooses", () => {
    const policy = admin({ defaultLoad: "always", defaultRoute: "direct" });

    expect(resolveImagePolicy(policy, {})).toMatchObject({
      load: "always",
      route: "direct",
      routeChosen: true,
    });
    expect(
      resolveImagePolicy(policy, { load: "never", route: "proxy" })
    ).toMatchObject({ load: "never", route: "proxy", routeChosen: true });
  });

  it("never offers direct when the admin forbids it", () => {
    const policy = resolveImagePolicy(admin({ direct: false }), {
      route: "direct",
    });

    expect(policy.routes).toStrictEqual(["proxy"]);
    expect(policy.route).toBe("proxy");
    expect(policy.routeChosen).toBeFalsy();
  });

  it("doesn't offer the proxy when there is none", () => {
    const policy = resolveImagePolicy(admin({ proxy: false }), {});

    expect(policy.routes).toStrictEqual(["direct"]);
    expect(policy.route).toBe("direct");
    // Only because there is nothing else: not a choice.
    expect(policy.routeChosen).toBeFalsy();
  });

  it("keeps the user's 'proxy' without a proxy as 'ask first'", () => {
    const policy = resolveImagePolicy(admin({ proxy: false }), {
      route: "proxy",
    });

    expect(policy).toMatchObject({ route: "direct", routeChosen: false });
  });

  it("forces never, whatever the user chose", () => {
    expect(
      resolveImagePolicy(admin({ defaultLoad: "never", forceNever: true }), {
        load: "always",
        route: "direct",
      })
    ).toStrictEqual({
      forced: true,
      load: "never",
      loadModes: ["never"],
      route: null,
      routeChosen: false,
      routes: [],
    });
  });

  it("has no way to load without a proxy and with direct forbidden", () => {
    const policy = resolveImagePolicy(admin({ direct: false, proxy: false }), {
      load: "always",
    });

    expect(policy).toMatchObject({ forced: false, load: "never", route: null });
  });
});

describe(decideRemoteImages, () => {
  const trusted = { senderTrusted: true };
  const stranger = { senderTrusted: false };

  it("never loads anything with 'never', even from a trusted sender", () => {
    const policy = resolveImagePolicy(admin(), { load: "never" });

    expect(decideRemoteImages(policy, trusted)).toBe("never");
    expect(decideRemoteImages(policy, stranger)).toBe("never");
  });

  it("asks with 'ask', even for a trusted sender", () => {
    const policy = resolveImagePolicy(admin(), { load: "ask" });

    expect(decideRemoteImages(policy, trusted)).toBe("ask");
  });

  it("loads at once only from trusted senders with 'trusted-senders'", () => {
    const policy = resolveImagePolicy(admin(), { load: "trusted-senders" });

    expect(decideRemoteImages(policy, trusted)).toBe("auto");
    expect(decideRemoteImages(policy, stranger)).toBe("ask");
  });

  it("loads at once with 'always', by the chosen route", () => {
    expect(
      decideRemoteImages(
        resolveImagePolicy(admin(), { load: "always" }),
        stranger
      )
    ).toBe("auto");
    expect(
      decideRemoteImages(
        resolveImagePolicy(admin(), { load: "always", route: "direct" }),
        stranger
      )
    ).toBe("auto");
  });

  it("asks first when direct is only there for lack of a proxy", () => {
    const noProxy = admin({ proxy: false });

    expect(
      decideRemoteImages(
        resolveImagePolicy(noProxy, { load: "always" }),
        trusted
      )
    ).toBe("ask");
    expect(
      decideRemoteImages(
        resolveImagePolicy(noProxy, { load: "always", route: "direct" }),
        stranger
      )
    ).toBe("auto");
  });

  it("never loads when forced or without a route", () => {
    for (const changes of [
      { forceNever: true },
      { direct: false, proxy: false },
    ]) {
      const policy = resolveImagePolicy(admin(changes), { load: "always" });
      expect(decideRemoteImages(policy, trusted)).toBe("never");
    }
  });
});

/** An address (written this way so no tool rewrites it). */
const address = (user: string, host = "example.com"): string =>
  [user, host].join(String.fromCodePoint(64));

describe(isTrustedSender, () => {
  const facts = {
    from: [address("Ana", "Example.com")],
    outsideJunk: true,
    verdict: null,
  } as const;
  const lists = { contacts: [], list: [address("ana")] };

  it("trusts the list and the contacts, without caring about case", () => {
    expect(isTrustedSender(facts, lists)).toBeTruthy();
    expect(
      isTrustedSender(facts, { contacts: [address("ana")], list: [] })
    ).toBeTruthy();
    expect(isTrustedSender(facts, { contacts: [], list: [] })).toBeFalsy();
  });

  it("doesn't trust a failed sender check, Junk or several senders", () => {
    expect(isTrustedSender({ ...facts, verdict: "fail" }, lists)).toBeFalsy();
    expect(
      isTrustedSender({ ...facts, outsideJunk: false }, lists)
    ).toBeFalsy();
    expect(
      isTrustedSender(
        { ...facts, from: [address("ana"), address("bob")] },
        lists
      )
    ).toBeFalsy();
    expect(isTrustedSender({ ...facts, from: [] }, lists)).toBeFalsy();
    expect(isTrustedSender({ ...facts, verdict: "pass" }, lists)).toBeTruthy();
  });
});

describe(withTrustedSender, () => {
  it("adds once, in lower case, and removes", () => {
    const list = withTrustedSender(
      [address("bob")],
      ` ${address("Ana", "Example.com")} `,
      true
    );

    expect(list).toStrictEqual([address("bob"), address("ana")]);
    expect(withTrustedSender(list, address("ana"), true)).toStrictEqual(list);
    expect(withTrustedSender(list, address("ANA"), false)).toStrictEqual([
      address("bob"),
    ]);
  });
});
