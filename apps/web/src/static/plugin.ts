// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Last step of `bun run build:static`, after TanStack Start
// has prerendered the SPA shell:
//   1. index.html gets its CSP as the first <meta> (static/csp.ts), with the
//      hashes of its inline scripts;
//   2. the manifest and the offline page point inside the base path;
//   3. nothing of the BFF may be left in the client code (a build error);
//   4. dist/nebula-mail-static.zip and its SHA-256, for Stalwart, with the
//      manifest Stalwart 1.0 requires (`stalwart-app.json`) and never an
//      `oauth-client.json`.

import { createHash } from "node:crypto";
import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";

import type { Plugin } from "vite";
import { z } from "zod";

import { framePage } from "../lib/frame-page.ts";
import { LOADER_POLICY, buildStaticCsp } from "./csp.ts";
import type { UnsubscribePageTexts } from "./unsubscribe-page.ts";
import { unsubscribePage } from "./unsubscribe-page.ts";
import { createZip } from "./zip.ts";

export interface StaticBundleOptions {
  /** "/nebulamail/": where Stalwart mounts the app. */
  readonly base: string;
  readonly styleNonce: string;
  readonly imageProxyOrigin: string;
}

/** Endpoints only our server has: the static code must never call them. */
export const FORBIDDEN_IN_STATIC = [
  "/auth/token",
  "/auth/logout",
  "/api/images/tickets",
  "/api/images/preload",
  "/api/unsubscribe",
  // The full BFF mode's JMAP proxy (lib/jmap-proxy.ts).
  "/jmap-proxy/",
] as const;

/**
 * A TanStack Start server function in client code: its id (a SHA-256 in
 * hex) passed to the RPC factory. (The factory itself, with its
 * "/_serverFn/" path, is in the runtime of every build.)
 */
