// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  browserResponseHeaders,
  checkCsrf,
  contentDisposition,
  downloadHeaders,
  downloadName,
  matchRoute,
  upstreamRequestHeaders,
} from "./request";

const APP = new URL("https://mail.example.com");
const at = (path: string): URL => new URL(path, APP);

describe(matchRoute, () => {
  it("knows each route and its method", () => {
    expect(matchRoute("GET", at("/jmap-proxy/session"))).toStrictEqual({
      ok: true,
      route: { kind: "session" },
    });
    expect(matchRoute("POST", at("/jmap-proxy/api"))).toStrictEqual({
      ok: true,
      route: { kind: "api" },
    });
    expect(matchRoute("POST", at("/jmap-proxy/upload/c1"))).toStrictEqual({
      ok: true,
      route: { accountId: "c1", kind: "upload" },
    });
    expect(
      matchRoute(
        "GET",
        at(
          "/jmap-proxy/download/c1/G-x_9/Factura%20marzo.pdf?type=application/pdf"
        )
      )
    ).toStrictEqual({
      ok: true,
      route: {
        accountId: "c1",
        blobId: "G-x_9",
        kind: "download",
        name: "Factura marzo.pdf",
        type: "application/pdf",
      },
    });
    expect(
      matchRoute(
        "GET",
        at("/jmap-proxy/eventsource?types=Email,Mailbox&closeafter=no&ping=30")
      )
    ).toStrictEqual({
      ok: true,
      route: {
        closeafter: "no",
        kind: "eventsource",
        ping: "30",
        types: "Email,Mailbox",
      },
    });
  });

  it("answers 405 for the wrong method and 404 for unknown paths", () => {
    expect(
      [
        matchRoute("POST", at("/jmap-proxy/session")),
        matchRoute("GET", at("/jmap-proxy/api")),
        matchRoute("GET", at("/jmap-proxy/other")),
        matchRoute("GET", at("/jmap-proxy/session/x")),
        matchRoute("GET", at("/jmap-proxy/toString")),
        matchRoute("GET", at("/other/session")),
      ].map((match) => (match.ok ? 200 : match.status))
    ).toStrictEqual([405, 405, 404, 404, 404, 404]);
  });

  it("refuses ids that aren't JMAP ids (encoded slashes, dots, CRLF)", () => {
    for (const path of [
      "/jmap-proxy/upload/a%2Fb",
      "/jmap-proxy/upload/..%2F..%2Fadmin",
      "/jmap-proxy/upload/a%0D%0AX-Evil:%201",
      "/jmap-proxy/upload/%",
      "/jmap-proxy/upload/a.b",
      "/jmap-proxy/download/a%2F..%2F/b/n",
      "/jmap-proxy/download/a/b%3Fx%3D1/n",
      "/jmap-proxy/download/a/b%23x/n",
    ]) {
      expect(
        matchRoute(path.includes("upload") ? "POST" : "GET", at(path))
      ).toMatchObject({ ok: false, status: 400 });
    }
  });

  it("never lets ../ through (normalised away or refused)", () => {
    for (const path of [
      "/jmap-proxy/download/a/b/..",
      "/jmap-proxy/download/a/b/%2E%2E",
      "/jmap-proxy/download/a/b/.",
      "/jmap-proxy/download/a/../../admin/x/y",
      "/jmap-proxy/../admin",
    ]) {
      expect(matchRoute("GET", at(path)).ok).toBeFalsy();
    }
  });

  it("checks the download's name, type and the push parameters", () => {
    for (const path of [
      "/jmap-proxy/download/a/b/x%0D%0Ay",
      "/jmap-proxy/download/a/b/x%00",
      "/jmap-proxy/download/a/b/x?type=text/html%0D%0AX:1",
      "/jmap-proxy/download/a/b/x?type=text",
      "/jmap-proxy/download/a/b/x?type=text/html;charset=utf-8",
      "/jmap-proxy/eventsource?types=Email%0A",
      "/jmap-proxy/eventsource?types=*&closeafter=maybe",
      "/jmap-proxy/eventsource?types=*&ping=-1",
      "/jmap-proxy/eventsource?types=*&ping=99999",
    ]) {
      expect(matchRoute("GET", at(path))).toMatchObject({
        ok: false,
        status: 400,
      });
    }
    // A slash in a name is fine: it is re-encoded into one segment.
    expect(
      matchRoute("GET", at("/jmap-proxy/download/a/b/x%2Fy.txt")).ok
    ).toBeTruthy();
  });

  it("defaults a missing type to opaque bytes", () => {
    const match = matchRoute("GET", at("/jmap-proxy/download/a/b/x"));
    expect(
      match.ok && match.route.kind === "download" && match.route.type
    ).toBe("application/octet-stream");
  });
});

describe(downloadName, () => {
  it("refuses the dot segments, controls and bad encodings", () => {
    for (const segment of [
      "",
      ".",
      "..",
      "%2E%2E",
      "%2e",
      "a%0Ab",
      "a%0Db",
      "a%7Fb",
      "a%C2%85b",
      "%E0%A4%A",
      "%ED%A0%80",
      "x".repeat(256),
    ]) {
      expect(downloadName(segment)).toBeUndefined();
    }
    expect(downloadName("a%2F..%2Fb")).toBe("a/../b");
    expect(downloadName("%E2%82%AC.txt")).toBe("€.txt");
  });
});

const request = (headers: Record<string, string>): Request =>
  new Request(at("/jmap-proxy/session"), { headers });

