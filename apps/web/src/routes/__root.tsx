// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { UIProvider, themeProps } from "@nebulamail/ui";
import { GEIST_LATIN_WOFF2 } from "@nebulamail/ui/font-files";
import type { QueryClient } from "@tanstack/react-query";
import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRouteWithContext,
  useRouter,
} from "@tanstack/react-router";

import { SessionWatch } from "../components/session-watch";
import { ENV } from "../env";
import { readCspNonce } from "../lib/csp-nonce";
import { readPreferences } from "../lib/preferences";
import { getLocale } from "../paraglide/runtime.js";
import { ServiceWorker } from "../pwa/service-worker";
import { buildCsp } from "../server/security-headers";

import appCss from "../styles/app.css?url";
import editorCss from "@nebulamail/ui/editor.css?url";
import fontsCss from "@nebulamail/ui/fonts.css?url";

/** "/" or, in the static build, where Stalwart serves it ("/nebulamail/"). */
const BASE = import.meta.env.BASE_URL;

export interface RouterAppContext {
  readonly queryClient: QueryClient;
}

/**
 * The browser / installed-app bar takes the Helix canvas colour: the forced
 * theme, or light and dark by media query when it follows the system.
 */
const THEME_COLOR = { dark: "#06070C", light: "#F6F6F9" } as const;
const themeColor = (theme: "dark" | "light" | "system") =>
  theme === "system"
    ? (["light", "dark"] as const).map((mode) => ({
        content: THEME_COLOR[mode],
        media: `(prefers-color-scheme: ${mode})`,
        name: "theme-color",
      }))
    : [{ content: THEME_COLOR[theme], name: "theme-color" }];

// The theme comes from the preferences cookie, rendered on the server, so
// the first paint already has the right colours.
const RootDocument = () => {
  // On the server the router has it; in the browser it's in the page.
  const nonce =
    useRouter().options.ssr?.nonce ??
    (typeof document === "undefined" ? undefined : readCspNonce());
  const { preferences } = Route.useRouteContext();
  return (
    <html
      lang={getLocale()}
      {...themeProps(preferences.theme, preferences.density)}
    >
      <head>
        <HeadContent />
      </head>
      <body>
        <UIProvider {...(nonce === undefined ? {} : { nonce })}>
          <Outlet />
          <ServiceWorker />
          <SessionWatch />
        </UIProvider>
        {import.meta.env.DEV ? (
          <script
            nonce={nonce}
            src="/src/dev/stylex-refresh.ts"
            type="module"
          />
        ) : null}
        <Scripts />
      </body>
    </html>
  );
};

export const Route = createRootRouteWithContext<RouterAppContext>()({
  beforeLoad: () => ({ preferences: readPreferences() }),
  component: RootDocument,
  // Page responses only; start.ts adds the headers every response gets.
  headers: ({ ssr }) => {
    const nonce = ssr?.nonce;
    // Without a nonce (client navigation) there is no response to decorate.
    // The static build's CSP is a <meta> in index.html (static/plugin.ts).
    return nonce === undefined || import.meta.env.NEBULA_STATIC
      ? {}
      : {
          // Pages may contain the user's data: never in a shared cache.
          "cache-control": "private, no-store",
          "content-security-policy": buildCsp({
            appUrl: ENV.APP_URL,
            dev: import.meta.env.DEV,
            https: new URL(ENV.APP_URL).protocol === "https:",
            imageProxyOrigin: ENV.IMAGE_PROXY_ORIGIN,
            jmapOrigin: ENV.JMAP_SERVER,
            // Checked strictly when the server starts (server.ts).
            jmapProxy: ENV.JMAP_PROXY === "on",
            nonce,
          }),
        };
  },
  head: ({ match }) => ({
    links: [
      // ?v=: browsers keep favicons for a long time; bump it with a new icon
      // (scripts/generate-icons.ts, manifest.webmanifest).
      { href: `${BASE}favicon.svg?v=2`, rel: "icon", type: "image/svg+xml" },
      // Installable app.
      { href: `${BASE}manifest.webmanifest`, rel: "manifest" },
      {
        href: `${BASE}icons/apple-touch-icon.png?v=2`,
        rel: "apple-touch-icon",
      },
      // The font starts downloading with the page, not after fonts.css is
      // read (a later swap moves text less, too: fonts.css has a fallback
      // with Geist's metrics, fontaine in vite.config.ts). crossOrigin is
      // required for font preloads, even same-origin ones (fonts are
      // fetched in CORS mode; without it the preload is downloaded twice).
      {
        as: "font",
        crossOrigin: "anonymous",
        href: GEIST_LATIN_WOFF2,
        rel: "preload",
        type: "font/woff2",
      },
      { href: fontsCss, rel: "stylesheet" },
      { href: appCss, rel: "stylesheet" },
      // The compose editor's content styles (TipTap's injected <style> is
      // off: it has no CSP nonce).
      { href: editorCss, rel: "stylesheet" },
      ...(import.meta.env.DEV
        ? [{ href: "/virtual:stylex.css", rel: "stylesheet" }]
        : []),
    ],
    meta: [
      { charSet: "utf-8" },
      // viewport-fit=cover: the shell pads itself with env(safe-area-inset-*).
      {
        content: "width=device-width, initial-scale=1, viewport-fit=cover",
        name: "viewport",
      },
      ...themeColor(match.context.preferences.theme),
      { content: "no-referrer", name: "referrer" },
      { title: "Nebula Mail" },
    ],
  }),
});