const SERVER_FN_CALL = /\(\s*[`"'][\da-f]{64}[`"']\s*\)/u;

/** The first thing of our server's that `code` still uses, or undefined. */
export const bffLeftover = (code: string): string | undefined =>
  FORBIDDEN_IN_STATIC.find((endpoint) => code.includes(endpoint)) ??
  (SERVER_FN_CALL.test(code) ? "a server function" : undefined);

export const ZIP_NAME = "nebula-mail-static.zip";

/** Stalwart 1.0 refuses a bundle without this file at its root. */
export const APP_MANIFEST = "stalwart-app.json";

/**
 * Never in the zip: with it, Stalwart makes the app a first-party client,
 * whose tokens have no scopes (every permission of the account, management
 * included for an admin) and need no consent. The guide registers a public
 * `OAuthClient` with the four scopes instead (docs/static.md).
 */
export const FIRST_PARTY_CLIENT = "oauth-client.json";

/** Stalwart's manifest limit. */
const MAX_APP_MANIFEST_BYTES = 4096;

/** `x.y.z`, optionally with a pre-release or build suffix (Stalwart's check). */
const SEMVER = /^\d+\.\d+\.\d+(?:[-+][\w.+-]*)?$/u;

/**
 * The bundle's manifest for Stalwart 1.0: its `id`, its `version` and the
 * oldest server it works with (1.0.0: OAuth scopes, consent, revocation).
 */
export const appManifest = (version: string): string => {
  if (!SEMVER.test(version)) {
    throw new Error(`static build: "${version}" is not a version like 1.2.3`);
  }
  const text = `${JSON.stringify({
    id: "nebula-mail",
    minServerVersion: "1.0.0",
    version,
  })}\n`;
  if (new TextEncoder().encode(text).length > MAX_APP_MANIFEST_BYTES) {
    throw new Error("static build: stalwart-app.json is too large");
  }
  return text;
};

/**
 * Pages that keep a fixed name although Stalwart serves every file but
 * index.html as `immutable` for a year: index.html isn't cached, and
 * offline.html is precached by the worker with `cache: "reload"`.
 */
const FIXED_NAME_PAGES = new Set(["index.html", "offline.html"]);

/** `name.<10 characters of the hash>.html`, as `hashedPageName` writes. */
const HASHED_PAGE = /\.[\w-]{10}\.html$/u;

/** What Stalwart needs at the zip's root, and what must never be there. */
export const checkBundleFiles = (names: readonly string[]): void => {
  for (const needed of ["index.html", APP_MANIFEST]) {
    if (!names.includes(needed)) {
      throw new Error(`static build: the zip has no ${needed}`);
    }
  }
  const unhashed = names.find(
    (name) =>
      name.endsWith(".html") &&
      !FIXED_NAME_PAGES.has(name) &&
      !HASHED_PAGE.test(name)
  );
  if (unhashed !== undefined) {
    throw new Error(
      `static build: ${unhashed} has no hash in its name, and Stalwart serves it as immutable`
    );
  }
  if (names.some((name) => name.split("/").at(-1) === FIRST_PARTY_CLIENT)) {
    throw new Error(
      `static build: ${FIRST_PARTY_CLIENT} would make the app a first-party client with unscoped tokens`
    );
  }
};

/**
 * Stalwart's rules for an Application's `urlPrefix` (1.0): one path segment
 * of letters, digits, `-`, `.`, `_` or `~`, not `.` or `..`, and none of
 * its own endpoints. `admin` and `account` are its web interface's.
 */
const URL_PREFIX = /^[\w.~-]+$/u;
const RESERVED_PREFIXES = new Set([
  "jmap",
  "dav",
  ".well-known",
  "auth",
  "scim",
  "api",
  "mail",
  "calendar",
  "autodiscover",
  "robots.txt",
  "healthz",
  "metrics",
  "logo",
  "form",
  "login",
  "device",
  "admin",
  "account",
]);

/**
 * NEBULA_BASE_PATH as the build's base, `/<prefix>/`. A value Stalwart
 * would refuse as `urlPrefix` fails the build instead of the install.
 */
export const staticBasePath = (value = "nebulamail"): string => {
  const prefix = value.replaceAll(/^\/+|\/+$/gu, "");
  if (!URL_PREFIX.test(prefix) || prefix === "." || prefix === "..") {
    throw new Error(
      `NEBULA_BASE_PATH: "${value}" must be one path segment of letters, digits, "-", ".", "_" or "~", like "nebulamail"`
    );
  }
  if (RESERVED_PREFIXES.has(prefix.toLowerCase())) {
    throw new Error(
      `NEBULA_BASE_PATH: "${prefix}" is taken by Stalwart itself; use another, like "nebulamail"`
    );
  }
  return `/${prefix}/`;
};

const PackageSchema = z.object({ version: z.string().optional() });

/** apps/web/package.json's version (0.0.0 if missing). */
const appVersion = async (root: string): Promise<string> =>
  PackageSchema.parse(
    JSON.parse(await readFile(path.join(root, "package.json"), "utf-8"))
  ).version ?? "0.0.0";

const INLINE_SCRIPT =
  /<script\b(?<attributes>[^>]*)>(?<body>[\s\S]*?)<\/script>/giu;

/** `sha256-…` of every inline script (those without `src`). */
export const inlineScriptHashes = (html: string): string[] =>
  [...html.matchAll(INLINE_SCRIPT)]
    .filter((match) => !/\bsrc\s*=/iu.test(match.groups?.attributes ?? ""))
    .map(
      (match) =>
        `sha256-${createHash("sha256")
          .update(match.groups?.body ?? "")
          .digest("base64")}`
    );

const escapeAttribute = (value: string): string =>
  value.replaceAll("&", "&amp;").replaceAll('"', "&quot;");

/**
 * `<stem>.<hash>.html`: 10 characters of the content's SHA-256 (base64url).
 * Stalwart serves every file but index.html as `immutable` for a year, so a
 * page that can change needs a new name when it does.
 */
export const hashedPageName = (stem: string, content: string): string =>
  `${stem}.${createHash("sha256").update(content).digest("base64url").slice(0, 10)}.html`;

const UNSUBSCRIBE_KEYS = {
  body: "unsubscribe_page_body",
  confirm: "unsubscribe_page_confirm",
  failed: "unsubscribe_page_failed",
  invalid: "unsubscribe_page_invalid",
  sent: "unsubscribe_page_sent",
  title: "unsubscribe_page_title",
} as const;

/** The texts of unsubscribe.html, from messages/<locale>.json. */
const unsubscribeTexts = async (
  root: string
): Promise<Record<"en" | "es", UnsubscribePageTexts>> => {
  const read = async (locale: "en" | "es"): Promise<UnsubscribePageTexts> => {
    const all: unknown = JSON.parse(
      await readFile(path.join(root, "messages", `${locale}.json`), "utf-8")
    );
    const text = (key: string): string => {
      const value: unknown =
        typeof all === "object" && all !== null
          ? Reflect.get(all, key)
          : undefined;
      if (typeof value !== "string") {
        throw new TypeError(`messages/${locale}.json has no "${key}"`);
      }
      return value;
    };
    return {
      body: text(UNSUBSCRIBE_KEYS.body),
      confirm: text(UNSUBSCRIBE_KEYS.confirm),
      failed: text(UNSUBSCRIBE_KEYS.failed),
      invalid: text(UNSUBSCRIBE_KEYS.invalid),
      sent: text(UNSUBSCRIBE_KEYS.sent),
      title: text(UNSUBSCRIBE_KEYS.title),
    };
  };
  const [en, es] = await Promise.all([read("en"), read("es")]);
  return { en, es };
};

export interface HelperPage {
  readonly name: string;
  readonly content: string;
}

/**
 * The zip's helper pages with their hashed names: the frame page of
 * mail/remote-images.ts `DIRECT_FRAME_PAGE` and the page of
 * mail/one-click-unsubscribe.ts `UNSUBSCRIBE_PAGE`. They don't depend on the
 * client build, so their names are known before it (`define`).
 */
export const helperPages = async (
  root: string
): Promise<{
  readonly frame: HelperPage;
  readonly unsubscribe: HelperPage;
}> => {
  const frame = framePage();
  const unsubscribe = unsubscribePage(await unsubscribeTexts(root));
  return {
    frame: { content: frame, name: hashedPageName("email-frame", frame) },
    unsubscribe: {
      content: unsubscribe,
      name: hashedPageName("unsubscribe", unsubscribe),
    },
  };
};

/** The `define` that gives the app the helper pages' names. */
export const helperPageDefines = (pages: {
  readonly frame: HelperPage;
  readonly unsubscribe: HelperPage;
}): Record<string, string> => ({
  "import.meta.env.NEBULA_FRAME_PAGE": JSON.stringify(pages.frame.name),
  "import.meta.env.NEBULA_UNSUBSCRIBE_PAGE": JSON.stringify(
    pages.unsubscribe.name
  ),
});

const ENTRY_SCRIPT =
  /<script\b(?=[^>]*\btype="module")[^>]*\bsrc="(?<src>\/[\w./@+-]+\.js)"[^>]*><\/script>/giu;
const MODULE_PRELOAD = /<link\b(?=[^>]*\brel="modulepreload")[^>]*\/?>/giu;

/**
 * An inline script that adds `<script type="module" src>`. The CSP trusts
 * hashed inline scripts and what they add ('strict-dynamic'), never a file
 * just for being on this origin (Stalwart serves attachments here too), and
 * a parser-inserted `<script src>` or an `import()` from one isn't trusted.
 * Its Trusted Types policy only accepts this one URL.
 */
const entryLoader = (src: string): string => {
  const url = JSON.stringify(src);
  const policy = JSON.stringify(LOADER_POLICY);
  return `<script>(()=>{const u=new URL(${url},location.href).href;const p=self.trustedTypes?.createPolicy(${policy},{createScriptURL:(v)=>{if(v!==u)throw new TypeError(${policy});return v}});const s=document.createElement("script");s.type="module";s.src=p?p.createScriptURL(u):u;document.head.append(s)})()</script>`;
};

/**
 * The entry `<script src>` becomes an inline loader (above), and the
 * `modulepreload` links go (the CSP would block them; Vite preloads from
 * the code too).
 */
export const inlineEntryScripts = (html: string): string =>
  html
    .replaceAll(MODULE_PRELOAD, "")
    .replaceAll(ENTRY_SCRIPT, (_whole, src: string) => entryLoader(src));

/**
 * The time of the prerender, which TanStack Start writes in the shell's
 * route state (`u:` of each match): 0, so the same sources always give the
 * same zip. The router then sees that state as old and reruns the root's
 * `beforeLoad` in the browser, which only reads a cookie.
 */
export const withoutBuildTime = (html: string): string =>
  html.replaceAll(/(?<match>\{i:"[^"]*",u:)\d+(?=,)/gu, "$<match>0");

/** index.html with its CSP <meta> at the top of <head>. */
export const withCsp = (
  html: string,
  options: Omit<StaticBundleOptions, "base">
): string => {
  if (/<script\b[^>]*\bsrc=/iu.test(html)) {
    throw new Error("index.html still loads a script by URL");
  }
  if (html.includes('http-equiv="Content-Security-Policy"')) {
    throw new Error("index.html already has a CSP <meta>");
  }
  if (!html.includes(`content="${options.styleNonce}"`)) {
    throw new Error("index.html doesn't carry the build's style nonce");
  }
  const csp = buildStaticCsp({
    imageProxyOrigin: options.imageProxyOrigin,
    scriptHashes: inlineScriptHashes(html),
    styleNonce: options.styleNonce,
  });
  const meta = `<meta http-equiv="Content-Security-Policy" content="${escapeAttribute(csp)}">`;
  // Right after <meta charset>, which must stay in the first 1024 bytes.
  const head = /<head[^>]*>(?:<meta charset[^>]*>)?/iu.exec(html);
  if (head === null) {
    throw new Error("index.html has no <head>");
  }
  const at = head.index + head[0].length;
  if (/<(?:script|link|style)\b/iu.test(html.slice(0, at))) {
    throw new Error("index.html loads something before its CSP");
  }
  return `${html.slice(0, at)}${meta}${html.slice(at)}`;
};

/** Absolute app paths in public files ("/icons/…") → under the base path. */
export const rebasePublicFile = (text: string, base: string): string =>
  text.replaceAll(
    /(?<before>"|href="|src=")\/(?<rest>[^"/][^"]*|)"/gu,
    (_whole, before: string, rest: string) => `${before}${base}${rest}"`
  );

const listFiles = async (root: string, dir = ""): Promise<string[]> => {
  const entries = await readdir(path.join(root, dir), { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const name = dir === "" ? entry.name : `${dir}/${entry.name}`;
      return entry.isDirectory() ? await listFiles(root, name) : [name];
    })
  );
  return nested.flat();
};

export const staticBundle = (options: StaticBundleOptions): Plugin => {
  let pages: Awaited<ReturnType<typeof helperPages>> | undefined;
  return {
    apply: "build",
    // The names go into the client code, so they are fixed before it builds.
    config: async (config) => {
      pages = await helperPages(path.resolve(config.root ?? process.cwd()));
      return { define: helperPageDefines(pages) };
    },
    buildApp: {
      async handler(builder) {
        const { client } = builder.environments;
        if (client === undefined) {
          throw new Error("static build: no client environment");
        }
        const outDir = path.resolve(
          client.config.root,
          client.config.build.outDir
        );
        const indexFile = path.join(outDir, "index.html");
        await writeFile(
          indexFile,
          withCsp(
            inlineEntryScripts(
              withoutBuildTime(await readFile(indexFile, "utf-8"))
            ),
            options
          )
        );
        await Promise.all(
          ["manifest.webmanifest", "offline.html"].map(async (name) => {
            const file = path.join(outDir, name);
            await writeFile(
              file,
              rebasePublicFile(await readFile(file, "utf-8"), options.base)
            );
          })
        );
        if (pages === undefined) {
          throw new Error("static build: the helper pages weren't prepared");
        }
        for (const page of [pages.frame, pages.unsubscribe]) {
          // oxlint-disable-next-line no-await-in-loop -- two small files.
          await writeFile(path.join(outDir, page.name), page.content);
        }
        await writeFile(
          path.join(outDir, APP_MANIFEST),
          appManifest(await appVersion(builder.config.root))
        );
        const files = await listFiles(outDir);
        checkBundleFiles(files);
        const entries = await Promise.all(
          files.map(async (name) => ({
            data: new Uint8Array(await readFile(path.join(outDir, name))),
            name,
          }))
        );
        for (const entry of entries) {
          if (!/\.(?:js|html)$/u.test(entry.name)) {
            continue;
          }
          const text = new TextDecoder().decode(entry.data);
          const found = bffLeftover(text);
          if (found !== undefined) {
            throw new Error(
              `static build: ${entry.name} still calls ${found}, which only the BFF has`
            );
          }
        }
        const zip = createZip(entries);
        const zipFile = path.join(outDir, "..", ZIP_NAME);
        await writeFile(zipFile, zip);
        const sha256 = createHash("sha256").update(zip).digest("hex");
        await writeFile(`${zipFile}.sha256`, `${sha256}  ${ZIP_NAME}\n`);
        builder.config.logger.info(
          `static build: ${path.relative(builder.config.root, zipFile)} (${files.length} files, sha256 ${sha256})`
        );
      },
      order: "post",
    },
    enforce: "post",
    name: "nebula:static-bundle",
  };
};
