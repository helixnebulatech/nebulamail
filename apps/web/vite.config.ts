// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { randomBytes } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";

import { paraglideVitePlugin } from "@inlang/paraglide-js";
import stylex from "@stylexjs/unplugin/vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import { varlockVitePlugin } from "@varlock/vite-integration";
import viteReact from "@vitejs/plugin-react";
import { FontaineTransform } from "fontaine";
import { defineConfig } from "vite";

import { FRAME_PAGE } from "./src/lib/frame-page.ts";
import { oauthScope, parseOAuthServices } from "./src/lib/oauth-scope.ts";
import { serviceWorker } from "./src/pwa/plugin.ts";
import { staticProxyOrigin } from "./src/static/options.ts";
import { staticBasePath, staticBundle } from "./src/static/plugin.ts";

/**
 * Build target: "bff" (default, with our server) or
 * "static" (`bun run build:static`: a zip that Stalwart serves itself).
 */
const STATIC = process.env.NEBULA_TARGET === "static";
/** Where Stalwart mounts the static build (its Application `urlPrefix`). */
const STATIC_BASE = STATIC ? staticBasePath(process.env.NEBULA_BASE_PATH) : "/";
/** The image proxy of preloaded images (option D), normalized. */
const IMAGE_PROXY_ORIGIN = STATIC
  ? staticProxyOrigin(process.env.NEBULA_IMAGE_PROXY_ORIGIN)
  : "";

/**
 * Static build: the style nonce of its index.html (router.tsx). Kept in the
 * environment because Vite loads this file more than once per build (the
 * prerender too); set it by hand to rebuild the same zip byte for byte.
 */
if (STATIC) {
  // `||=`, not `??=`: an empty NEBULA_STYLE_NONCE means "not set" too.
  // oxlint-disable-next-line typescript/prefer-nullish-coalescing
  process.env.NEBULA_STYLE_NONCE ||= randomBytes(16).toString("base64url");
}
const STYLE_NONCE = STATIC ? (process.env.NEBULA_STYLE_NONCE ?? "") : "";

/**
 * Where fontaine reads Geist's metrics (fonts.css points to the files by
 * package name, which only packages/ui resolves). Only the latin files,
 * upright and italic: the metrics average the widths of an ASCII sample the
 * other subsets don't have. fontaine adds one fallback per @font-face it
 * can read and copies no `unicode-range`, so leaving the other subsets
 * unresolved (no metrics, no fallback) gives one fallback per style instead
 * of five identical ones.
 */
const requireFromUi = createRequire(
  path.resolve(import.meta.dirname, "../../packages/ui/package.json")
);
const GEIST_LATIN =
  /^@fontsource-variable\/geist\/files\/geist-latin-wght-(?:normal|italic)\.woff2$/u;
const geistMetricsFile = (source: string): string =>
  GEIST_LATIN.test(source) ? requireFromUi.resolve(source) : source;

