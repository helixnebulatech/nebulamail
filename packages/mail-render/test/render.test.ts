// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/max-expects -- each test checks one behaviour over a table of related inputs
// oxlint-disable no-script-url -- these tests feed javascript: URLs to the pipeline on purpose

import { describe, expect, it, vi } from "vitest";

import {
  DEFAULT_PRIVACY,
  IFRAME_SANDBOX,
  InvalidImageSourceError,
  InvalidStyleNonceError,
  PRINT_IFRAME_SANDBOX,
  attachFrame,
  buildContentSecurityPolicy,
  fillFramePage,
  findLinkFromEvent,
  renderEmail,
  shouldLoadRemoteImages,
  validateProxyOrigin,
} from "../src/index";
import type { ImageSource } from "../src/index";

const PROXY = "https://img.test";
const proxy: ImageSource = {
  kind: "proxy",
  origin: PROXY,
  rewrite: (url) => `${PROXY}/t/${encodeURIComponent(url)}`,
};
const IMG = `<img src="https://cdn.example/a.png" width="100" height="100">`;
const cspFor = (images: ImageSource): string =>
  renderEmail({ html: "<p>x</p>" }, DEFAULT_PRIVACY, { images }).csp;
const loads = (
  settings: typeof DEFAULT_PRIVACY,
  context: { userApproved?: boolean; senderTrusted?: boolean }
): boolean =>
  renderEmail({ html: IMG }, settings, { images: proxy, ...context })
    .remoteImagesLoaded;
const imgTag = (url: string): string =>
  `<img src="${url}" width="9" height="9">`;
const parse = (srcdoc: string): Document =>
  new DOMParser().parseFromString(srcdoc, "text/html");

describe("iframe document", () => {
  it("puts the CSP meta first in head, before anything from the email", () => {
    const html = `<html><head><style>p{color:red}</style><meta http-equiv="Content-Security-Policy" content="default-src *"></head><body><p>x</p></body></html>`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY, { images: proxy });
    expect(
      result.srcdoc.startsWith(
        `<!DOCTYPE html><html><head><meta http-equiv="Content-Security-Policy" content="${result.csp}">`
      )
    ).toBeTruthy();
    const doc = parse(result.srcdoc);
    expect(doc.head.firstElementChild?.getAttribute("content")).toBe(
      result.csp
    );
    expect(
      [...doc.querySelectorAll("meta")].map((m) => m.getAttribute("content"))
    ).toStrictEqual([result.csp, "no-referrer", "off"]);
  });

  it("builds the CSP for each image source", () => {
    expect(cspFor({ kind: "none" })).toBe(
      "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; form-action 'none'; base-uri 'none'"
    );
    expect(cspFor(proxy)).toContain("img-src data: https://img.test;");
    expect(cspFor({ kind: "batch", rewrite: () => null })).toContain(
      "img-src data: blob:;"
    );
    expect(buildContentSecurityPolicy(["data:", "https:"])).toContain(
      "img-src data: https:;"
    );
  });

  it("validates the proxy origin", () => {
    expect(validateProxyOrigin("https://img.example.com")).toBe(
      "https://img.example.com"
    );
    expect(validateProxyOrigin("https://img.example.com/")).toBe(
      "https://img.example.com"
    );
    expect(validateProxyOrigin("http://localhost:3001")).toBe(
      "http://localhost:3001"
    );
    // RFC 6761: *.localhost is loopback; the dev proxy runs on img.localhost
    // so it doesn't receive the app's cookies.
    expect(validateProxyOrigin("http://img.localhost:3003")).toBe(
      "http://img.localhost:3003"
    );
    for (const bad of [
      "http://img.example.com",
      "http://localhost.evil.com",
      "http://img.localhost.evil.com",
      "https://img.example.com/path",
      "https://u:p@img.example.com",
      "img.example.com",
      "https://x; script-src *",
      "javascript:alert(1)",
    ]) {
      expect(() => validateProxyOrigin(bad)).toThrow(InvalidImageSourceError);
    }
    expect(() =>
      renderEmail({ html: "<p>x</p>" }, DEFAULT_PRIVACY, {
        images: { ...proxy, origin: "http://evil" },
      })
    ).toThrow(InvalidImageSourceError);
  });

  it("exports the exact sandbox value", () => {
    expect(IFRAME_SANDBOX).toBe("allow-same-origin");
  });
});

