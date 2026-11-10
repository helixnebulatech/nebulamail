// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import path from "node:path";
import { inflateRawSync } from "node:zlib";

import { describe, expect, it } from "vitest";

import { framePage } from "../lib/frame-page";
import { buildStaticCsp } from "./csp";
import { staticProxyOrigin } from "./options";
import {
  APP_MANIFEST,
  FIRST_PARTY_CLIENT,
  appManifest,
  bffLeftover,
  checkBundleFiles,
  hashedPageName,
  helperPageDefines,
  helperPages,
  inlineEntryScripts,
  inlineScriptHashes,
  rebasePublicFile,
  staticBasePath,
  withCsp,
  withoutBuildTime,
} from "./plugin";
import { crc32, createZip } from "./zip";

const NONCE = "abcdefghijklmnopqrstuv";
const HASH = `sha256-${"A".repeat(43)}=`;

describe(buildStaticCsp, () => {
  it("allows scripts by hash only and everything on our origin", () => {
    const csp = buildStaticCsp({
      imageProxyOrigin: "",
      scriptHashes: [HASH, HASH],
      styleNonce: NONCE,
    });
    expect(csp).toContain(`script-src '${HASH}' 'strict-dynamic'`);
    expect(csp).toContain("connect-src 'self';");
    expect(csp).toContain(`style-src 'self' 'nonce-${NONCE}'`);
    expect(csp).toContain("default-src 'none'");
    expect(csp).not.toContain("unsafe-eval");
  });

  it("adds the image proxy's origin", () => {
    const csp = buildStaticCsp({
      imageProxyOrigin: "https://img.example.com",
      scriptHashes: [HASH],
      styleNonce: NONCE,
    });
    expect(csp).toContain("img-src 'self' data: blob: https://img.example.com");
  });

  it("refuses an http proxy, a bad nonce or no hashes", () => {
    const options = {
      imageProxyOrigin: "",
      scriptHashes: [HASH],
      styleNonce: NONCE,
    };
    expect(() =>
      buildStaticCsp({ ...options, imageProxyOrigin: "http://img.example" })
    ).toThrow("https");
    expect(() =>
      buildStaticCsp({ ...options, imageProxyOrigin: "https://img.example/x" })
    ).toThrow("origin");
    expect(() => buildStaticCsp({ ...options, styleNonce: "a'b" })).toThrow(
      "nonce"
    );
    expect(() => buildStaticCsp({ ...options, scriptHashes: [] })).toThrow(
      "inline script"
    );
  });
});

describe(withCsp, () => {
  const built = `<!DOCTYPE html><html><head><meta property="csp-nonce" content="${NONCE}"><script>a()</script><script type="module" async="" src="/x.js" nonce="${NONCE}"></script></head><body></body></html>`;
  const html = inlineEntryScripts(built);

  it("hashes inline scripts only", () => {
    expect(inlineScriptHashes(built)).toHaveLength(1);
    expect(inlineScriptHashes(html)).toHaveLength(2);
  });

  it("puts the CSP first in <head>, after the charset", () => {
    const out = withCsp(html, { imageProxyOrigin: "", styleNonce: NONCE });
    expect(out).toMatch(
      /^<!DOCTYPE html><html><head><meta http-equiv="Content-Security-Policy" content="[^"]+">/u
    );
    const withCharset = html.replace("<head>", '<head><meta charSet="utf-8"/>');
    expect(
      withCsp(withCharset, { imageProxyOrigin: "", styleNonce: NONCE })
    ).toContain(
      '<meta charSet="utf-8"/><meta http-equiv="Content-Security-Policy"'
    );
  });

  it("turns the entry script into an inline loader", () => {
    const out = inlineEntryScripts(
      `${built}<link rel="modulepreload" href="/nebulamail/assets/a.js"/>`
    );
    expect(out).toContain('s.type="module"');
    expect(out).toContain('new URL("/x.js",location.href)');
    expect(out).not.toContain("modulepreload");
    expect(out).not.toMatch(/<script[^>]*src=/u);
  });

  it("refuses a page that still loads a script by URL", () => {
    expect(() =>
      withCsp(built, { imageProxyOrigin: "", styleNonce: NONCE })
    ).toThrow("by URL");
  });

  it("refuses a page without the build's nonce", () => {
    expect(() =>
      withCsp(html, { imageProxyOrigin: "", styleNonce: "x".repeat(22) })
    ).toThrow("nonce");
  });
});

