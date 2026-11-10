// @vitest-environment node
// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs

// Runs without a DOM on purpose: the trackers module is shared with the BFF
// and the image proxy.

import { execFileSync } from "node:child_process";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  LINEAR_EQUIVALENTS,
  MAX_TRACKER_INPUT,
  MTB_PATTERN_COUNT,
  MTB_RULES,
  hostMatches,
  isSpyPixel,
  isTrackerUrl,
  parseCssPx,
  parseHtmlDimension,
  spyPixelReason,
} from "../src/trackers/index";

describe("MailTrackerBlocker data", () => {
  it("has the upstream size (~320 services, ~468 patterns)", () => {
    expect(MTB_RULES).toHaveLength(320);
    expect(MTB_PATTERN_COUNT).toBe(468);
    expect(MTB_RULES.flatMap(([, patterns]) => patterns)).toHaveLength(
      MTB_PATTERN_COUNT
    );
  });

  it("is up to date with the vendored source", () => {
    const script = path.join(
      import.meta.dirname,
      "../scripts/generate-trackers.ts"
    );
    expect(() =>
      execFileSync("bun", [script, "--check"], { stdio: "pipe" })
    ).not.toThrow();
  });

  it("drops commented-out upstream rules", () => {
    const all = MTB_RULES.flatMap(([, patterns]) => patterns);
    expect(all).not.toContain("/O/[a-z0-9_-]+");
    expect(all).not.toContain("click.uber.com/q/");
  });
});

describe(isTrackerUrl, () => {
  it.each([
    ["https://github.com/notifications/beacon/ABC.gif", "GitHub"],
    ["https://x.us1.list-manage.com/track/open.php?u=1", "Intuit"],
    ["https://T.HUBSPOTEMAIL.net/e2t/o/abc", "HubSpot"],
    ["https://mailtrack.io/trace/mail/1.png", "MailTrack"],
    ["https://www.google-analytics.com/collect?v=1", "Google"],
    ["https://pixel.wp.com/t.gif", "WordPress"],
    ["https://eoapxl.com/x", "Email on Acid"],
  ])("%s → %s", (url, service) => {
    expect(isTrackerUrl(url)?.service).toBe(service);
  });

  it("leaves ordinary images alone", () => {
    expect(
      isTrackerUrl("https://cdn.example.com/newsletter/hero.jpg")
    ).toBeNull();
  });

  it("can turn the list off, keeping custom blocks", () => {
    const url = "https://github.com/notifications/beacon/x.gif";
    expect(isTrackerUrl(url, { mailTrackerBlocker: false })).toBeNull();
    expect(
      isTrackerUrl("https://t.evil.example/x.png", {
        blockedHosts: ["evil.example"],
        mailTrackerBlocker: false,
      })
    ).toStrictEqual({ service: "Custom list", source: "custom" });
  });

  it("allow list exempts a host from the list", () => {
    expect(
      isTrackerUrl("https://github.com/notifications/beacon/x.gif", {
        allowedHosts: ["github.com"],
      })
    ).toBeNull();
  });

  it("matches hosts and subdomains only", () => {
    expect(hostMatches("a.b.example", "b.example")).toBeTruthy();
    expect(hostMatches("b.example", ".B.Example.")).toBeTruthy();
    expect(hostMatches("notb.example", "b.example")).toBeFalsy();
    expect(hostMatches("b.example", "")).toBeFalsy();
  });

  it("stays fast on adversarial input (no catastrophic backtracking)", () => {
    const inputs = [
      `https://x.example/${"/CI0/".repeat(800)}`,
      `https://x.example/${"a/".repeat(2000)}`,
      `https://x.example/${"/e/".repeat(1300)}`,
      `https://${"a.".repeat(1000)}example/${"=".repeat(1000)}`,
      `https://x.example/?${"%".repeat(MAX_TRACKER_INPUT * 2)}`,
    ];
    const start = performance.now();
    for (const input of inputs) {
      isTrackerUrl(input);
    }
    expect(performance.now() - start).toBeLessThan(2000);
  });
});

describe("linear equivalents of slow upstream patterns", () => {
  it.each(Object.entries(LINEAR_EQUIVALENTS))(
    "%s agrees with its regex",
    (pattern, linear) => {
      expect(
        MTB_RULES.some(([, patterns]) => patterns.includes(pattern))
      ).toBeTruthy();
      const regex = new RegExp(pattern, "iu");
      const alphabet = [
        "/",
        "c",
        "C",
        "i",
        "I",
        "0",
        "=",
        "1",
        "2",
        "3",
        "x",
        "\n",
      ];
      let seed = 42;
      const random = (): number => {
        seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
        return seed / 2_147_483_648;
      };
      for (let i = 0; i < 20_000; i += 1) {
        let input = "";
        const length = Math.floor(random() * 24);
        for (let j = 0; j < length; j += 1) {
          input +=
            random() < 0.3
              ? "/CI0/"
              : (alphabet[Math.floor(random() * alphabet.length)] ?? "");
        }
        expect({ input, matches: linear(input) }).toStrictEqual({
          input,
          matches: regex.test(input),
        });
      }
      expect(linear("https://x/CI0/a/b=123")).toBeTruthy();
      expect(linear("https://x/CI0//=123")).toBeFalsy();
    }
  );
});

describe("spy-pixel rule", () => {
  it.each([
    [{ height: "1", width: "1" }, "small-side"],
    [{ height: 0, width: 0 }, "small-side"],
    [{ width: "2" }, "small-side"],
    [{ height: "2", width: "400" }, "small-side"],
    [{ height: "5", width: "5" }, "small-area"],
    [{ height: "5", width: "0" }, "small-side"],
    [{ height: "1px", width: "1px" }, "small-side"],
    [{ style: "width:1px;height:1px" }, "small-side"],
    [
      { height: "400", style: "width: 1px !important", width: "600" },
      "small-side",
    ],
    [{ height: "400", style: "max-width:1px", width: "600" }, "max-size"],
    [{ height: "400", style: "MAX-HEIGHT: 0", width: "600" }, "max-size"],
    [{ style: "width:0em;height:10px" }, "small-side"],
  ] as const)("%o → %s", (input, reason) => {
    expect(spyPixelReason(input)).toBe(reason);
  });

  it.each([
    { height: "5", width: "6" },
    { height: "3", width: "3", url: "https://x/spacer.gif" },
    { height: "1%", width: "1%" },
    { width: "120" },
    {},
    { height: "1", url: "https://x/images/transparent.gif", width: "1" },
    { height: "1", url: "cid:img1", width: "1" },
    { height: "40", style: "width:1em", width: "300" },
  ])("%o is not a spy pixel", (input) => {
    expect(isSpyPixel(input)).toBeFalsy();
  });

  it("CSS size overrides the attribute", () => {
    expect(
      isSpyPixel({ height: "1", style: "height:300px", width: "300" })
    ).toBeFalsy();
  });

  it("parses dimensions like the HTML spec", () => {
    expect(parseHtmlDimension(" 12px")).toBe(12);
    expect(parseHtmlDimension("50%")).toBeNull();
    expect(parseHtmlDimension("abc")).toBeNull();
    expect(parseCssPx("0")).toBe(0);
    expect(parseCssPx("3px")).toBe(3);
    expect(parseCssPx("3em")).toBeNull();
    expect(parseCssPx("-1px")).toBeNull();
  });
});