describe("text formatting of received emails", () => {
  it("keeps font-family, font-size and dir (what our editor and others write)", () => {
    const html = `<div style="font-family: Arial, sans-serif; font-size: 14px"><p dir="rtl" style="font-family: 'Times New Roman', serif; font-size: 18px">שלום</p><p style="font-family: Georgia, serif">b</p></div>`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY, { images: proxy });
    const doc = parse(result.srcdoc);
    const [first, second] = doc.querySelectorAll("p");
    expect(first?.getAttribute("dir")).toBe("rtl");
    expect(first?.style.fontFamily).toContain("Times New Roman");
    expect(first?.style.fontSize).toBe("18px");
    expect(second?.style.fontFamily).toContain("Georgia");
    expect(doc.querySelector("div")?.style.fontSize).toBe("14px");
  });

  it("never loads a font, however it is written", () => {
    const html = `<style>@font-face{font-family:Arial;src:url(https://attacker.example/f.woff)}</style><p style="font-family: Arial, url(https://attacker.example/g.woff)">x</p>`;
    const { srcdoc } = renderEmail({ html }, DEFAULT_PRIVACY, {
      images: proxy,
    });
    expect(srcdoc).not.toContain("attacker.example");
  });
});

describe("style nonce", () => {
  const html = `<html><head><style>p{color:red}</style></head><body><style>b{color:blue}</style><p nonce="evil" style="color:green">x</p></body></html>`;
  const NONCE = "AbCdEf0123456789_-+/=";

  it("is set on every <style> (ours and the email's) when given", () => {
    for (const view of ["html", "text"] as const) {
      const doc = parse(
        renderEmail({ html, text: "t" }, DEFAULT_PRIVACY, {
          styleNonce: NONCE,
          view,
        }).srcdoc
      );
      const styles = [...doc.querySelectorAll("style")];
      expect(styles.length).toBeGreaterThanOrEqual(view === "html" ? 3 : 1);
      for (const style of styles) {
        expect(style.getAttribute("nonce")).toBe(NONCE);
      }
      // Only <style> elements get it; the email cannot bring its own.
      expect(doc.querySelectorAll("[nonce]:not(style)")).toHaveLength(0);
    }
  });

  it("is absent when not given", () => {
    const doc = parse(renderEmail({ html }, DEFAULT_PRIVACY).srcdoc);
    expect(doc.querySelectorAll("[nonce]")).toHaveLength(0);
  });

  it.each([
    "short",
    "x".repeat(65),
    'abcdefghijklmnop"><script>',
    "abcdefghijklmnop qr",
    "ñandúñandúñandúñandú",
  ])("rejects %s", (nonce) => {
    expect(() =>
      renderEmail({ html }, DEFAULT_PRIVACY, { styleNonce: nonce })
    ).toThrow(InvalidStyleNonceError);
  });
});

