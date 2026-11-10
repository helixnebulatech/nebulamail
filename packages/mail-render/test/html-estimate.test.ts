// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs

// The limits prefilter (html-estimate.ts) must not underestimate what the
// parser builds: a few bytes can make it build a huge tree (deepsec).

import { JSDOM } from "jsdom";
import { describe, expect, it, vi } from "vitest";

import { extractRemoteImageUrls } from "../src/extract";
import { estimateHtml } from "../src/html-estimate";
import { DEFAULT_PRIVACY, checkHtmlLimits, renderEmail } from "../src/index";
import { DEFAULT_LIMITS, measureTree } from "../src/limits";

const { window } = new JSDOM("");
const parser = new window.DOMParser();

const parsed = (html: string): { elements: number; depth: number } =>
  measureTree(parser.parseFromString(html, "text/html").documentElement);

/** `<p>`, K distinct `<b>` and M paragraphs: K·M elements (11 KB → 600 000). */
const rebuiltFormatting = (k: number, m: number): string =>
  `<p>${Array.from({ length: k }, (_, i) => `<b id=a${i}>`).join("")}${"<p>x".repeat(m)}`;

/** `<p>`, 20 distinct `<b>` and 50 times `part`. */
const rebuiltWith = (part: string): string =>
  `<p>${Array.from({ length: 20 }, (_, i) => `<b id=a${i}>`).join("")}${part.repeat(50)}`;

describe("estimated elements", () => {
  it("counts the formatting elements the parser rebuilds", () => {
    const html = rebuiltFormatting(300, 2000);
    expect(html.length).toBeLessThan(12_000);
    expect(checkHtmlLimits(html)).toBe("too-many-elements");
  });

  it("never underestimates the parsed tree on small samples", () => {
    const samples = [
      rebuiltFormatting(20, 50),
      "<p><b><i>x<p>y<p>z</b>w",
      "<b><p>x</b>y<p>z",
      "<a href=1>x<a href=2>y<div>z</a>w</div>",
      "<b><div><i>x</b>y</i>z</div>",
      "<p><b>x".repeat(200),
      "<font face=Arial><p>x</p>".repeat(30),
      "<table><tr><td><b>x</td><td><i>y</table>".repeat(20),
      "<svg><g><style><g></g></style></g></svg>".repeat(20),
      "<ul><li><b>x<li>y<li>z</ul>".repeat(20),
    ];
    for (const html of samples) {
      const estimate = estimateHtml(html);
      const real = parsed(html);
      expect({ elements: estimate.elements, html }).toStrictEqual({
        elements: Math.max(estimate.elements, real.elements),
        html,
      });
    }
  });

  it("the server refuses it without parsing, the browser shows text", () => {
    const html = rebuiltFormatting(300, 2000);
    const spy = vi.fn<DOMParser["parseFromString"]>((source, type) =>
      parser.parseFromString(source, type)
    );
    expect(
      extractRemoteImageUrls(`${html}<img src="https://x.test/a.png">`, {
        parser: { parseFromString: spy },
      })
    ).toStrictEqual({ trackers: [], urls: [] });
    expect(spy).not.toHaveBeenCalled();
    expect(renderEmail({ html, text: "plain" }, DEFAULT_PRIVACY)).toMatchObject(
      { fallbackReason: "too-many-elements", view: "text" }
    );
  });

  it("counts `</br>` as the `<br>` it is (formatting rebuilt too)", () => {
    const formatting = Array.from(
      { length: 300 },
      (_, i) => `<b id=a${i}>`
    ).join("");
    const html = `<p>${formatting}${"<p></br>".repeat(2000)}`;
    expect(html.length).toBeLessThan(25_000);
    expect(checkHtmlLimits(html)).toBe("too-many-elements");
    const spy = vi.fn<DOMParser["parseFromString"]>((source, type) =>
      parser.parseFromString(source, type)
    );
    expect(
      extractRemoteImageUrls(`${html}<img src="https://x.test/a.png">`, {
        parser: { parseFromString: spy },
      })
    ).toStrictEqual({ trackers: [], urls: [] });
    expect(spy).not.toHaveBeenCalled();
    expect(renderEmail({ html, text: "plain" }, DEFAULT_PRIVACY)).toMatchObject(
      { fallbackReason: "too-many-elements", view: "text" }
    );
    // Small samples, also out of foreign content: never underestimated.
    for (const sample of [
      rebuiltWith("<p></br>"),
      rebuiltWith("<p><svg></br>"),
    ]) {
      const estimate = estimateHtml(sample);
      const real = parsed(sample);
      expect(estimate.elements).toBeGreaterThanOrEqual(real.elements);
    }
  });

  it("counts every start tag", () => {
    expect(checkHtmlLimits("<p>x".repeat(60_000))).toBe("too-many-elements");
    expect(
      renderEmail({ html: "<p>x".repeat(60_000) }, DEFAULT_PRIVACY)
    ).toMatchObject({ fallbackReason: "too-many-elements", view: "text" });
  });

  it("the server also checks the parsed tree", () => {
    // Cells add no estimated depth (their end tags are optional), but they
    // do in the parsed tree: table > tbody > tr > td.
    const html = `${"<table><td>".repeat(100)}<img src="https://x.test/a.png">`;
    const limits = { ...DEFAULT_LIMITS, maxNestingDepth: 150 };
    expect(checkHtmlLimits(html, limits)).toBeNull();
    expect(parsed(html).depth).toBeGreaterThan(150);
    expect(extractRemoteImageUrls(html, { limits, parser }).urls).toStrictEqual(
      []
    );
  });

  it("no false positives on ordinary newsletters", () => {
    const newsletters = [
      `<font face=Arial>${"<p>texto</p>".repeat(5000)}</font>`,
      "<table><tr><td>x</table>".repeat(5000),
      "<p><b>Título<p>texto".repeat(3000),
      `${"İ".repeat(50)}${"<br>".repeat(1000)}`,
      `<div>${"<p><span style='color:red'>x</span></p>".repeat(5000)}</div>`,
    ];
    for (const html of newsletters) {
      expect([html.slice(0, 40), checkHtmlLimits(html)]).toStrictEqual([
        html.slice(0, 40),
        null,
      ]);
    }
  });
});

