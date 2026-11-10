// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs

// Every regex and scan on email content must be linear (deepsec: the
// earlier ones took seconds to hours on a 2 MB email).

import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

import {
  cssCommentHazard,
  findCssUrls,
  neutralizeCss,
  stripCssComments,
} from "../src/css";
import { extractRemoteImageUrls } from "../src/extract";
import { DEFAULT_PRIVACY, renderEmail } from "../src/index";
import { registrableDomain } from "../src/links";
import { parseCssPx } from "../src/trackers/spy-pixel";
import { classifyImageUrl, normalizeUrlInput, parseSrcset } from "../src/urls";

const parser = new new JSDOM("").window.DOMParser();

// The best of three runs: about complexity (the quadratic versions took
// seconds to hours), not about a busy CI machine.
const timed = (run: () => unknown, rounds = 3): number => {
  let best = Number.POSITIVE_INFINITY;
  for (let round = 0; round < rounds; round += 1) {
    const start = performance.now();
    run();
    best = Math.min(best, performance.now() - start);
  }
  return best;
};

describe("linear on adversarial input (urls, css, spy-pixel)", () => {
  it("URL edges, srcset commas and trailing dots", () => {
    expect(
      timed(() => classifyImageUrl(`a${" ".repeat(1_000_000)}b`))
    ).toBeLessThan(1000);
    expect(
      timed(() => parseSrcset(`a${",".repeat(1_000_000)}b,`))
    ).toBeLessThan(1000);
    expect(
      timed(() => registrableDomain(`a${".".repeat(1_000_000)}b`))
    ).toBeLessThan(1000);
  });

  it("CSS lengths", () => {
    expect(
      timed(() => {
        parseCssPx(`${"1".repeat(1_000_000)}!`);
        parseCssPx(`${"1".repeat(3000)}${" ".repeat(3000)}!`);
      })
    ).toBeLessThan(1000);
  });

  it("CSS comments", () => {
    const css = "/*a".repeat(300_000);
    expect(timed(() => neutralizeCss(css))).toBeLessThan(1000);
    expect(timed(() => stripCssComments(css))).toBeLessThan(1000);
    expect(cssCommentHazard(css)).toBeTruthy();
  });

  it("CSS URLs inside many @font-face blocks", () => {
    const k = 100_000;
    const css = `${"@font-face{}".repeat(k)}${"proton-url(a)".repeat(k)}`;
    // It was 75 s for a 1 MB input.
    let refs = 0;
    expect(
      timed(() => {
        refs = findCssUrls(css).length;
      })
    ).toBeLessThan(1000);
    expect(refs).toBe(k);
  });

  it("the server ignores the whole email when its CSS is a hazard", () => {
    const html = `<style>${"/*a".repeat(300_000)}</style><img src="https://x.test/a.png" width="100" height="100">`;
    let urls: readonly string[] = ["not run"];
    expect(
      timed(() => {
        ({ urls } = extractRemoteImageUrls(html, { parser }));
      }, 1)
    ).toBeLessThan(3000);
    expect(urls).toStrictEqual([]);
  });

  it("a spy pixel with a hazardous style gets no ticket", () => {
    const html = `<img src="https://x.test/pixel.gif" style="width:1px;height:1px${"/*a".repeat(20)}"><img src="https://x.test/b.png" width="100" height="100">`;
    expect(extractRemoteImageUrls(html, { parser }).urls).toStrictEqual([]);
  });

  it("the browser falls back to text when a style attribute is a hazard", () => {
    const html = `<p style="color:red${"/*a".repeat(300_000)}">x</p>`;
    let result: ReturnType<typeof renderEmail> | undefined;
    expect(
      timed(() => {
        result = renderEmail({ html, text: "plain" }, DEFAULT_PRIVACY);
      }, 1)
    ).toBeLessThan(5000);
    expect(result).toMatchObject({
      fallbackReason: "too-complex",
      view: "text",
    });
  });
});

describe("same results after the linear rewrites", () => {
  it("parses CSS lengths as before", () => {
    expect(parseCssPx("10px")).toBe(10);
    expect(parseCssPx("0")).toBe(0);
    expect(parseCssPx("0em")).toBe(0);
    expect(parseCssPx("1px !important")).toBe(1);
    expect(parseCssPx(" 1PX ! important ")).toBe(1);
    expect(parseCssPx(".5")).toBe(0.5);
    expect(parseCssPx("2.5px")).toBe(2.5);
    expect(parseCssPx("-1px")).toBeNull();
    expect(parseCssPx("10 px")).toBeNull();
    expect(parseCssPx("1.")).toBeNull();
    expect(parseCssPx("10em")).toBeNull();
  });

  it("trims URL edges and dots like before", () => {
    expect(normalizeUrlInput(" \u0000https://x/ \n")).toBe("https://x/");
    expect(normalizeUrlInput("ht\ttp://x/\u0001")).toBe("http://x/");
    expect(normalizeUrlInput(" \u0000 ")).toBe("");
    expect(registrableDomain("mail.example.com...")).toBe("example.com");
    expect(parseSrcset("a.png,, b.png 2x")).toStrictEqual([
      { descriptor: "", url: "a.png" },
      { descriptor: "2x", url: "b.png" },
    ]);
  });

  it("strips CSS comments like the CSS parser", () => {
    expect(stripCssComments("a/* x */b/**/c")).toBe("abc");
    expect(stripCssComments("a/* never closed")).toBe("a");
    expect(stripCssComments("a*/b")).toBe("a*/b");
    expect(cssCommentHazard("/*a*/".repeat(1000))).toBeFalsy();
    expect(cssCommentHazard("/*".repeat(16))).toBeFalsy();
    expect(cssCommentHazard("/**/".repeat(10) + "/*x".repeat(17))).toBeTruthy();
  });

  it("finds font contexts with the moving pointer", () => {
    const css =
      "a{background:proton-url(1)} @font-face{src:proton-url(2)} b{x:proton-url(3)} @font-face{src:proton-url(4)}";
    expect(findCssUrls(css).map((ref) => [ref.url, ref.context])).toStrictEqual(
      [
        ["1", "image"],
        ["2", "font"],
        ["3", "image"],
        ["4", "font"],
      ]
    );
  });
});