describe("remote images", () => {
  it("are off by default and listed for the BFF", () => {
    const result = renderEmail({ html: IMG }, DEFAULT_PRIVACY, {
      images: proxy,
    });
    expect(result.remoteImages).toStrictEqual(["https://cdn.example/a.png"]);
    expect(result.remoteImagesLoaded).toBeFalsy();
    expect(
      parse(result.srcdoc).querySelector("img")?.hasAttribute("src")
    ).toBeFalsy();
    expect(result.report.remoteImagesBlocked).toBe(1);
  });

  it("load only through the rewriter, and only URLs on the proxy origin", () => {
    const loaded = renderEmail({ html: IMG }, DEFAULT_PRIVACY, {
      images: proxy,
      userApproved: true,
    });
    expect(parse(loaded.srcdoc).querySelector("img")?.getAttribute("src")).toBe(
      `${PROXY}/t/https%3A%2F%2Fcdn.example%2Fa.png`
    );
    for (const evil of [
      "https://cdn.example/a.png",
      "https://img.test.evil.example/x",
      "javascript:alert(1)",
      "//img.test/x",
      "data:image/svg+xml,<svg/>",
    ]) {
      const result = renderEmail({ html: IMG }, DEFAULT_PRIVACY, {
        images: { ...proxy, rewrite: () => evil },
        userApproved: true,
      });
      expect(
        parse(result.srcdoc).querySelector("img")?.hasAttribute("src")
      ).toBeFalsy();
      expect(result.report.remoteImagesBlocked).toBe(1);
    }
  });

  it("follow the setting and trusted senders", () => {
    expect(loads(DEFAULT_PRIVACY, {})).toBeFalsy();
    expect(loads(DEFAULT_PRIVACY, { senderTrusted: true })).toBeTruthy();
    expect(
      loads({ ...DEFAULT_PRIVACY, remoteImages: "always" }, {})
    ).toBeTruthy();
    expect(
      loads(
        { ...DEFAULT_PRIVACY, remoteImages: "never" },
        { senderTrusted: true, userApproved: true }
      )
    ).toBeFalsy();
    expect(
      shouldLoadRemoteImages(
        { remoteImages: "trusted-senders" },
        { senderTrusted: true }
      )
    ).toBeTruthy();
  });

  it("never load without an image source, and direct mode needs the proxy turned off", () => {
    expect(
      renderEmail({ html: IMG }, DEFAULT_PRIVACY, { userApproved: true })
        .remoteImagesLoaded
    ).toBeFalsy();
    const direct = renderEmail({ html: IMG }, DEFAULT_PRIVACY, {
      images: { kind: "direct" },
      userApproved: true,
    });
    expect(direct.remoteImagesLoaded).toBeFalsy();
    const off = renderEmail(
      {
        html: `${IMG}<img src="http://cdn.example/b.png" width="9" height="9">`,
      },
      { ...DEFAULT_PRIVACY, loadThroughProxy: false },
      {
        images: { kind: "direct" },
        userApproved: true,
      }
    );
    const srcs = [...parse(off.srcdoc).querySelectorAll("img")].map((img) =>
      img.getAttribute("src")
    );
    expect(srcs).toStrictEqual(["https://cdn.example/a.png", null]);
    expect(off.csp).toContain("img-src data: https:;");
  });

  it("direct mode never points the browser at the local network", () => {
    // Built, not written: a public IPv4 literal (Google's resolver).
    const PUBLIC_V4 = [8, 8, 4, 4].join(".");
    const blocked = [
      "https://192.168.1.1/a.png",
      "https://10.0.0.1/a.png",
      "https://127.0.0.1/a.png",
      "https://0x7f.1/a.png",
      "https://2130706433/a.png",
      "https://[::1]/a.png",
      "https://[::ffff:192.168.1.1]/a.png",
      "https://[fd00::1]/a.png",
      "https://169.254.169.254/a.png",
      "https://100.64.0.1/a.png",
      "https://localhost/a.png",
      "https://app.localhost/a.png",
      "https://nas.local/a.png",
      "https://router.lan/a.png",
      "https://grafana.internal/a.png",
      "https://intranet/a.png",
      "https://router.home.arpa/a.png",
      "https://cdn.example:8443/a.png",
      "https://user@cdn.example/a.png",
    ];
    const allowed = [
      "https://cdn.example/a.png",
      `https://${PUBLIC_V4}/a.png`,
      "https://[2606:4700::1]/a.png",
      "https://cdn.example:443/a.png",
    ];
    const html = [
      ...[...blocked, ...allowed].map(imgTag),
      `<div style="background:url(https://192.168.1.1/b.png)">x</div>`,
      `<img srcset="https://10.0.0.1/c.png 1x" width="9" height="9">`,
    ].join("");
    const result = renderEmail(
      { html },
      { ...DEFAULT_PRIVACY, loadThroughProxy: false },
      { images: { kind: "direct" }, userApproved: true }
    );
    const srcs = [...parse(result.srcdoc).querySelectorAll("img")]
      .map((img) => img.getAttribute("src"))
      .filter((src) => src !== null);
    expect(srcs).toStrictEqual([
      "https://cdn.example/a.png",
      `https://${PUBLIC_V4}/a.png`,
      "https://[2606:4700::1]/a.png",
      "https://cdn.example/a.png",
    ]);
    for (const host of ["192.168.1.1", "10.0.0.1", "127.0.0.1", "localhost"]) {
      expect(result.srcdoc).not.toContain(`https://${host}`);
    }
  });

  it("batch mode accepts only blob: URLs", () => {
    const result = renderEmail({ html: IMG }, DEFAULT_PRIVACY, {
      images: {
        kind: "batch",
        rewrite: () => "blob:https://mail.example/1234",
      },
      userApproved: true,
    });
    expect(parse(result.srcdoc).querySelector("img")?.getAttribute("src")).toBe(
      "blob:https://mail.example/1234"
    );
  });

  it("removes trackers even with images on, unless the setting is off", () => {
    const html = `<img src="https://github.com/notifications/beacon/x.gif" width="50" height="50"><img src="https://px.example/p.gif" width="1" height="1">`;
    const on = renderEmail({ html }, DEFAULT_PRIVACY, {
      images: proxy,
      userApproved: true,
    });
    expect(parse(on.srcdoc).querySelectorAll("img")).toHaveLength(0);
    expect(on.report.trackersBlocked).toBe(2);
    expect(on.remoteImages).toStrictEqual([]);
    const listOff = renderEmail(
      { html },
      { ...DEFAULT_PRIVACY, mailTrackerBlocker: false },
      { images: proxy }
    );
    expect(listOff.report.trackers.map((t) => t.service)).toStrictEqual([
      "Spy pixel",
    ]);
    const pixelsOff = renderEmail(
      { html },
      { ...DEFAULT_PRIVACY, spyPixelRules: false },
      { images: proxy }
    );
    expect(pixelsOff.report.trackers.map((t) => t.service)).toStrictEqual([
      "GitHub",
    ]);
    const allOff = renderEmail(
      { html },
      { ...DEFAULT_PRIVACY, removeTrackers: false },
      { images: proxy, userApproved: true }
    );
    expect(allOff.report.trackersBlocked).toBe(0);
    expect(parse(allOff.srcdoc).querySelectorAll("img[src]")).toHaveLength(2);
  });

  it("honours custom tracker hosts", () => {
    const result = renderEmail(
      { html: IMG },
      { ...DEFAULT_PRIVACY, blockedTrackerHosts: ["cdn.example"] },
      { images: proxy }
    );
    expect(result.report.trackers).toStrictEqual([
      { count: 1, service: "Custom list", urls: ["https://cdn.example/a.png"] },
    ]);
  });
});

