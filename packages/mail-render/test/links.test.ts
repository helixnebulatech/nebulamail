// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs
// oxlint-disable no-script-url -- these tests feed javascript: URLs to the pipeline on purpose

import { describe, expect, it } from "vitest";

import {
  DEFAULT_PRIVACY,
  analyzeLink,
  cleanLink,
  domainsInText,
  needsConfirmation,
  parseMailto,
  registrableDomain,
  renderEmail,
  resolveLinkForOpening,
} from "../src/index";
import { MAX_CLEANED_LINKS } from "../src/links";

/** What the browser hands us on click: `anchor.href` after HTML and URL parsing. */
const browserResolved = (rawAttribute: string): string => {
  const doc = new DOMParser().parseFromString(
    `<a href="${rawAttribute}">x</a>`,
    "text/html"
  );
  const anchor = doc.querySelector("a");
  return anchor?.href ?? "";
};

const SRCDOC_BASE = "about:srcdoc";

describe(resolveLinkForOpening, () => {
  it.each([
    "javascript:alert(1)",
    "jAvAsCrIpT:alert(1)",
    "java&#x09;script:alert(1)",
    "java&#10;script:alert(1)",
    "java&#13;script:alert(1)",
    "&#106;&#97;&#118;&#97;&#115;&#99;&#114;&#105;&#112;&#116;&#58;alert(1)",
    "&#x6A;avascript&colon;alert(1)",
    "&#x01;javascript:alert(1)",
    "   javascript:alert(1)",
    "\u0000javascript:alert(1)",
    "javascript&#x3A;alert(1)",
    "vbscript:msgbox(1)",
    "data:text/html,<script>alert(1)</script>",
    "file:///etc/passwd",
    "blob:https://mail.example/uuid",
    "about:blank",
    "/relative/path",
    "tel:+34600000000",
  ])("rejects %s", (raw) => {
    // In a srcdoc, relative URLs resolve against the app page (here the
    // jsdom test page), so the app passes its origin.
    const appOrigin = window.location.origin;
    expect(
      resolveLinkForOpening(browserResolved(raw), SRCDOC_BASE, { appOrigin })
    ).toBeNull();
    expect(resolveLinkForOpening(raw, SRCDOC_BASE)).toBeNull();
  });

  it("refuses links into the app itself", () => {
    expect(
      resolveLinkForOpening("https://mail.example/settings", undefined, {
        appOrigin: "https://mail.example",
      })
    ).toBeNull();
  });

  it.each([
    ["https://example.com/a?b=c", "https://example.com/a?b=c", "https:"],
    ["HTTP://EXAMPLE.com", "http://example.com/", "http:"],
    ["  https://example.com/\n", "https://example.com/", "https:"],
    ["mailto:a@example.com", "mailto:a@example.com", "mailto:"],
  ])("accepts %s", (raw, url, protocol) => {
    expect(
      resolveLinkForOpening(browserResolved(raw), SRCDOC_BASE)
    ).toStrictEqual({ protocol, url });
  });

  it("a protocol-relative href is a normal external link once resolved (the render step strips it anyway)", () => {
    expect(
      resolveLinkForOpening(
        "//attacker.example/x",
        "https://mail.example/inbox"
      )?.url
    ).toBe("https://attacker.example/x");
    expect(
      resolveLinkForOpening("//attacker.example/x", SRCDOC_BASE)
    ).toBeNull();
  });

  it("rejects http URLs without a host", () => {
    expect(resolveLinkForOpening("https:")).toBeNull();
  });
});

