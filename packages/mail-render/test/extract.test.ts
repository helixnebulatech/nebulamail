// @vitest-environment node
// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The BFF runs on a server: no global window. It injects an inert parser.

import { JSDOM } from "jsdom";
import { describe, expect, it, vi } from "vitest";

import { extractRemoteImageUrls } from "../src/extract";
import { MAX_IMAGE_REFS, createImageDecider } from "../src/image-policy";
import { DEFAULT_PRIVACY, renderEmail } from "../src/index";
import { SanitizerUnavailableError, sanitizerAvailable } from "../src/proton";
import * as match from "../src/trackers/match";

const { window } = new JSDOM("");
const parser = new window.DOMParser();

describe("extractRemoteImageUrls (server)", () => {
  it("finds img, srcset, background, inline and <style> CSS URLs", () => {
    const html = `<html><head><style>.a{background:url(https://cdn.example/style.png)} @import url(https://cdn.example/x.css); @font-face{src:url(https://cdn.example/f.woff)}</style></head>
      <body background="https://cdn.example/body.png">
      <img src="https://cdn.example/a.png" srcset="https://cdn.example/a2.png 2x, /rel.png 3x" width="100" height="100">
      <div style="background:u\\72l(https://cdn.example/escaped.png)">x</div>
      <img src="http://cdn.example/insecure.png" width="50" height="50">
      <img src="data:image/png;base64,AAAA"><img src="cid:x@y">
      </body></html>`;
    expect(extractRemoteImageUrls(html, { parser }).urls).toStrictEqual([
      "https://cdn.example/style.png",
      "https://cdn.example/body.png",
      "https://cdn.example/a.png",
      "https://cdn.example/a2.png",
      "https://cdn.example/escaped.png",
      "http://cdn.example/insecure.png",
    ]);
  });

  it("never returns trackers or spy pixels", () => {
    const html = `<img src="https://github.com/notifications/beacon/x.gif" width="40" height="40">
      <img src="https://px.example/p.gif" width="1" height="1">
      <img src="https://cdn.example/ok.png" srcset="https://substack.com/o/x 2x" width="40" height="40">
      <div style="width:1px;height:1px;background:url(https://px.example/css.gif)"></div>`;
    const result = extractRemoteImageUrls(html, { parser });
    expect(result.urls).toStrictEqual([]);
    expect(result.trackers.map((t) => t.service).toSorted()).toStrictEqual([
      "GitHub",
      "Spy pixel",
      "Spy pixel",
      "Substack",
    ]);
  });

  it("respects tracker settings", () => {
    const html = `<img src="https://github.com/notifications/beacon/x.gif" width="40" height="40">`;
    expect(
      extractRemoteImageUrls(html, {
        parser,
        settings: { ...DEFAULT_PRIVACY, removeTrackers: false },
      }).urls
    ).toHaveLength(1);
  });

  it("skips content the browser never shows", () => {
    const html = `<svg><image href="https://cdn.example/svg.png"/></svg><noscript><img src="https://cdn.example/ns.png"></noscript>
      <template><img src="https://cdn.example/tpl.png"></template><video poster="https://cdn.example/poster.png"></video>
      <object><img src="https://cdn.example/fallback.png" width="9" height="9"></object>`;
    expect(extractRemoteImageUrls(html, { parser }).urls).toStrictEqual([
      "https://cdn.example/fallback.png",
    ]);
  });

  it("returns nothing when the browser would fall back to text", () => {
    const html = `${"<div>".repeat(500)}<img src="https://cdn.example/a.png" width="9" height="9">`;
    expect(extractRemoteImageUrls(html, { parser }).urls).toStrictEqual([]);
    expect(extractRemoteImageUrls("", { parser }).urls).toStrictEqual([]);
  });

  it("the browser pipeline refuses to run without a DOM", () => {
    expect(sanitizerAvailable()).toBeFalsy();
    expect(() => renderEmail({ html: "<p>x</p>" }, DEFAULT_PRIVACY)).toThrow(
      TypeError
    );
    expect(new SanitizerUnavailableError().name).toBe(
      "SanitizerUnavailableError"
    );
  });

  it("decides at most MAX_IMAGE_REFS references per email, quickly", () => {
    const refs = 40_000;
    const rules = Array.from(
      { length: refs },
      (_, i) => `.c${i}{background:url(https://a${i}.test/x.png)}`
    ).join("");
    const html = `<style>${rules}</style><p>x</p>`;
    const start = performance.now();
    const { urls } = extractRemoteImageUrls(html, { parser });
    // About 2 s on an idle machine; deciding all 40 000 took about 12 s.
    // The exact limit is checked below without timing.
    expect(performance.now() - start).toBeLessThan(20_000);
    expect(urls).toHaveLength(MAX_IMAGE_REFS);
    expect(urls.at(-1)).toBe(`https://a${MAX_IMAGE_REFS - 1}.test/x.png`);
  }, 60_000);

  it("the decider blocks references past its limit", () => {
    const decide = createImageDecider(DEFAULT_PRIVACY, 2);
    expect(decide("https://a.test/1.png").kind).toBe("remote");
    expect(decide("data:image/png;base64,AAAA").kind).toBe("inline");
    expect(decide("https://a.test/1.png")).toStrictEqual({
      kind: "blocked",
      reason: "too-many",
    });
  });

  it("checks the tracker list once per URL", () => {
    const spy = vi.spyOn(match, "isTrackerUrl");
    try {
      const html = `<style>${".a{background:url(https://cdn.example/x.png)}".repeat(4000)}</style><img src="https://cdn.example/x.png" width="1" height="1">`;
      const result = extractRemoteImageUrls(html, { parser });
      expect(result.urls).toStrictEqual(["https://cdn.example/x.png"]);
      // The spy-pixel rule still applies per element.
      expect(result.trackers).toStrictEqual([
        { service: "Spy pixel", url: "https://cdn.example/x.png" },
      ]);
      expect(spy).toHaveBeenCalledOnce();
    } finally {
      spy.mockRestore();
    }
  });
});