describe("inline images", () => {
  it("shows data: rasters always, never SVG", () => {
    const html = `<img src="data:image/png;base64,AAAA"><img src="data:image/svg+xml,%3Csvg/%3E"><div style="background:url(data:image/gif;base64,R0lG)">x</div>`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY);
    const imgs = [...parse(result.srcdoc).querySelectorAll("img")].map((img) =>
      img.getAttribute("src")
    );
    expect(imgs).toStrictEqual(["data:image/png;base64,AAAA", null]);
    expect(
      parse(result.srcdoc).querySelector("div")?.getAttribute("style")
    ).toBe('background:url("data:image/gif;base64,R0lG")');
    expect(result.report.unsafeImagesRemoved).toStrictEqual({ svg: 1 });
  });

  it("resolves cid: through the caller's map and lists the needed ones", () => {
    const html = `<img src="cid:Logo@Mail"><img src="cid:%3Cother@mail%3E"><img src="cid:missing">`;
    const result = renderEmail({ html }, DEFAULT_PRIVACY, {
      cids: (cid) =>
        ({
          "logo@mail": "data:image/png;base64,AAAA",
          "other@mail": "https://evil.example/x.png",
        })[cid],
    });
    expect(result.neededCids).toStrictEqual([
      "logo@mail",
      "other@mail",
      "missing",
    ]);
    expect(
      [...parse(result.srcdoc).querySelectorAll("img")].map((img) =>
        img.getAttribute("src")
      )
    ).toStrictEqual(["data:image/png;base64,AAAA", null, null]);
  });
});

