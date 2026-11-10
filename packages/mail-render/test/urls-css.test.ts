// @vitest-environment node
// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs
// oxlint-disable no-script-url -- these tests feed javascript: URLs to the pipeline on purpose

import { describe, expect, it } from "vitest";

import { cssUrl, findCssUrls, neutralizeCss, rewriteCssUrls } from "../src/css";
import { classifyImageUrl, normalizeCid, parseSrcset } from "../src/urls";

describe(classifyImageUrl, () => {
  it.each([
    ["data:image/png;base64,AAAA", "inline"],
    ["DATA:IMAGE/GIF;base64,R0lG", "inline"],
    ["data:image/webp,xx", "inline"],
    ["https://cdn.example/a.png", "remote"],
    ["HTTP://cdn.example/a.png", "remote"],
    [" \n https://cdn.example/a.png\t", "remote"],
    ["cid:part1@x", "cid"],
  ])("%s → %s", (url, kind) => {
    expect(classifyImageUrl(url).kind).toBe(kind);
  });

  it.each([
    ["data:image/svg+xml,<svg/>", "svg"],
    ["data:image/svg+xml;base64,PHN2Zy8+", "svg"],
    ["data:text/html,<script>", "data-other"],
    ["data:image/png", "data-other"],
    ["/x.png", "relative"],
    ["x.png", "relative"],
    ["//host/x.png", "relative"],
    ["\\\\host\\x.png", "relative"],
    ["javascript:alert(1)", "scheme"],
    ["java\tscript:alert(1)", "scheme"],
    ["blob:https://mail.example/1", "scheme"],
    ["file:///etc/passwd", "scheme"],
    ["https://user:pw@host/x.png", "invalid"],
    ["https://", "invalid"],
    [`https://cdn.example/${"a".repeat(3000)}`, "too-long"],
    ["", "empty"],
    ["cid:", "empty"],
  ])("blocks %s (%s)", (url, reason) => {
    expect(classifyImageUrl(url)).toStrictEqual({ kind: "blocked", reason });
  });

  it("normalizes remote URLs", () => {
    expect(classifyImageUrl("https://CDN.example/a b.png")).toStrictEqual({
      kind: "remote",
      secure: true,
      url: "https://cdn.example/a%20b.png",
    });
  });

  it("normalizes Content-IDs", () => {
    expect(normalizeCid("%3CImage001.PNG@01D%3E")).toBe("image001.png@01d");
    expect(normalizeCid("<a@b>")).toBe("a@b");
    expect(normalizeCid("%E0%A4%A")).toBe("%e0%a4%a");
  });
});

describe(parseSrcset, () => {
  it("splits candidates and descriptors", () => {
    expect(parseSrcset("a.png 1x, b.png 2x,c.png 480w , d.png")).toStrictEqual([
      { descriptor: "1x", url: "a.png" },
      { descriptor: "2x", url: "b.png" },
      { descriptor: "480w", url: "c.png" },
      { descriptor: "", url: "d.png" },
    ]);
    expect(parseSrcset("a.png,b.png")).toStrictEqual([
      { descriptor: "", url: "a.png,b.png" },
    ]);
    expect(parseSrcset("a.png, ,b.png")).toStrictEqual([
      { descriptor: "", url: "a.png" },
      { descriptor: "", url: "b.png" },
    ]);
    expect(parseSrcset("")).toStrictEqual([]);
  });
});

describe("CSS", () => {
  it("rewrites URLs after characters whose lower case is longer (İ)", () => {
    expect(
      rewriteCssUrls(
        "İİİ a{background:proton-url(https://x.test/a.png)}",
        () => "https://p/1"
      )
    ).toBe('İİİ a{background:url("https://p/1")}');
  });

  it("finds proton-url references with their context", () => {
    const css = `@blocked-import proton-url('a.css'); @font-face{src:proton-url(f.woff)} .x{background:proton-url( "i.png" )} .y{b:PROTON-URL(j.png)}`;
    expect(findCssUrls(css).map((r) => [r.url, r.context])).toStrictEqual([
      ["a.css", "import"],
      ["f.woff", "font"],
      ["i.png", "image"],
      ["j.png", "image"],
    ]);
  });

  it("handles a missing closing parenthesis", () => {
    expect(rewriteCssUrls("a{b:proton-url(x.png", () => null)).toBe("a{b:none");
  });

  it("neutralizes anything Proton left that loads", () => {
    expect(neutralizeCss("a{color:red}")).toBe("a{color:red}");
    expect(neutralizeCss("a{b:url(x)}")).toBe("a{b:blocked-url(x)}");
    expect(neutralizeCss("a{b:\\75 rl(x)}")).toBe("a{b:blocked-url(x)}");
    expect(neutralizeCss("@import proton-url(x);")).toBe(
      "@blocked-import proton-url(x);"
    );
    expect(neutralizeCss("a{b:image-set('x' 1x)}")).toBe(
      "a{b:blocked-image-set('x' 1x)}"
    );
    expect(neutralizeCss("a{-moz-binding:x}")).toBe(
      "a{blocked--moz-binding:x}"
    );
  });

  it("writes url() literals that cannot break out", () => {
    expect(cssUrl('https://x/a")}</style><b>')).toBe(
      'url("https://x/a\\22 )}\\3c /style\\3e \\3c b\\3e ")'
    );
    expect(cssUrl("a\nb\\c")).toBe('url("a\\a b\\5c c")');
  });
});