describe(analyzeLink, () => {
  it.each([
    ["another domain after the real one", "paypal.com evil.example"],
    ["soft hyphen", "paypal\u00AD.com"],
    ["zero-width space", "pay\u200Bpal.com"],
    ["zero-width joiner", "paypal\u200D.com"],
    ["zero-width non-joiner", "paypal\u200C.com"],
    ["combining grapheme joiner", "paypal\u034F.com"],
    ["variation selector", "paypal\uFE0F.com"],
    ["tag characters", "paypal\u{E0041}.com"],
    ["ideographic full stop", "paypal\u3002com"],
    ["fullwidth full stop", "paypal\uFF0Ecom"],
    ["halfwidth ideographic full stop", "paypal\uFF61com"],
  ])("flags text naming another domain hidden with %s", (_name, text) => {
    expect(analyzeLink("https://evil.example/", text)?.warnings).toStrictEqual([
      "text-domain-mismatch",
    ]);
  });

  it("the visible text is read without invisible characters", () => {
    expect(domainsInText("paypal\u200D.com")).toStrictEqual(["paypal.com"]);
    expect(
      analyzeLink("https://www.paypal.com/", "paypal\u3002com")?.warnings
    ).toStrictEqual([]);
  });

  it("warns when any named domain differs (accepted false positive)", () => {
    // `.pdf` looks like a TLD and can't be told apart: `.zip` and `.mov`
    // are real ones. The dialog shows the real host anyway.
    expect(
      analyzeLink("https://example.com/informe", "informe.pdf en example.com")
        ?.warnings
    ).toStrictEqual(["text-domain-mismatch"]);
  });

  it("flags text naming another domain", () => {
    expect(
      analyzeLink("https://evil.example/login", "https://www.paypal.com/login")
        ?.warnings
    ).toStrictEqual(["text-domain-mismatch"]);
    expect(
      analyzeLink("https://www.paypal.com/x", "paypal.com")?.warnings
    ).toStrictEqual([]);
    expect(
      analyzeLink("https://evil.example/x", "Click here")?.warnings
    ).toStrictEqual([]);
  });

  it("flags punycode, userinfo, IPs and plain http", () => {
    expect(analyzeLink("https://xn--pypal-4ve.com/")?.warnings).toContain(
      "punycode"
    );
    expect(analyzeLink("https://pаypal.com/")?.warnings).toContain("punycode");
    expect(analyzeLink("https://paypal.com@evil.example/")?.warnings).toContain(
      "userinfo"
    );
    expect(analyzeLink("https://192.168.0.1/")?.warnings).toContain(
      "ip-address"
    );
    expect(analyzeLink("http://[::1]/")?.warnings).toStrictEqual([
      "ip-address",
      "insecure",
    ]);
  });

  it("compares Unicode text with the punycode target", () => {
    expect(domainsInText("visit bücher.de today")).toStrictEqual([
      "xn--bcher-kva.de",
    ]);
    expect(
      analyzeLink("https://xn--bcher-kva.de/", "bücher.de")?.warnings
    ).toStrictEqual(["punycode"]);
  });

  it("knows common second-level suffixes", () => {
    expect(registrableDomain("smile.amazon.co.uk")).toBe("amazon.co.uk");
    expect(registrableDomain("a.b.example.com")).toBe("example.com");
    expect(registrableDomain("localhost")).toBe("localhost");
    expect(registrableDomain("10.0.0.1")).toBe("10.0.0.1");
  });

  it("decides when to confirm", () => {
    const clean = analyzeLink("https://example.com/");
    const shady = analyzeLink("https://evil.example/", "bank.com");
    expect(needsConfirmation("always", clean)).toBeTruthy();
    expect(needsConfirmation("suspicious", clean)).toBeFalsy();
    expect(needsConfirmation("suspicious", shady)).toBeTruthy();
    expect(needsConfirmation("never", shady)).toBeTruthy();
    expect(
      needsConfirmation("never", analyzeLink("http://example.com/"))
    ).toBeFalsy();
    expect(needsConfirmation("never", null)).toBeTruthy();
  });
});

describe(parseMailto, () => {
  it("keeps only to/cc/bcc/subject/body", () => {
    expect(
      parseMailto(
        "mailto:a@x.example,b@x.example?cc=c@x.example&subject=Hi%20there&body=1+1&attach=/etc/passwd&attachment=C:\\x"
      )
    ).toStrictEqual({
      bcc: [],
      body: "1+1",
      cc: ["c@x.example"],
      subject: "Hi there",
      to: ["a@x.example", "b@x.example"],
    });
    expect(parseMailto("https://x.example")).toBeNull();
  });
});

describe("links in the document", () => {
  it(`cleans at most ${MAX_CLEANED_LINKS} links per email`, () => {
    const html = '<a href="https://x.test/?utm_source=a&amp;n=1">x</a>'.repeat(
      2500
    );
    const result = renderEmail({ html }, DEFAULT_PRIVACY);
    expect(result.report.linksCleaned).toHaveLength(MAX_CLEANED_LINKS);
    // The rest keep their (safe) href.
    expect(result.srcdoc.split("utm_source=a")).toHaveLength(
      2500 - MAX_CLEANED_LINKS + 1
    );
  }, 60_000);

  it("removes target, sets rel, cleans tracking parameters and reports it", () => {
    const html = `<a href="https://shop.example/p?utm_source=nl&amp;utm_medium=mail&amp;id=4" target="_blank">p</a>`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY);
    expect(result.srcdoc).toContain(
      '<a href="https://shop.example/p?id=4" rel="noopener noreferrer">p</a>'
    );
    expect(result.report.linksCleaned).toStrictEqual([
      {
        cleaned: "https://shop.example/p?id=4",
        original: "https://shop.example/p?utm_source=nl&utm_medium=mail&id=4",
      },
    ]);
  });

  it("does not clean links when the setting is off", () => {
    const html = `<a href="https://shop.example/p?utm_source=nl">p</a>`;
    const result = renderEmail(
      { html },
      { ...DEFAULT_PRIVACY, cleanLinks: false }
    );
    expect(result.srcdoc).toContain("utm_source=nl");
    expect(result.report.linksCleaned).toStrictEqual([]);
  });

  it("keeps simple fragments, drops relative and non-http hrefs", () => {
    const html = `<a href="#top">a</a><a href="/x">b</a><a href="tel:1">c</a><a href="data:text/html,x">d</a>`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY);
    expect(result.srcdoc).toContain(
      '<a href="#top" rel="noopener noreferrer">a</a>'
    );
    expect(result.srcdoc).toContain('<a rel="noopener noreferrer">b</a>');
    expect(result.report.unsafeLinksRemoved).toBe(3);
  });

  it("cleanLink never turns a link into a non-http URL", () => {
    expect(cleanLink("https://example.com/?fbclid=1&a=2")).toBe(
      "https://example.com/?a=2"
    );
    expect(cleanLink("mailto:a@example.com")).toBe("mailto:a@example.com");
    expect(cleanLink("not a url")).toBe("not a url");
  });
});