export default defineConfig(({ command }) => ({
  base: STATIC ? STATIC_BASE : "/",
  // Apart from the BFF build's dist/, so one never overwrites the other.
  build: STATIC ? { outDir: "dist-static" } : {},
  // Replaced in the code, so the other target's code is dropped.
  define: {
    "import.meta.env.NEBULA_STATIC": JSON.stringify(STATIC),
    // Static build only (docs/static.md): fixed when the zip is built.
    "import.meta.env.NEBULA_OAUTH_CLIENT_ID": JSON.stringify(
      process.env.NEBULA_OAUTH_CLIENT_ID ?? "nebulamail"
    ),
    // The scopes registered for the static client (docs/static.md); an
    // unknown name fails the build.
    "import.meta.env.NEBULA_OAUTH_SCOPE": JSON.stringify(
      STATIC
        ? oauthScope(parseOAuthServices(process.env.NEBULA_OAUTH_SERVICES))
        : ""
    ),
    "import.meta.env.NEBULA_STYLE_NONCE": JSON.stringify(STYLE_NONCE),
    "import.meta.env.NEBULA_IMAGE_PROXY_ORIGIN":
      JSON.stringify(IMAGE_PROXY_ORIGIN),
    // The static zip's helper pages, with a hash in the name: the static
    // plugin sets them (static/plugin.ts `helperPageDefines`). With our
    // server the frame page is a route (routes/email-frame[.]html.ts), not
    // served as immutable, so it keeps its plain name; there is no
    // unsubscribe page (the BFF has /api/unsubscribe).
    ...(STATIC
      ? {}
      : {
          "import.meta.env.NEBULA_FRAME_PAGE": JSON.stringify(FRAME_PAGE),
          "import.meta.env.NEBULA_UNSUBSCRIBE_PAGE": "null",
        }),
  },
  // The session worker (session/worker.ts) is an ES module.
  worker: { format: "es" },
  server: {
    port: 3001,
  },
  resolve: {
    tsconfigPaths: true,
    alias: {
      // The session layer: the BFF's cookie, or OAuth in the browser.
      "#session": path.resolve(
        import.meta.dirname,
        STATIC ? "src/session/browser.ts" : "src/session/bff.ts"
      ),
    },
  },
  // oxlint-disable-next-line typescript/no-unsafe-assignment -- @stylexjs/unplugin types its Vite plugin as `any`.
  plugins: [
    varlockVitePlugin({ ssrInjectMode: "auto-load" }),
    // i18n (TanStack Start's official Paraglide example). The language comes
    // from the small preferences cookie, then the browser, then English; it
    // isn't part of the URL (an app, not pages to index).
    paraglideVitePlugin({
      cookieName: "wm-locale",
      outdir: "./src/paraglide",
      // One module per message in builds (the bundler keeps only the
      // ones a chunk uses); one per language with `vite dev`, which serves
      // every module as its own request: per message, a reload asked for
      // ~1,500 files just for texts.
      // Paraglide's other bundler plugins do the same. Both write
      // messages.js and runtime.js, the only files the code imports.
      outputStructure:
        command === "serve" ? "locale-modules" : "message-modules",
      project: "./project.inlang",
      strategy: ["cookie", "preferredLanguage", "baseLocale"],
    }),
    // Client and SSR builds share the collected StyleX rules through disk.
    // `css-only` dev mode serves the CSS as a stylesheet link instead of
    // injecting a <style> without nonce, so the strict CSP also works in dev.
    // Its link is refreshed by src/dev/stylex-refresh.ts (no flash).
    stylex({ devMode: "css-only", devPersistToDisk: true, useCSSLayers: true }),
    // A local fallback font with Geist's metrics (size-adjust, ascent/
    // descent overrides) next to fonts.css's @font-face, so the text barely
    // moves when Geist arrives (font-display: swap). The family is
    // "Geist Variable fallback", listed in tokens.stylex.ts `font.sans`
    // (fontaine can't see into StyleX's variables). `fallbacks: {}`: its
    // sans-serif system fonts (Segoe UI, Helvetica Neue, Arial, Noto Sans,
    // Liberation Sans).
    FontaineTransform.vite({
      fallbacks: {},
      resolvePath: geistMetricsFile,
    }),
    tanstackStart(
      STATIC
        ? {
            router: { basepath: STATIC_BASE },
            // One index.html for every path (Stalwart serves it for any
            // path under the mount that isn't a file).
            spa: { enabled: true, prerender: { outputPath: "/index.html" } },
          }
        : {}
    ),
    // React Compiler running in Rust (oxc-transform-react). Experimental:
    // if it breaks, fall back to babel-plugin-react-compiler.
    viteReact({ compiler: true }),
    // /sw.js for the installable app, production builds only.
    serviceWorker({ base: STATIC ? STATIC_BASE : "/", static: STATIC }),
    // CSP in <meta> with the hashes of the inline scripts, then the zip.
    ...(STATIC
      ? [
          staticBundle({
            base: STATIC_BASE,
            imageProxyOrigin: IMAGE_PROXY_ORIGIN,
            styleNonce: STYLE_NONCE,
          }),
        ]
      : []),
  ],
}));