describe(checkCsrf, () => {
  const good = {
    "sec-fetch-mode": "cors",
    "sec-fetch-site": "same-origin",
    "x-nebula-proxy": "1",
  };

  it("accepts the app's own fetches, and tells the service worker apart", () => {
    expect(checkCsrf(request(good), APP)).toStrictEqual({
      background: false,
      ok: true,
    });
    expect(
      checkCsrf(request({ ...good, "x-nebula-proxy": "background" }), APP)
    ).toStrictEqual({ background: true, ok: true });
    // Browsers without Sec-Fetch-Site: the Origin then.
    expect(
      checkCsrf(request({ origin: APP.origin, "x-nebula-proxy": "1" }), APP).ok
    ).toBeTruthy();
  });

  it("refuses a missing or wrong custom header", () => {
    const { "x-nebula-proxy": _, ...without } = good;
    expect(checkCsrf(request(without), APP)).toStrictEqual({
      ok: false,
      reason: "header",
    });
    expect(
      checkCsrf(request({ ...good, "x-nebula-proxy": "yes" }), APP)
    ).toStrictEqual({ ok: false, reason: "header" });
  });

  it("refuses other sites, even with the header", () => {
    for (const site of ["cross-site", "same-site", "none"]) {
      expect(
        checkCsrf(request({ ...good, "sec-fetch-site": site }), APP)
      ).toStrictEqual({ ok: false, reason: "origin" });
    }
    // Sec-Fetch-Site wins over a matching Origin.
    expect(
      checkCsrf(
        request({
          ...good,
          origin: APP.origin,
          "sec-fetch-site": "cross-site",
        }),
        APP
      ).ok
    ).toBeFalsy();
    for (const origin of [
      "https://evil.example",
      "https://mail.example.com.evil.example",
      "http://mail.example.com",
      "null",
    ]) {
      expect(
        checkCsrf(request({ origin, "x-nebula-proxy": "1" }), APP)
      ).toStrictEqual({ ok: false, reason: "origin" });
    }
    // Neither Sec-Fetch-Site nor Origin: refused.
    expect(checkCsrf(request({ "x-nebula-proxy": "1" }), APP).ok).toBeFalsy();
  });

  it("refuses navigations (links, forms, a download opened as a page)", () => {
    expect(
      checkCsrf(request({ ...good, "sec-fetch-mode": "navigate" }), APP)
    ).toStrictEqual({ ok: false, reason: "mode" });
  });
});

describe(upstreamRequestHeaders, () => {
  it("forwards only the allowlist, never cookies or the browser's auth", () => {
    const headers = upstreamRequestHeaders(
      new Request(at("/jmap-proxy/api"), {
        headers: {
          accept: "application/json",
          "accept-language": "es",
          authorization: "Basic ZXZpbDpldmls",
          "content-type": "application/json",
          cookie: "__Host-wm-session=secret; other=1",
          host: "evil.example",
          "last-event-id": "42",
          origin: APP.origin,
          "x-forwarded-for": "1.2.3.4",
          "x-nebula-proxy": "1",
        },
        method: "POST",
      }),
      "access-1"
    );
    expect(Object.fromEntries(headers)).toStrictEqual({
      accept: "application/json",
      "accept-encoding": "identity",
      "accept-language": "es",
      authorization: "Bearer access-1",
      "content-type": "application/json",
      "last-event-id": "42",
    });
  });
});

describe(browserResponseHeaders, () => {
  it("forwards only the allowlist: no Set-Cookie, Location or CORS", () => {
    const upstream = new Response("{}", {
      headers: {
        "access-control-allow-origin": "*",
        "content-length": "2",
        "content-security-policy": "default-src *",
        "content-type": "application/json",
        location: "https://evil.example/",
        "retry-after": "5",
        server: "Stalwart",
        "set-cookie": "a=b; Path=/",
      },
    });
    expect(Object.fromEntries(browserResponseHeaders(upstream))).toStrictEqual({
      "cache-control": "no-store",
      "content-length": "2",
      // Ours, never the JMAP server's: nothing renders on our origin.
      "content-security-policy": "sandbox; default-src 'none'",
      "content-type": "application/json",
      "retry-after": "5",
      "x-content-type-options": "nosniff",
    });
  });

  it("drops Content-Length of a compressed body (fetch inflates it)", () => {
    const upstream = new Response("{}", {
      headers: { "content-encoding": "gzip", "content-length": "1" },
    });
    expect(browserResponseHeaders(upstream).has("content-length")).toBeFalsy();
  });
});

describe(downloadHeaders, () => {
  it("is always an attachment, sandboxed and not sniffed", () => {
    const headers = downloadHeaders(
      new Response("x", {
        headers: {
          "content-disposition": "inline",
          "content-type": "text/html",
        },
      }),
      'evil".html'
    );
    expect(headers.get("content-disposition")).toBe(
      "attachment; filename=\"evil_.html\"; filename*=UTF-8''evil%22.html"
    );
    expect(headers.get("content-security-policy")).toBe(
      "sandbox; default-src 'none'"
    );
    expect(headers.get("x-content-type-options")).toBe("nosniff");
    expect(headers.get("cache-control")).toBe("no-store");
  });

  it("makes a strange media type opaque bytes", () => {
    const headers = downloadHeaders(
      new Response("x", { headers: { "content-type": "text html" } }),
      "a"
    );
    expect(headers.get("content-type")).toBe("application/octet-stream");
  });
});

describe(contentDisposition, () => {
  it("encodes any name (RFC 8187) with an ASCII fallback", () => {
    expect(contentDisposition("Café (1).pdf")).toBe(
      "attachment; filename=\"Caf_ (1).pdf\"; filename*=UTF-8''Caf%C3%A9%20%281%29.pdf"
    );
    expect(contentDisposition("a\\b%c")).toBe(
      "attachment; filename=\"a_b_c\"; filename*=UTF-8''a%5Cb%25c"
    );
  });
});