/** `<table>`, K distinct `<b>` (foster-parented) and M repeats of `part`. */
const tableFormatting = (k: number, part: string, m: number): string =>
  `<table>${Array.from({ length: k }, (_, i) => `<b id=a${i}>`).join("")}${part.repeat(m)}`;

const row = (cell: string): string => `<tr><td>${cell}</td></tr>\n`;

describe("estimated elements in tables", () => {
  // Table parts clear the stack back to the table, but the foster-parented
  // `<b>` stay in the list and the next text rebuilds all of them (jsdom
  // took 90 s to 228 s on each of these, 23 KB to 43 KB).
  const amplifiers = [
    ["<tr>", tableFormatting(300, "<tr></tr>x", 2000)],
    ["<caption>", tableFormatting(300, "<caption></caption>x", 2000)],
    ["<tbody>", tableFormatting(300, "<tbody></tbody>x", 2000)],
    ["<tr><td>", tableFormatting(300, "<tr><td></td></tr>x", 2000)],
    ["<colgroup>", tableFormatting(300, "<colgroup></colgroup>x", 2000)],
    ["<table> in a table", tableFormatting(300, "<table><tr></tr>x", 2000)],
    [
      "a caption that closes a cell",
      tableFormatting(300, "<tr><td><caption></caption>x", 2000),
    ],
    [
      "a cell that closes an object without its marker",
      `<table><tr><td>${Array.from({ length: 300 }, (_, i) => `<b id=a${i}>`).join("")}<object></td>${"<td></td>x".repeat(2000)}`,
    ],
    [
      "a cell outside any table (ignored)",
      `${rebuiltFormatting(300, 0)}<td>${"<p>x".repeat(2000)}`,
    ],
    [
      "a caption outside any table (ignored)",
      `${rebuiltFormatting(300, 0)}<caption>${"<p>x".repeat(2000)}`,
    ],
  ] as const;

  it.each(amplifiers)("%s", (_name, html) => {
    expect(html.length).toBeLessThan(64_000);
    expect(checkHtmlLimits(html)).toBe("too-many-elements");
  });

  it("the server refuses them without parsing, the browser shows text", () => {
    const spy = vi.fn<DOMParser["parseFromString"]>((source, type) =>
      parser.parseFromString(source, type)
    );
    for (const [, html] of amplifiers.slice(0, 4)) {
      expect(
        extractRemoteImageUrls(`${html}<img src="https://x.test/a.png">`, {
          parser: { parseFromString: spy },
        })
      ).toStrictEqual({ trackers: [], urls: [] });
      expect(
        renderEmail({ html, text: "plain" }, DEFAULT_PRIVACY)
      ).toMatchObject({ fallbackReason: "too-many-elements", view: "text" });
    }
    expect(spy).not.toHaveBeenCalled();
  });

  it("matches the parsed tree on small samples", () => {
    const samples = [
      tableFormatting(20, "<tr></tr>x", 30),
      tableFormatting(20, "<caption></caption>x", 30),
      tableFormatting(20, "<tbody></tbody>x", 30),
      tableFormatting(20, "<tr><td></td></tr>x", 30),
      tableFormatting(20, "<table><tr></tr>x", 30),
      tableFormatting(20, "<tr><td><caption></caption>x", 30),
      tableFormatting(20, "<tr></tr> ", 30),
      "<table><col><svg></caption><ul><table>",
      "<table><font face=Arial><tr><td><b>x</b><font color=red>y</td></tr>\n<tr><td>z</table>".repeat(
        30
      ),
    ];
    for (const html of samples) {
      const estimate = estimateHtml(html);
      const real = parsed(html);
      expect({ elements: estimate.elements, html }).toStrictEqual({
        elements: Math.max(estimate.elements, real.elements),
        html,
      });
    }
  });

  it("no false positives on newsletters made of tables", () => {
    const newsletters = [
      // Unclosed formatting inside cells, closed with the cell.
      `<table>${row("<font face=Arial color=#333><b>Título<br>texto").repeat(3000)}</table>`,
      // Formatting around the rows (foster-parented), whitespace between.
      `<table><font face=Arial>\n${row("<b>x</b> y").repeat(3000)}</font></table>`,
      `<table width=600><tbody>${row("<table><tr><td><font size=2><b>x</b></font></td></tr></table>").repeat(1500)}</tbody></table>`,
      `<center><table>${"<tr><td><b><font color=red>x</font></b></td><td><i>y</i></td></tr>".repeat(2000)}</table></center>`,
    ];
    for (const html of newsletters) {
      expect([html.slice(0, 40), checkHtmlLimits(html)]).toStrictEqual([
        html.slice(0, 40),
        null,
      ]);
    }
  });
});

