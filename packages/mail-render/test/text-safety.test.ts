// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable no-script-url -- these tests feed javascript: URLs to the pipeline on purpose

import { describe, expect, it } from "vitest";

import {
  DEFAULT_PRIVACY,
  analyzeAttachment,
  analyzeDisplayText,
  checkHtmlLimits,
  cleanDisplayText,
  estimateNestingDepth,
  hasBidiControls,
  isPreviewableImage,
  linkifySegments,
  renderEmail,
  stripBidiControls,
} from "../src/index";
import { roughHtmlToText } from "../src/limits";
import { MAX_TEXT_LINKS } from "../src/text";

describe("unicode", () => {
  it("strips bidi controls: factura\\u202Efdp.exe", () => {
    const name = "factura\u202Efdp.exe";
    expect(hasBidiControls(name)).toBeTruthy();
    expect(stripBidiControls(name)).toBe("facturafdp.exe");
  });

  it.each([
    "\u202A",
    "\u202B",
    "\u202C",
    "\u202D",
    "\u202E",
    "\u2066",
    "\u2067",
    "\u2068",
    "\u2069",
    "\u200E",
    "\u200F",
    "\u061C",
  ])("removes U+%s", (ch) => {
    expect(cleanDisplayText(`a${ch}b`)).toBe("ab");
  });

  it("removes invisible and control characters, keeps ZWJ emoji", () => {
    expect(cleanDisplayText("Pay\u200BPal\u00AD \uFEFFSupport\u0007")).toBe(
      "PayPal Support"
    );
    expect(cleanDisplayText("line1\nline2\r\n")).toBe("line1 line2");
    const family = "\u{1F468}‍\u{1F469}‍\u{1F467}";
    expect(cleanDisplayText(family)).toBe(family);
  });

  it("reports alterations", () => {
    expect(analyzeDisplayText("Hola")).toStrictEqual({
      altered: false,
      hadBidiControls: false,
      text: "Hola",
    });
    expect(analyzeDisplayText("Ho\u202Ela").altered).toBeTruthy();
  });
});

describe("attachments", () => {
  it("does not read extensions from the object prototype", () => {
    for (const name of ["factura.constructor", "a.__proto__", "a.toString"]) {
      expect(
        analyzeAttachment({ name, type: "text/plain" }).warnings
      ).toStrictEqual([]);
    }
  });

  it.each([
    ["evil.exe.", "evil.exe", ["dangerous-extension"]],
    ["evil.exe. .", "evil.exe", ["dangerous-extension"]],
    [
      "factura.pdf.exe. .",
      "factura.pdf.exe",
      ["dangerous-extension", "double-extension"],
    ],
    [
      "factura.pdf      .exe  ",
      "factura.pdf      .exe",
      ["dangerous-extension", "double-extension"],
    ],
  ])(
    "trailing dots and spaces do not hide the extension: %j",
    (name, fileName, warnings) => {
      expect(analyzeAttachment({ name })).toMatchObject({
        extension: "exe",
        fileName,
        warnings,
      });
    }
  );

  it("a name of only dots and spaces leaves no file name", () => {
    expect(analyzeAttachment({ name: "..." }).fileName).toBe("");
    expect(analyzeAttachment({ name: " . " }).fileName).toBe("");
  });

  it("is linear on long runs of spaces", () => {
    const start = performance.now();
    analyzeAttachment({ name: `a${" ".repeat(1_000_000)}b` });
    analyzeAttachment({ name: `a${" ".repeat(1_000_000)}.exe` });
    analyzeAttachment({ name: `a${". ".repeat(500_000)}b.` });
    expect(performance.now() - start).toBeLessThan(1000);
    expect(
      analyzeAttachment({ name: `a${" ".repeat(10_000)}.exe` }).warnings
    ).toContain("double-extension");
  });

  it.each([
    [
      { name: "factura\u202Efdp.exe", type: "application/pdf" },
      ["bidi-in-name", "dangerous-extension"],
    ],
    [
      { name: "invoice.pdf.exe", type: "application/octet-stream" },
      ["dangerous-extension", "double-extension"],
    ],
    [{ name: "photo.jpg", type: "text/html" }, ["mime-mismatch", "no-preview"]],
    [
      { name: "report.pdf          .scr", type: "application/pdf" },
      ["dangerous-extension", "double-extension"],
    ],
    [
      { name: "drawing.svg", type: "image/svg+xml" },
      ["dangerous-extension", "no-preview"],
    ],
    [
      { name: "page.HTML", type: "text/html" },
      ["dangerous-extension", "no-preview"],
    ],
    [{ name: "archive.tar.gz", type: "application/gzip" }, []],
    [{ name: "photo.jpg", type: "image/jpeg" }, []],
    [{ name: "photo.jpg", type: "application/octet-stream" }, []],
    [{ name: "README", type: null }, []],
  ])("%o → %o", (input, warnings) => {
    expect(analyzeAttachment(input).warnings).toStrictEqual(warnings);
  });

  it("shows the real name and extension", () => {
    expect(analyzeAttachment({ name: "factura\u202Efdp.exe" })).toMatchObject({
      displayName: "facturafdp.exe",
      extension: "exe",
    });
  });

  it("previews only plain raster images", () => {
    expect(
      isPreviewableImage({ name: "a.png", type: "image/png" })
    ).toBeTruthy();
    expect(
      isPreviewableImage({ name: "a.svg", type: "image/svg+xml" })
    ).toBeFalsy();
    expect(
      isPreviewableImage({ name: "a.png", type: "image/svg+xml" })
    ).toBeFalsy();
    expect(
      isPreviewableImage({ name: "a.jpg", type: "image/png" })
    ).toBeFalsy();
  });
});