describe("integrity check", () => {
  it("never returns HTML that fails the re-parse check", () => {
    // Everything here is handled before the check; the check itself is
    // exercised by the corpus. This asserts the success path is clean.
    const result = renderEmail({ html: "<p>ok</p>" }, DEFAULT_PRIVACY);
    expect(result.problems).toBeUndefined();
    expect(result.view).toBe("html");
  });

  it("rewrites <style> URLs after İ (lower case changes the length)", () => {
    const result = renderEmail(
      {
        html: '<style>.İİİ{background:url(https://cdn.example/a.png)}</style><p class="İİİ">x</p>',
      },
      DEFAULT_PRIVACY,
      { images: proxy, userApproved: true }
    );
    expect(result.problems).toBeUndefined();
    expect(result.view).toBe("html");
    expect(result.srcdoc).not.toContain("proton-url");
    expect(result.srcdoc).toContain(
      `url("${PROXY}/t/${encodeURIComponent("https://cdn.example/a.png")}")`
    );
  });
});

describe("hostile input never throws", () => {
  it("falls back to text when the sanitizer leaves no body (frameset)", () => {
    for (const html of [
      "<frameset></frameset>",
      "<frameset><frame src=x></frameset><noframes>hola</noframes>",
    ]) {
      const result = renderEmail({ html, text: "hola" }, DEFAULT_PRIVACY);
      expect(result.view).toBe("text");
      expect(result.fallbackReason).toBe("integrity-check-failed");
      expect(result.problems).toStrictEqual(["no-body"]);
    }
  });

  it("falls back to text when the pipeline throws", async () => {
    const proton = await import("../src/proton");
    const spy = vi.spyOn(proton, "protonize").mockImplementation(() => {
      throw new TypeError("element.tagName.toUpperCase is not a function");
    });
    try {
      const result = renderEmail(
        { html: "<form><input name=tagName></form>", text: "hola" },
        DEFAULT_PRIVACY
      );
      expect(result.view).toBe("text");
      expect(result.fallbackReason).toBe("integrity-check-failed");
      expect(result.problems).toStrictEqual(["render-error"]);
      expect(parse(result.srcdoc).body.textContent).toContain("hola");
    } finally {
      spy.mockRestore();
    }
  });

  it("still throws configuration errors", async () => {
    const proton = await import("../src/proton");
    const spy = vi.spyOn(proton, "protonize").mockImplementation(() => {
      throw new proton.SanitizerUnavailableError();
    });
    try {
      expect(() => renderEmail({ html: "<p>x</p>" }, DEFAULT_PRIVACY)).toThrow(
        proton.SanitizerUnavailableError
      );
    } finally {
      spy.mockRestore();
    }
  });
});