describe("estimated depth", () => {
  it.each([
    ["stray end tags", "<div></i>".repeat(500)],
    ["a quote that does not open a value", `<a b=c">${"<div>".repeat(500)}`],
    ["a comment ended by --!>", `<!-- --!>${"<div>".repeat(500)}`],
    ["an empty comment <!-->", `<!-->${"<div>".repeat(500)}`],
    ["an empty comment <!--->", `<!--->${"<div>".repeat(500)}`],
    ["noscript content", `<noscript>${"<div>".repeat(500)}</noscript>`],
    ["style inside svg", `<svg><style>${"<g>".repeat(500)}`],
    [
      "a div end tag that cannot leave a table cell",
      // Without the cell the next `<table>` closes the open one (and the
      // div foster-parented into it): depth 4.
      "<div><table><td></div>".repeat(250),
    ],
    ["</span> stopped by a block", "<span><div></span>".repeat(250)],
    ["a formatting end tag around a block", "<b><div></b>".repeat(500)],
    ["self-closing HTML elements (ignored /)", "<div/>".repeat(500)],
    ["self-closing foreign elements", `<svg>${"<g>".repeat(500)}`],
    [
      "raw text with İ before it",
      `${"İ".repeat(10)}<style></style>${"<div>".repeat(500)}`,
    ],
    ["a style end tag in capitals", `<style></STYLE >${"<div>".repeat(500)}`],
  ])("%s", (_name, html) => {
    expect(checkHtmlLimits(html)).toBe("too-deep");
  });

  it("does not overcount what the parser closes", () => {
    expect(checkHtmlLimits(`<svg>${"<g/>".repeat(1000)}</svg>`)).toBeNull();
    expect(checkHtmlLimits("<a b='>'><div></div></a>".repeat(1000))).toBeNull();
    expect(checkHtmlLimits(`<style>${"<div>".repeat(500)}</STYLE>`)).toBeNull();
    // `</styles>` does not end the style: the rest is CSS text.
    expect(
      checkHtmlLimits(`<style></styles>${"<div>".repeat(500)}`)
    ).toBeNull();
    expect(
      checkHtmlLimits(
        `<svg><foreignObject><p>x</p></foreignObject></svg>`.repeat(1000)
      )
    ).toBeNull();
    expect(
      checkHtmlLimits(`<svg><path></svg>${"<div></div>".repeat(1000)}`)
    ).toBeNull();
  });

  it("is linear on adversarial input", () => {
    const inputs = [
      `<p>${"<b id=x>".repeat(399)}${"</b>".repeat(500_000)}`,
      `${"<span>".repeat(300)}<div>${"</span>".repeat(500_000)}`,
      `<b>${"<caption>".repeat(40_000)}${"</b><b>".repeat(100_000)}`,
      `<table>${"<object>".repeat(40_000)}${"<tr></object>x".repeat(100_000)}`,
      `<table>${"<b id=x>".repeat(399)}${"<td><caption>x".repeat(100_000)}`,
      `<p>${"<b id=x>".repeat(399)}${"<td><tr><caption>".repeat(100_000)}`,
      "<!--".repeat(500_000),
      "</ ".repeat(500_000),
      `<style>${"</sty".repeat(400_000)}`,
      `<p a="${'"'.repeat(1_000_000)}`,
    ];
    for (const html of inputs) {
      const start = performance.now();
      estimateHtml(html, 400, 50_000);
      const elapsed = performance.now() - start;
      expect(elapsed).toBeLessThan(2000);
    }
  });
});
