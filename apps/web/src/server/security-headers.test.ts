// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  FRAME_PAGE_PATH,
  buildCsp,
  commonSecurityHeaders,
  createNonce,
  framePageResponse,
} from "./security-headers";

const directives = (csp: string): Map<string, string> =>
  new Map(
    csp.split("; ").map((part) => {
      const [name = "", ...values] = part.split(" ");
      return [name, values.join(" ")];
    })
  );

const NONCE = "abcdefghijklmnopqrstuv";
const prod = directives(
  buildCsp({
    dev: false,
    https: true,
    jmapOrigin: "https://jmap.example.com/x",
    nonce: NONCE,
  })
);

describe(buildCsp, () => {
  it("allows scripts only by nonce, with strict-dynamic", () => {
    expect(prod.get("script-src")).toBe(`'nonce-${NONCE}' 'strict-dynamic'`);
    expect(prod.get("default-src")).toBe("'none'");
    expect(prod.get("object-src")).toBe("'none'");
  });

  it("requires Trusted Types, from our policies and the libraries' own", () => {
    expect(prod.get("require-trusted-types-for")).toBe("'script'");
    expect(prod.get("trusted-types")).toBe(
      "default nebula-mail-parse nebula-mail-srcdoc dompurify ProseMirrorClipboard 'allow-duplicates'"
    );
  });

  it("closes framing, base and plugins", () => {
    expect([
      prod.get("base-uri"),
      prod.get("frame-ancestors"),
      prod.get("frame-src"),
    ]).toStrictEqual(["'none'", "'none'", "'none'"]);
  });

  it("frames only the email frame page of this app (direct images)", () => {
    const withPage = directives(
      buildCsp({
        appUrl: "https://mail.example.com/",
        dev: false,
        https: true,
        jmapOrigin: "https://jmap.example.com",
        nonce: NONCE,
      })
    );

    expect(withPage.get("frame-src")).toBe(
      "https://mail.example.com/email-frame.html"
    );
    // Nothing else changes.
    expect(
      new Map([...withPage].filter(([name]) => name !== "frame-src"))
    ).toStrictEqual(
      new Map([...prod].filter(([name]) => name !== "frame-src"))
    );
  });

  it("lists the JMAP origin, not its path", () => {
    expect(prod.get("connect-src")).toBe("'self' https://jmap.example.com");
    expect(prod.get("form-action")).toBe("'self' https://jmap.example.com");
  });

  it("never allows unsafe-inline or unsafe-eval for scripts and style elements", () => {
    const risky = [prod.get("script-src"), prod.get("style-src")].join(" ");

    expect(risky).not.toMatch(/unsafe-(?:inline|eval)/u);
    expect(prod.has("upgrade-insecure-requests")).toBeTruthy();
  });

  it("upgrades insecure requests only over https", () => {
    const local = buildCsp({
      dev: false,
      https: false,
      jmapOrigin: "http://localhost:8080",
      nonce: NONCE,
    });

    expect(local).not.toContain("upgrade-insecure-requests");
  });

  it("opens only the dev server's WebSocket and blob worker in dev", () => {
    const dev = directives(
      buildCsp({
        dev: true,
        https: false,
        jmapOrigin: "http://localhost:8080",
        nonce: NONCE,
      })
    );

    expect(dev.get("connect-src")).toBe(
      "'self' http://localhost:8080 ws: wss:"
    );
    expect(dev.get("worker-src")).toBe("'self' blob:");
    expect(dev.has("upgrade-insecure-requests")).toBeFalsy();
  });

  it("with the JMAP proxy, pages connect only to their own origin", () => {
    const proxied = directives(
      buildCsp({
        dev: false,
        https: true,
        imageProxyOrigin: "https://img.example.com",
        jmapOrigin: "https://jmap.example.com",
        jmapProxy: true,
        nonce: NONCE,
      })
    );
    expect(proxied.get("connect-src")).toBe("'self' https://img.example.com");
    // The login form still redirects there.
    expect(proxied.get("form-action")).toBe("'self' https://jmap.example.com");
  });

  it("adds the image proxy origin to img-src and connect-src", () => {
    const withProxy = directives(
      buildCsp({
        dev: false,
        https: true,
        imageProxyOrigin: "https://img.example.com/i",
        jmapOrigin: "https://jmap.example.com",
        nonce: NONCE,
      })
    );

    expect(withProxy.get("img-src")).toBe(
      "'self' data: blob: https://img.example.com"
    );
    expect(withProxy.get("connect-src")).toBe(
      "'self' https://jmap.example.com https://img.example.com"
    );
  });

  it.each([
    ["short", "too-short"],
    ["quote", `${NONCE}'`],
    ["space", `${NONCE} 'unsafe-inline'`],
  ])("refuses a bad nonce (%s)", (_name, nonce) => {
    expect(() =>
      buildCsp({
        dev: false,
        https: true,
        jmapOrigin: "https://a.example",
        nonce,
      })
    ).toThrow("invalid nonce");
  });

  it("refuses a non-http JMAP origin", () => {
    expect(() =>
      buildCsp({
        dev: false,
        https: true,
        jmapOrigin: ["javascript", "x"].join(":"),
        nonce: NONCE,
      })
    ).toThrow("not an http(s) URL");
  });
});

describe(createNonce, () => {
  it("makes distinct 128-bit base64url nonces the CSP accepts", () => {
    const nonces = new Set(Array.from({ length: 50 }, createNonce));

    expect(nonces.size).toBe(50);
    expect(
      [...nonces].every((nonce) => /^[\w-]{22}$/u.test(nonce))
    ).toBeTruthy();
  });
});

describe(commonSecurityHeaders, () => {
  it("sends HSTS only over https", () => {
    expect(commonSecurityHeaders({ https: true })).toHaveProperty(
      "strict-transport-security"
    );
    expect(commonSecurityHeaders({ https: false })).not.toHaveProperty(
      "strict-transport-security"
    );
  });

  it("sets the security headers", () => {
    expect(commonSecurityHeaders({ https: false })).toMatchObject({
      "cross-origin-opener-policy": "same-origin",
      "cross-origin-resource-policy": "same-origin",
      "referrer-policy": "no-referrer",
      "x-content-type-options": "nosniff",
      "x-dns-prefetch-control": "off",
      "x-frame-options": "DENY",
    });
  });

  it("lets only this origin frame the email frame page", () => {
    expect(
      commonSecurityHeaders({ framePage: true, https: true })
    ).toMatchObject({ "x-frame-options": "SAMEORIGIN" });
    expect(commonSecurityHeaders({ https: true })).toMatchObject({
      "x-frame-options": "DENY",
    });
  });
});

describe(framePageResponse, () => {
  it("is served at /email-frame.html with the strict policy of an email", async () => {
    const response = framePageResponse();
    const csp = directives(
      response.headers.get("content-security-policy") ?? ""
    );

    expect(FRAME_PAGE_PATH).toBe("/email-frame.html");
    expect(Object.fromEntries(csp)).toStrictEqual({
      "base-uri": "'none'",
      "default-src": "'none'",
      "font-src": "data:",
      "form-action": "'none'",
      "frame-ancestors": "'self'",
      "img-src": "data: https:",
      "style-src": "'unsafe-inline'",
    });
    const page = await response.text();
    expect(page).not.toMatch(/<script|script-src/u);
    expect(page).toContain('<meta name="referrer" content="no-referrer">');
    expect(response.headers.get("content-type")).toContain("text/html");
  });
});
