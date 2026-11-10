// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createHash } from "node:crypto";

import { describe, expect, it } from "vitest";

import { isPublicHttpsUrl } from "../mail/public-url";
import { ONE_CLICK_INIT, unsubscribePage } from "./unsubscribe-page";

describe(isPublicHttpsUrl, () => {
  it.each([
    ["https://lists.example.org/u?id=1", true],
    ["https://news.shop.co.uk/unsub", true],
    ["http://lists.example.org/u", false],
    ["https://user:pw@lists.example.org/u", false],
    ["https://lists.example.org:8443/u", false],
    ["https://127.0.0.1/u", false],
    ["https://2130706433/u", false],
    ["https://0x7f.1/u", false],
    ["https://[::1]/u", false],
    ["https://localhost/u", false],
    ["https://printer.local/u", false],
    ["https://router.lan/u", false],
    ["https://intranet/u", false],
    ["https://svc.internal/u", false],
    ["https://nas.home.arpa./u", false],
    // oxlint-disable-next-line no-script-url -- a hostile link, on purpose.
    ["javascript:alert(1)", false],
    ["not a url", false],
  ])("%s → %s", (url, expected) => {
    expect(isPublicHttpsUrl(url)).toBe(expected);
  });
});

const hashOf = (text: string): string =>
  `'sha256-${createHash("sha256").update(text).digest("base64")}'`;

describe(unsubscribePage, () => {
  const texts = {
    body: "Ask {host}",
    confirm: "Go",
    failed: "Failed",
    invalid: "No",
    sent: "Sent to {host}",
    title: "</script><b>x</b>",
  };
  const page = unsubscribePage({ en: texts, es: texts });

  const csp = /content="(?<csp>[^"]+)"/u.exec(page)?.groups?.csp ?? "";
  const srcdoc = (/srcdoc="(?<doc>[^"]*)"/u.exec(page)?.groups?.doc ?? "")
    .replaceAll("&quot;", '"')
    .replaceAll("&amp;", "&");
  const frameScript =
    /^<script>(?<body>[\s\S]*)<\/script>$/u.exec(srcdoc)?.groups?.body ?? "";

  it("allows only its own scripts and style, fetch to https and no forms", () => {
    const script =
      /<script>(?<body>[^<]*)<\/script><\/body>/u.exec(page)?.groups?.body ??
      "";
    expect(csp).toContain(
      `script-src ${hashOf(script)} ${hashOf(frameScript)}`
    );
    expect(csp).toContain("connect-src https:");
    expect(csp).toContain("form-action 'none'");
    expect(csp).toContain("default-src 'none'");
  });

  it("keeps the texts inside the script", () => {
    // The page's script, and the frame's inside its srcdoc attribute.
    expect(page.match(/<\/script>/gu)).toHaveLength(2);
    expect(frameScript).not.toContain("</script>");
    expect(page).not.toContain("<b>x</b>");
  });

  it("sends the RFC 8058 request without cookies or referrer", () => {
    // RFC 8058 §3.1: no cookies, authorization or other context.
    expect(ONE_CLICK_INIT).toStrictEqual({
      body: "List-Unsubscribe=One-Click",
      cache: "no-store",
      credentials: "omit",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      method: "POST",
      mode: "no-cors",
      redirect: "follow",
      referrerPolicy: "no-referrer",
    });
    expect(frameScript).toContain(
      `fetch(e.data,${JSON.stringify(ONE_CLICK_INIT)})`
    );
  });

  it("sends from an opaque origin, only after a click", () => {
    // `Origin: null`, not the app's.
    expect(page).toContain('<iframe id="s" sandbox="allow-scripts" hidden ');
    expect(page).not.toContain("allow-same-origin");
    expect(page).not.toMatch(/<form|\.submit\(|requestSubmit|fetch\(target/u);
    // The page only hands the URL to the frame inside its click handler.
    expect(page.indexOf("w.postMessage(target")).toBeGreaterThan(
      page.indexOf('b.addEventListener("click"')
    );
  });
});