describe(rebasePublicFile, () => {
  it("moves absolute paths under the base", () => {
    expect(
      rebasePublicFile(
        '{"start_url":"/","src":"/icons/a.png"} <a href="/">',
        "/nebulamail/"
      )
    ).toBe(
      '{"start_url":"/nebulamail/","src":"/nebulamail/icons/a.png"} <a href="/nebulamail/">'
    );
  });

  it("leaves protocol-relative URLs alone", () => {
    expect(rebasePublicFile('"//cdn.example/x"', "/nebulamail/")).toBe(
      '"//cdn.example/x"'
    );
  });
});

describe(createZip, () => {
  const entries = [
    { data: new TextEncoder().encode("hello"), name: "b.txt" },
    { data: new TextEncoder().encode("<html>"), name: "index.html" },
  ];

  it("is reproducible", () => {
    expect(createZip(entries)).toStrictEqual(createZip(entries.toReversed()));
  });

  it("stores the files so they can be read back", () => {
    const zip = createZip(entries);
    const view = new DataView(zip.buffer);
    // First local header: index.html sorts after b.txt.
    expect(view.getUint32(0, true)).toBe(0x04_03_4b_50);
    const nameLength = view.getUint16(26, true);
    const size = view.getUint32(18, true);
    const name = new TextDecoder().decode(zip.slice(30, 30 + nameLength));
    const data = inflateRawSync(
      zip.slice(30 + nameLength, 30 + nameLength + size)
    );
    expect(name).toBe("b.txt");
    expect(new TextDecoder().decode(data)).toBe("hello");
    expect(view.getUint32(14, true)).toBe(
      crc32(entries[0]?.data ?? new Uint8Array())
    );
  });

  it.each(["../x", "/etc/passwd", "a/../b", "a\\b", "a/./b", "a//b", "a/"])(
    "refuses %s",
    (name) => {
      expect(() => createZip([{ data: new Uint8Array(), name }])).toThrow(
        "unsafe"
      );
    }
  );

  it("refuses the same name twice", () => {
    expect(() =>
      createZip([...entries, { data: new Uint8Array(), name: "b.txt" }])
    ).toThrow("twice");
  });

  it("computes the standard CRC-32", () => {
    expect(crc32(new TextEncoder().encode("123456789"))).toBe(0xcb_f4_39_26);
  });
});

describe(bffLeftover, () => {
  it("finds our server's endpoints and server functions", () => {
    expect(bffLeftover('fetch("/auth/token")')).toBe("/auth/token");
    expect(bffLeftover(`h(\`${"ab".repeat(32)}\`)`)).toBe("a server function");
    expect(
      bffLeftover("function f(e){let t=`/nebulamail/_serverFn/`+e}")
    ).toBeUndefined();
  });
});

describe(withoutBuildTime, () => {
  it("zeroes the prerender time of each match", () => {
    expect(
      withoutBuildTime('matches:[{i:"__root__#",u:1793295282617,s:"success"}]')
    ).toBe('matches:[{i:"__root__#",u:0,s:"success"}]');
  });
});

describe(framePage, () => {
  it("allows https images and nothing that runs", () => {
    const page = framePage();
    expect(page).toContain("img-src data: https:");
    expect(page).toContain("default-src 'none'");
    expect(page).not.toMatch(/<script|script-src/u);
    expect(page).toContain('<meta name="referrer" content="no-referrer">');
  });
});

describe(hashedPageName, () => {
  it("names a page after its content", () => {
    const name = hashedPageName("email-frame", framePage());

    expect(name).toMatch(/^email-frame\.[\w-]{10}\.html$/u);
    expect(hashedPageName("email-frame", framePage())).toBe(name);
    // Another CSP (or any change) is another name: no stale immutable copy.
    expect(
      hashedPageName("email-frame", framePage().replace("https:", "http:"))
    ).not.toBe(name);
  });
});

