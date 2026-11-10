// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { afterEach, describe, expect, it, vi } from "vitest";

interface Created {
  readonly name: string;
  readonly options: Record<
    string,
    ((input: string) => string | null) | undefined
  >;
}

/** A stand-in for the browser's `trustedTypes` (jsdom has none). */
const fakeTrustedTypes = (): Created[] => {
  const created: Created[] = [];
  vi.stubGlobal("trustedTypes", {
    createPolicy: (
      name: string,
      options: Created["options"]
    ): Record<string, unknown> => {
      created.push({ name, options });
      return {
        createHTML: (input: string) => options.createHTML?.(input) ?? null,
      };
    },
  });
  return created;
};

describe("Trusted Types policies", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it("the default policy sanitizes and refuses scripts", async () => {
    const created = fakeTrustedTypes();
    const { installTrustedTypes } = await import("../src/trusted-types");
    installTrustedTypes();
    installTrustedTypes();
    const policy = created.find((item) => item.name === "default");
    expect(created.filter((item) => item.name === "default")).toHaveLength(1);
    expect(
      policy?.options.createHTML?.(
        '<p onclick="x()">Hi<script>x()</script></p>'
      )
    ).toBe("<p>Hi</p>");
    expect(policy?.options.createScript?.("alert(1)")).toBeNull();
    expect(
      policy?.options.createScriptURL?.("https://evil.example/x.js")
    ).toBeNull();
    expect(policy?.options.createScriptURL?.("/sw.js")).toBe("/sw.js");
  });

  it("can limit script URLs to the app's own files", async () => {
    const created = fakeTrustedTypes();
    const { installTrustedTypes } = await import("../src/trusted-types");
    installTrustedTypes({
      scriptUrlPaths: ["/nebulamail/assets/", "/nebulamail/sw.js"],
    });
    const allow = created.find((item) => item.name === "default")?.options
      .createScriptURL;
    expect(allow?.("/nebulamail/assets/worker-a1.js")).toBe(
      "/nebulamail/assets/worker-a1.js"
    );
    expect(allow?.("/nebulamail/sw.js")).toBe("/nebulamail/sw.js");
    for (const url of [
      "/jmap/download/a/b/x.js?accept=text/javascript",
      "/nebulamail/assets/../../jmap/download/x.js",
      "/nebulamail/assets/x.js?y",
      "/nebulamail/sw.js/x",
      "https://evil.example/nebulamail/assets/x.js",
    ]) {
      expect(allow?.(url)).toBeNull();
    }
  });

  it("parses into an inert document through its own policy", async () => {
    const created = fakeTrustedTypes();
    const { parseInertHtml } = await import("../src/trusted-types");
    const doc = parseInertHtml("<p>One <b>two</b></p>");
    expect(doc.body.querySelector("b")?.textContent).toBe("two");
    expect(created.map((item) => item.name)).toStrictEqual([
      "nebula-mail-parse",
    ]);
  });

  it("only srcdocs made by renderEmail go into an iframe", async () => {
    const created = fakeTrustedTypes();
    const { DEFAULT_PRIVACY, renderEmail, trustedSrcdoc } =
      await import("../src/index");
    const result = renderEmail({ html: "<p>Hello</p>" }, DEFAULT_PRIVACY, {});
    expect(trustedSrcdoc(result.frame)).toBe(result.srcdoc);
    const forged: unknown[] = [
      "<script>alert(1)</script>",
      {},
      { html: "<script>alert(1)</script>" },
      Object.create(result.frame),
    ];
    for (const value of forged) {
      // As untyped code could call it.
      expect(() => {
        Reflect.apply(trustedSrcdoc, undefined, [value]);
      }).toThrow("Not a srcdoc made by renderEmail");
    }
    // Nor with its constructor, which is reachable from any frame.
    expect(() => {
      Reflect.construct(result.frame.constructor, [
        Symbol("mint"),
        "<script>alert(1)</script>",
      ]);
    }).toThrow(TypeError);
    // The policy only accepts the HTML trustedSrcdoc is passing right now.
    const policy = created.find((item) => item.name === "nebula-mail-srcdoc");
    expect(policy?.options.createHTML?.(result.srcdoc)).toBeNull();
  });

  it("every frame stays valid in a big conversation", async () => {
    const { DEFAULT_PRIVACY, renderEmail, trustedSrcdoc } =
      await import("../src/index");
    // More than the 20 srcdocs that used to be remembered.
    const results = Array.from({ length: 50 }, (_, i) =>
      renderEmail({ html: `<p>Email ${i}</p>` }, DEFAULT_PRIVACY, {})
    );
    const [first] = results;
    const latest = results.at(-1);
    expect(first && trustedSrcdoc(first.frame)).toBe(first?.srcdoc);
    expect(latest && trustedSrcdoc(latest.frame)).toBe(latest?.srcdoc);
  }, 60_000);

  it("the same render gives the same frame (no iframe reload)", async () => {
    const { DEFAULT_PRIVACY, renderEmail } = await import("../src/index");
    const a = renderEmail({ html: "<p>Same</p>" }, DEFAULT_PRIVACY, {});
    const b = renderEmail({ html: "<p>Same</p>" }, DEFAULT_PRIVACY, {});
    const c = renderEmail({ html: "<p>Other</p>" }, DEFAULT_PRIVACY, {});
    expect(b.frame).toBe(a.frame);
    expect(c.frame).not.toBe(a.frame);
  });
});
