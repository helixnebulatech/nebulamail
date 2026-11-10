// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable no-script-url -- these tests feed javascript: URLs to the sanitizer on purpose

import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  COMPOSE_FONTS,
  COMPOSE_FONT_SIZES,
  cidUrl,
  escapeHtml,
  normalizeCid,
  normalizeContentId,
  sanitizeComposeHtml,
  textToComposeHtml,
} from "../src/index";

// Anything that could make the browser fetch or run something.
const LOADING =
  /\b(?:src|srcset|background|poster|on\w+)\s*=|url\(|<img|<style|<link|<script|<iframe|javascript:|@import/iu;

describe(sanitizeComposeHtml, () => {
  it("never throws when the sanitizer leaves no body (frameset)", () => {
    expect(() => sanitizeComposeHtml("<frameset></frameset>")).not.toThrow();
    const result = sanitizeComposeHtml(
      "<frameset><frame src=x></frameset><noframes>hola</noframes>"
    );
    expect(result).not.toMatch(LOADING);
    expect(result).not.toContain("<frame");
  });

  it("keeps simple formatting, lists, quotes and links", () => {
    expect(
      sanitizeComposeHtml(
        `<p>Hola <b>mundo</b> <i>y</i> <u>tú</u></p><ul><li>uno</li></ul><blockquote>cita</blockquote><a href="https://example.com/x">enlace</a>`
      )
    ).toBe(
      `<p>Hola <b>mundo</b> <i>y</i> <u>tú</u></p><ul><li>uno</li></ul><blockquote>cita</blockquote><a href="https://example.com/x">enlace</a>`
    );
  });

  it("drops images (remote, data:, cid:) without fetching them", () => {
    const html = `<p>a<img src="https://tracker.example/p.gif">b</p><img src="data:image/png;base64,AAAA"><img src="cid:x"><picture><source srcset="https://x/y.png"></picture>`;
    expect(sanitizeComposeHtml(html)).toBe("<p>ab</p>");
  });

  it("keeps only the listed cid: images, with src and alt only", () => {
    const html = `<p>a<img src="cid:Logo@x" alt="Logo" width="1" onerror="alert(1)" srcset="https://t.example/a.png 2x" style="background:url(https://t.example/b)">b</p><img src="cid:other@x"><img src="https://tracker.example/p.gif" alt="remote"><img src="data:image/png;base64,AAAA">`;
    expect(sanitizeComposeHtml(html, { inlineCids: ["logo@X"] })).toBe(
      // Matched like the reader (case-insensitive), written as listed.
      `<p>a<img src="cid:logo@X" alt="Logo">b</p>`
    );
  });

  // RFC 2392 §2: a cid: URL is percent-decoded to get the Content-ID, so a
  // "%" in the Content-ID is written as "%25".
  it("percent-encodes the Content-ID it writes into cid: URLs", () => {
    expect([
      sanitizeComposeHtml(`<img src="cid:a%2541b@x">`, {
        inlineCids: ["a%41b@x"],
      }),
      // "cid:a%41b@x" names the Content-ID "aAb@x", not "a%41b@x".
      sanitizeComposeHtml(`<img src="cid:a%41b@x">`, {
        inlineCids: ["a%41b@x"],
      }),
    ]).toStrictEqual([`<img src="cid:a%2541b@x" alt="">`, ""]);
  });

  it("round-trips Content-IDs through cid: URLs", () => {
    const ids = ["a%41b@x", "Logo.1@x", "p+q=r$s!~*'()/t@x"];

    expect(
      ids.map((id) => normalizeCid(cidUrl(id).slice("cid:".length)))
    ).toStrictEqual(ids.map(normalizeContentId));
    expect(normalizeContentId("<a%41b@X>")).toBe("a%41b@x");
  });

  it("drops cid: images whose Content-ID can't be written back safely", () => {
    expect(
      sanitizeComposeHtml(`<img src='cid:a"b'>`, { inlineCids: [`a"b`] })
    ).toBe("");
  });

  it("removes styles that could load something and keeps safe formatting", () => {
    expect(
      sanitizeComposeHtml(
        `<span style="font-weight: 700; background: url(https://x/y.png); color: red">a</span><span style="font-style:italic;background-image:image-set('x.png' 1x)">b</span><span style="font-weight: \\75 rl(x)">c</span>`
      )
    ).toBe(
      `<span style="font-weight: 700">a</span><span style="font-style: italic">b</span><span>c</span>`
    );
  });

  it("keeps the editor's colours, highlights, alignment, headings and rules", () => {
    expect(
      sanitizeComposeHtml(
        `<h2 style="text-align: center">t</h2><p style="text-align: right"><span style="color: #C5221F">a</span> <span style="color: rgb(26, 115, 232)">b</span> <mark data-color="#FFF3A3" style="background-color: rgb(255, 243, 163); color: inherit">c</mark></p><hr><pre><code>d</code></pre>`
      )
    ).toBe(
      `<h2 style="text-align: center">t</h2><p style="text-align: right"><span style="color: #C5221F">a</span> <span style="color: #1a73e8">b</span> <mark style="background-color: #fff3a3" data-color="#FFF3A3">c</mark></p><hr><pre><code>d</code></pre>`
    );
  });

  it("only keeps plain colours and known alignments", () => {
    expect(
      sanitizeComposeHtml(
        `<span style="color: rgb(300, 0, 0)">a</span><span style="color: rgba(0, 0, 0, 0)">b</span><span style="color: var(--x)">c</span><span style="background-color: #12345">d</span><p style="text-align: -webkit-center">e</p><mark data-color="url(x)">f</mark><span data-color="#fff">g</span>`
      )
    ).toBe(
      `<span>a</span><span>b</span><span>c</span><span>d</span><p>e</p><mark>f</mark><span>g</span>`
    );
  });

  it("keeps only the editor's fonts and sizes, written canonically", () => {
    expect(
      sanitizeComposeHtml(
        `<span style="font-family: Georgia, serif">a</span><span style="font-family: arial, helvetica, sans-serif; font-size: 14PX">b</span><span style='font-family: "Times New Roman", Times, serif'>c</span><span style="font-family: monospace; font-size: 32px">d</span>`
      )
    ).toBe(
      `<span style="font-family: Georgia, serif">a</span><span style="font-family: Arial, sans-serif; font-size: 14px">b</span><span style="font-family: Times New Roman, serif">c</span><span style="font-family: monospace; font-size: 32px">d</span>`
    );
  });

  // (Proton decodes CSS escapes first: "\\41 rial" arrives as "Arial".)
  it("drops other fonts, sizes and anything that could load a font", () => {
    const result = sanitizeComposeHtml(
      `<span style="font-family: Comic Sans MS, cursive">a</span><span style="font-family: url(https://x/f.woff)">b</span><span style="font-family: Arial; src: url(https://x/f.woff)">c</span><span style="font-family: var(--f)">d</span><span style="font-family: Arial !important">e</span><span style="font-size: 13px">f</span><span style="font-size: calc(10px + 4px)">g</span><span style="font-size: 2em">h</span><span style="font-size: 14px; font-size: url(x)">i</span><span style="font: 14px Arial">j</span><span style="font-family: \\41 rial">k</span><style>@font-face{font-family:x;src:url(https://x/f.woff)} @import "https://x/a.css";</style>`
    );
    expect(result).toBe(
      `<span>a</span><span>b</span><span style="font-family: Arial, sans-serif">c</span><span>d</span><span>e</span><span>f</span><span>g</span><span>h</span><span style="font-size: 14px">i</span><span>j</span><span style="font-family: Arial, sans-serif">k</span>`
    );
    expect(result).not.toMatch(LOADING);
  });

  it("keeps dir only as ltr or rtl", () => {
    expect(
      sanitizeComposeHtml(
        `<p dir="RTL">a</p><p dir="ltr">b</p><p dir="auto">c</p><p dir="rtl;color:red">d</p><h2 dir="rtl">e</h2>`
      )
    ).toBe(
      `<p dir="rtl">a</p><p dir="ltr">b</p><p>c</p><p>d</p><h2 dir="rtl">e</h2>`
    );
  });

  it("drops <style>, scripts, forms and event handlers", () => {
    const result = sanitizeComposeHtml(
      `<style>@import url(https://x/a.css); p{background:url(https://x)}</style><p onclick="alert(1)">x</p><script>alert(1)</script><form action="https://evil"><input value="pw"><button>Pay</button></form>`
    );
    expect(result).toBe("<p>x</p>Pay");
    expect(result).not.toMatch(LOADING);
  });

  it("keeps only absolute http(s)/mailto links", () => {
    expect(
      sanitizeComposeHtml(
        `<a href="javascript:alert(1)">a</a><a href="/relative">b</a><a href="mailto:ana@x.com">c</a><a href="https://x.com" target="_blank" class="c" id="i">d</a>`
      )
    ).toBe(
      `<a>a</a><a>b</a><a href="mailto:ana@x.com">c</a><a href="https://x.com/">d</a>`
    );
  });

  it("turns tables and unknown blocks into divs and keeps the text", () => {
    expect(
      sanitizeComposeHtml(
        `<table><tr><td>uno</td><td>dos</td></tr></table><center>tres</center><font color="red">cuatro</font>`
      )
    ).toBe(
      // The parser adds <tbody>, which is unwrapped.
      "<div><div><div>uno</div><div>dos</div></div></div><div>tres</div>cuatro"
    );
  });

  it("keeps the editor's own blocks (signature, quote) and dir", () => {
    expect(
      sanitizeComposeHtml(
        `<div data-signature="x" data-other="y"><p dir="rtl">Ana</p></div><div data-quote><blockquote>q</blockquote></div>`
      )
    ).toBe(
      `<div data-signature=""><p dir="rtl">Ana</p></div><div data-quote=""><blockquote>q</blockquote></div>`
    );
  });

  it("falls back to escaped text when the HTML is too big", () => {
    expect(
      sanitizeComposeHtml("<p>a</p><p><b>b</b></p>", {
        limits: { maxHtmlLength: 10 },
      })
    ).toBe("<p>a</p><p></p><p>b</p>");
  });

  it("returns an empty string for empty input", () => {
    expect(sanitizeComposeHtml("  ")).toBe("");
  });

  it("keeps only canonical fonts, sizes and directions from a hostile email", () => {
    const html = readFileSync(
      path.join(
        import.meta.dirname,
        "../../../security/hostile-emails/compose-fonts.html"
      ),
      "utf-8"
    );
    const result = sanitizeComposeHtml(html);
    expect(result).not.toMatch(LOADING);
    const fonts = new Set(COMPOSE_FONTS.map((font) => font.stack));
    const sizes = new Set<string>(COMPOSE_FONT_SIZES);
    const styles = [...result.matchAll(/style="(?<style>[^"]*)"/gu)].flatMap(
      (match) => (match.groups?.style ?? "").split("; ")
    );
    expect(styles.length).toBeGreaterThan(0);
    for (const declaration of styles) {
      const [property = "", value = ""] = declaration.split(": ");
      expect(
        (property === "font-family" && fonts.has(value)) ||
          (property === "font-size" && sizes.has(value))
      ).toBeTruthy();
    }
    const dirs = [...result.matchAll(/dir="(?<dir>[^"]*)"/gu)].map(
      (match) => match.groups?.dir
    );
    expect(dirs).toStrictEqual(["rtl"]);
    expect(result).toContain(
      `<span style="font-family: Times New Roman, serif; font-size: 32px">Entity quotes</span>`
    );
  });

  it("never lets a hostile email load or run anything", () => {
    const dir = path.join(
      import.meta.dirname,
      "../../../security/hostile-emails"
    );
    const files = readdirSync(dir).filter((name) => name.endsWith(".html"));
    expect(files.length).toBeGreaterThan(5);
    const loading = files.filter((name) =>
      LOADING.test(
        sanitizeComposeHtml(readFileSync(path.join(dir, name), "utf-8"))
      )
    );
    expect(loading).toStrictEqual([]);
  });
});

describe(textToComposeHtml, () => {
  it("makes one escaped paragraph per line", () => {
    expect(textToComposeHtml("a <b>\r\n\nc & d")).toBe(
      "<p>a &lt;b&gt;</p><p></p><p>c &amp; d</p>"
    );
  });
});

describe(escapeHtml, () => {
  it("escapes the five HTML characters", () => {
    expect(escapeHtml(`<a href="x">'&'</a>`)).toBe(
      "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
    );
  });
});