describe(helperPages, () => {
  it("gives the zip's helper pages hashed names, passed to the app", async () => {
    const pages = await helperPages(path.resolve(import.meta.dirname, "../.."));

    expect(pages.frame.name).toMatch(/^email-frame\.[\w-]{10}\.html$/u);
    expect(pages.unsubscribe.name).toMatch(/^unsubscribe\.[\w-]{10}\.html$/u);
    expect(pages.frame.content).toBe(framePage());
    expect(helperPageDefines(pages)).toStrictEqual({
      "import.meta.env.NEBULA_FRAME_PAGE": JSON.stringify(pages.frame.name),
      "import.meta.env.NEBULA_UNSUBSCRIBE_PAGE": JSON.stringify(
        pages.unsubscribe.name
      ),
    });
  });
});

describe(appManifest, () => {
  it("is the small manifest Stalwart 1.0 asks for", () => {
    const text = appManifest("1.2.3");
    expect(new TextEncoder().encode(text).length).toBeLessThanOrEqual(4096);
    expect(JSON.parse(text)).toStrictEqual({
      id: "nebula-mail",
      minServerVersion: "1.0.0",
      version: "1.2.3",
    });
    expect(appManifest("1.2.3")).toBe(text);
  });

  it("refuses a version Stalwart can't read", () => {
    expect(() => appManifest("1.2")).toThrow(/version/u);
    expect(() => appManifest("latest")).toThrow(/version/u);
    expect(JSON.parse(appManifest("1.0.0-rc.1"))).toMatchObject({
      version: "1.0.0-rc.1",
    });
  });
});

describe(checkBundleFiles, () => {
  it("needs index.html and stalwart-app.json at the root", () => {
    expect(() => {
      checkBundleFiles(["index.html", APP_MANIFEST, "assets/a.js"]);
    }).not.toThrow();
    expect(() => {
      checkBundleFiles(["index.html"]);
    }).toThrow(APP_MANIFEST);
    expect(() => {
      checkBundleFiles(["assets/index.html", APP_MANIFEST]);
    }).toThrow("index.html");
  });

  it("refuses a page without a hash in its name (served immutable)", () => {
    for (const name of ["email-frame.html", "assets/unsubscribe.html"]) {
      expect(() => {
        checkBundleFiles(["index.html", APP_MANIFEST, name]);
      }).toThrow(/no hash/u);
    }
    expect(() => {
      checkBundleFiles([
        "index.html",
        "offline.html",
        APP_MANIFEST,
        "email-frame.Ab3_-x9Zq1.html",
      ]);
    }).not.toThrow();
  });

  it("never lets oauth-client.json in (a first-party client, unscoped tokens)", () => {
    for (const name of [FIRST_PARTY_CLIENT, `assets/${FIRST_PARTY_CLIENT}`]) {
      expect(() => {
        checkBundleFiles(["index.html", APP_MANIFEST, name]);
      }).toThrow(/first-party/u);
    }
  });
});

describe(staticBasePath, () => {
  it("is one segment, with or without slashes", () => {
    expect(staticBasePath()).toBe("/nebulamail/");
    expect(staticBasePath("/mail.app/")).toBe("/mail.app/");
    expect(staticBasePath("nebula_mail~1")).toBe("/nebula_mail~1/");
  });

  it("refuses what Stalwart refuses as urlPrefix", () => {
    for (const value of [
      "",
      "/",
      "apps/mail",
      "web mail",
      "web%20mail",
      ".",
      "..",
      "correo-ñ",
    ]) {
      expect(() => staticBasePath(value)).toThrow(/one path segment/u);
    }
  });

  it("refuses Stalwart's own endpoints and its web interface", () => {
    for (const value of [
      "jmap",
      "API",
      ".well-known",
      "login",
      "admin",
      "/account/",
    ]) {
      expect(() => staticBasePath(value)).toThrow(/taken by Stalwart/u);
    }
  });
});

describe(staticProxyOrigin, () => {
  it("normalizes the origin, or none", () => {
    expect(staticProxyOrigin("https://img.example.com/")).toBe(
      "https://img.example.com"
    );
    expect(staticProxyOrigin()).toBe("");
    expect(() => staticProxyOrigin("http://img.example.com")).toThrow("https");
  });
});