describe("limits", () => {
  it("estimates depth, ignoring optional-end-tag elements and comments", () => {
    expect(estimateNestingDepth("<div><div><span>x</span></div></div>")).toBe(
      3
    );
    expect(estimateNestingDepth("<p>a<p>b<p>c<li>d<td>e")).toBe(0);
    expect(
      estimateNestingDepth("<!-- <div><div> --><div title='<div>'></div>")
    ).toBe(1);
    expect(estimateNestingDepth("<style><div><div></style><b>x</b>")).toBe(1);
    // HTML ignores `/>` on elements that are not void: `<div/>` stays open.
    expect(estimateNestingDepth("<br/><img><div/>")).toBe(1);
  });

  it("is linear on adversarial input", () => {
    const start = performance.now();
    estimateNestingDepth("<a".repeat(500_000));
    estimateNestingDepth('<a "'.repeat(300_000));
    roughHtmlToText("<a<a<".repeat(300_000));
    roughHtmlToText(" ".repeat(1_000_000));
    linkifySegments("a+".repeat(500_000));
    linkifySegments(`${".".repeat(500_000)}x`);
    expect(performance.now() - start).toBeLessThan(3000);
  });

  it("falls back to text when the HTML is too large", () => {
    const html = `<p>${"x".repeat(100)}</p>`;
    expect(
      checkHtmlLimits(html, {
        maxElements: 10,
        maxHtmlLength: 50,
        maxNestingDepth: 10,
        maxTextLength: 100,
      })
    ).toBe("too-large");
    const result = renderEmail({ html, text: "plain part" }, DEFAULT_PRIVACY, {
      limits: { maxHtmlLength: 50 },
    });
    expect(result).toMatchObject({ fallbackReason: "too-large", view: "text" });
    expect(result.srcdoc).toContain("plain part");
  });

  it("falls back to text when the HTML is too deep", () => {
    const html = `${"<div>".repeat(500)}deep${"</div>".repeat(500)}`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY);
    expect(result).toMatchObject({ fallbackReason: "too-deep", view: "text" });
    expect(result.srcdoc).toContain("deep");
  });

  it("falls back to text when there are too many elements", () => {
    const html = "<b>x</b>".repeat(200);
    const result = renderEmail({ html }, DEFAULT_PRIVACY, {
      limits: { maxElements: 100 },
    });
    expect(result).toMatchObject({
      fallbackReason: "too-many-elements",
      view: "text",
    });
  });

  it("rough text skips styles after characters whose lower case is longer", () => {
    expect(roughHtmlToText("İİİ<STYLE>a{}</Style>hola")).toBe("İİİhola");
  });

  it("rough text drops tags, scripts and styles", () => {
    expect(
      roughHtmlToText(
        "<style>a{}</style><p>Hola &amp; <b>adiós</b></p><script>x</script>fin"
      )
    ).toBe("Hola & adiós\nfin");
  });
});

describe("plain text view", () => {
  it(`makes at most ${MAX_TEXT_LINKS} links, quickly`, () => {
    const start = performance.now();
    const result = renderEmail(
      { text: "www.a.b/?a ".repeat(150_000) },
      DEFAULT_PRIVACY,
      { view: "text" }
    );
    const elapsed = performance.now() - start;
    expect(result.srcdoc.split("<a ")).toHaveLength(MAX_TEXT_LINKS + 1);
    // About 1 s; tidy-url on 150 000 links took minutes.
    expect(elapsed).toBeLessThan(10_000);
  }, 30_000);

  it("escapes everything and linkifies through DOM nodes", () => {
    const text =
      "Hi <script>alert(1)</script> <img src=x onerror=alert(1)>\nSee (https://example.com/a?utm_source=x). Mail me@example.com, www.example.org.";
    const result = renderEmail(
      { html: "<p>ignored</p>", text },
      DEFAULT_PRIVACY,
      { view: "text" }
    );
    const doc = new DOMParser().parseFromString(result.srcdoc, "text/html");
    expect(doc.querySelectorAll("script, img")).toHaveLength(0);
    expect(doc.body.textContent).toContain("<script>alert(1)</script>");
    expect(
      [...doc.querySelectorAll("a")].map((a) => [
        a.getAttribute("href"),
        a.textContent,
      ])
    ).toStrictEqual([
      ["https://example.com/a", "https://example.com/a?utm_source=x"],
      ["mailto:me@example.com", "me@example.com"],
      ["https://www.example.org/", "www.example.org"],
    ]);
    expect(result.report.linksCleaned).toHaveLength(1);
  });

  it("is used when there is no HTML part", () => {
    expect(renderEmail({ text: "only text" }, DEFAULT_PRIVACY)).toMatchObject({
      fallbackReason: "no-html",
      view: "text",
    });
  });

  it("does not linkify javascript: or bare words", () => {
    expect(linkifySegments("javascript:alert(1) foo.bar")).toStrictEqual([
      { text: "javascript:alert(1) foo.bar" },
    ]);
  });
});