describe("frame helpers", () => {
  it("sandboxes: never scripts; printing only adds modals", () => {
    expect(IFRAME_SANDBOX).toBe("allow-same-origin");
    expect(PRINT_IFRAME_SANDBOX.split(" ").toSorted()).toStrictEqual([
      "allow-modals",
      "allow-same-origin",
    ]);
  });

  it("findLinkFromEvent returns the browser-resolved href, also from text nodes", () => {
    document.body.innerHTML = `<a href="https://example.com/a b"><span>go <b>now</b></span></a><area href="https://example.com/m" alt="map"><p>none</p>`;
    const bold = document.querySelector("b");
    const text = bold?.firstChild;
    const event = new MouseEvent("click", { bubbles: true });
    Object.defineProperty(event, "target", { value: text });
    expect(findLinkFromEvent(event)).toMatchObject({
      href: "https://example.com/a%20b",
      text: "go now",
    });
    const areaEvent = new MouseEvent("click");
    Object.defineProperty(areaEvent, "target", {
      value: document.querySelector("area"),
    });
    expect(findLinkFromEvent(areaEvent)).toMatchObject({
      href: "https://example.com/m",
      text: "map",
    });
    const none = new MouseEvent("click");
    Object.defineProperty(none, "target", {
      value: document.querySelector("p"),
    });
    expect(findLinkFromEvent(none)).toBeNull();
  });

  it("attachFrame captures link clicks and prevents navigation", async () => {
    const iframe = document.createElement("iframe");
    iframe.setAttribute("sandbox", IFRAME_SANDBOX);
    const clicked: string[] = [];
    const heights: number[] = [];
    const detach = attachFrame(iframe, {
      onHeight: (h) => {
        heights.push(h);
      },
      onLink: (link) => {
        clicked.push(link.href);
      },
    });
    let loaded = false;
    iframe.addEventListener("load", () => {
      loaded = true;
    });
    // jsdom ignores srcdoc (real browsers: Playwright TODO), so fill the
    // loaded document directly.
    iframe.srcdoc = renderEmail(
      { html: `<a href="https://example.com/x">x</a>` },
      DEFAULT_PRIVACY
    ).srcdoc;
    document.body.append(iframe);
    await vi.waitFor(() => {
      expect(loaded).toBeTruthy();
    });
    if (iframe.contentDocument !== null) {
      iframe.contentDocument.body.innerHTML = `<a href="https://example.com/x">x</a><p>text</p>`;
    }
    const anchor = iframe.contentDocument?.querySelector("a");
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor?.dispatchEvent(event);
    expect(event.defaultPrevented).toBeTruthy();
    expect(clicked).toStrictEqual(["https://example.com/x"]);
    const plain = new MouseEvent("click", { bubbles: true, cancelable: true });
    iframe.contentDocument?.querySelector("p")?.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBeFalsy();
    const middle = new MouseEvent("auxclick", {
      bubbles: true,
      button: 1,
      cancelable: true,
    });
    anchor?.dispatchEvent(middle);
    expect(middle.defaultPrevented).toBeTruthy();
    expect(heights.length).toBeGreaterThan(0);
    detach();
    iframe.remove();
  });
});

describe(fillFramePage, () => {
  it("moves a rendered email into a frame page, its own CSP first", () => {
    const iframe = document.createElement("iframe");
    document.body.append(iframe);
    const result = renderEmail(
      { html: `<h1>Hola</h1>${IMG}` },
      { ...DEFAULT_PRIVACY, loadThroughProxy: false },
      { images: { kind: "direct" }, userApproved: true }
    );
    expect(fillFramePage(iframe, result.frame)).toBeTruthy();
    const doc = iframe.contentDocument;
    expect(doc?.head.firstElementChild?.getAttribute("content")).toBe(
      result.csp
    );
    expect(result.csp).toContain("img-src data: https:");
    expect(doc?.querySelector("h1")?.textContent).toBe("Hola");
    expect(doc?.querySelector("img")?.getAttribute("src")).toBe(
      "https://cdn.example/a.png"
    );
    iframe.remove();
  });

  it("only takes documents made by renderEmail", () => {
    const iframe = document.createElement("iframe");
    document.body.append(iframe);
    expect(() => {
      // A look-alike object, as untyped code could pass.
      Reflect.apply(fillFramePage, undefined, [
        iframe,
        { html: "<!DOCTYPE html><html><body>x</body></html>" },
      ]);
    }).toThrow("renderEmail");
    iframe.remove();
  });
});

describe("plain-text colours (textScheme)", () => {
  it("follows the app theme for text, never for HTML", () => {
    const text = { text: "Hola." };
    const dark = renderEmail(text, DEFAULT_PRIVACY, { textScheme: "dark" });
    expect(dark.view).toBe("text");
    expect(dark.srcdoc).toContain(":root{color-scheme:dark}");
    const light = renderEmail(text, DEFAULT_PRIVACY);
    expect(light.srcdoc).not.toContain("color-scheme:dark");
    expect(light.srcdoc).toContain("html,body{background:transparent}");
    // An HTML email keeps its light document whatever the theme.
    const html = renderEmail({ html: "<p>Hola.</p>" }, DEFAULT_PRIVACY, {
      textScheme: "dark",
    });
    expect(html.view).toBe("html");
    expect(html.srcdoc).not.toContain("color-scheme:dark");
    expect(html.srcdoc).not.toContain("html,body{background:transparent}");
  });
});
