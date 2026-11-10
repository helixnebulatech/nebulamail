// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// First: zod must be configured before any schema is created (zod-config.ts).
import "./lib/zod-config";
import { installTrustedTypes } from "@nebulamail/mail-render/trusted-types";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRouter as createTanStackRouter } from "@tanstack/react-router";
import type { ParsedLocation } from "@tanstack/react-router";
import { createIsomorphicFn } from "@tanstack/react-start";

import Loader from "./components/loader";
import { ErrorPage, NotFoundPage } from "./components/status-page";
import { ENV } from "./env";
import { setJmapProxyMode } from "./lib/auth";
import { markAuthChecked } from "./lib/auth-guard";
import type { RouterAppContext } from "./routes/__root";
import { routeTree } from "./routeTree.gen";
import { createNonce } from "./server/security-headers";

// A new CSP nonce for every server render (TanStack Start's CSP pattern). The
// router adds it to the scripts and styles it renders; __root.tsx puts it in
// the Content-Security-Policy header. In the browser this returns undefined.
// Trusted Types: the `default` policy exists before any
// editor or library writes HTML. Only in the browser.
// In the static build, scripts and workers only from the build's own files:
// Stalwart serves attachments on this origin too.
if (typeof window !== "undefined") {
  const base = import.meta.env.BASE_URL;
  installTrustedTypes(
    import.meta.env.NEBULA_STATIC && import.meta.env.PROD
      ? { scriptUrlPaths: [`${base}assets/`, `${base}sw.js`] }
      : {}
  );
}

// The static build's index.html is rendered once, at build time: its style
// nonce is fixed (static/plugin.ts); scripts go by hash.
const getSsrOptions = createIsomorphicFn().server(() => ({
  nonce: import.meta.env.NEBULA_STATIC
    ? import.meta.env.NEBULA_STYLE_NONCE
    : createNonce(),
}));

/**
 * Full BFF mode (JMAP_PROXY=on)? Server only: the browser
 * learns it from the page (`dehydrate` below). Never in the static build.
 */
const serverJmapProxy = createIsomorphicFn().server(
  (): boolean => !import.meta.env.NEBULA_STATIC && ENV.JMAP_PROXY === "on"
);

/** What the server render tells the browser besides the routes' data. */
interface Dehydrated {
  /** Full BFF mode: the grant comes from the proxy (session/bff.ts). */
  readonly jmapProxy: boolean;
  /** This render checked the session of a signed-in section (auth-guard). */
  readonly signedIn: boolean;
}

/** The app section of a path: "mail", "contacts", "calendar", "settings"… */
const section = (pathname: string): string => pathname.split("/")[1] ?? "";

/** Sections with the mail shell: their loaders prefetch its folders. */
const FOLDER_SECTIONS = new Set(["contacts", "mail", "settings"]);

/** The folders of `mailbox`'s account (own without one), asked early. */
const startFolders = async (mailbox: string | undefined): Promise<void> => {
  const [{ startMailboxesEarly }, { parseMailboxParam }] = await Promise.all([
    import("./mail/queries"),
    import("./mail/account-scope"),
  ]);
  startMailboxesEarly(
    mailbox === undefined ? null : parseMailboxParam(mailbox).accountId
  );
};

/**
 * Right after the server render, the browser starts what the page will
 * need anyway: no second sign-in check on the first
 * navigation, the JMAP session (grant + /jmap/session) in parallel with
 * React's hydration instead of after it, and the shell's folders right
 * after the session (the router doesn't run the loaders of a server
 * render again, so they would wait for the screen to hydrate). Failures
 * are left to the queries that use the client (they retry and
 * SessionWatch handles an expiry).
 */
const hydrateFromServer = (
  { jmapProxy, signedIn }: Dehydrated,
  pathname: string,
  /** The `$mailbox` of a /mail/$mailbox path, if any. */
  mailbox: string | undefined
): void => {
  setJmapProxyMode(jmapProxy);
  if (!signedIn) {
    return;
  }
  markAuthChecked();
  void import("./mail/client")
    .then(async ({ getClient }) => await getClient())
    .catch(() => null);
  if (FOLDER_SECTIONS.has(section(pathname))) {
    startFolders(mailbox).catch(() => null);
  }
};

/**
 * View transitions only when the section changes: a short
 * fade-in of the new screen (styles/app.css). Inside a section (folders,
 * opening a message, J/K) navigation stays instant.
 */
const viewTransition = {
  types: ({
    fromLocation,
    toLocation,
  }: {
    readonly fromLocation?: ParsedLocation;
    readonly toLocation: ParsedLocation;
  }): string[] | false =>
    fromLocation !== undefined &&
    section(fromLocation.pathname) !== section(toLocation.pathname)
      ? ["nb-section"]
      : false,
};

/**
 * Without transition types the router would animate every navigation (J/K
 * included) with the browser's default cross-fade: then no transitions.
 */
const supportsTransitionTypes = (): boolean =>
  typeof CSS !== "undefined" &&
  CSS.supports("selector(:active-view-transition-type(a))");

export const getRouter = () => {
  // Mail data is fetched in the browser only (the token lives in memory);
  // a new client per router keeps users apart during SSR.
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        // Push (JMAP state changes) refetches what changed.
        refetchOnWindowFocus: false,
        retry: 1,
        staleTime: 30_000,
      },
    },
  });
  const router = createTanStackRouter({
    routeTree,
    scrollRestoration: true,
    // Hovering or focusing a link loads its route's code and runs its
    // loader (which only warms the React Query cache, so the router's own
    // cache stays off: defaultPreloadStaleTime 0, as TanStack recommends
    // with an external cache).
    defaultPreload: "intent",
    defaultPreloadStaleTime: 0,
    ssr: getSsrOptions() ?? {},
    context: { queryClient } satisfies RouterAppContext,
    Wrap: ({ children }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    ),
    defaultPendingComponent: () => <Loader />,
    defaultViewTransition: supportsTransitionTypes() ? viewTransition : false,
    defaultNotFoundComponent: NotFoundPage,
    defaultErrorComponent: ErrorPage,
    dehydrate: (): Dehydrated => ({
      jmapProxy: serverJmapProxy() ?? false,
      signedIn: router.state.matches.some(
        ({ context }) => "authChecked" in context && context.authChecked
      ),
    }),
    hydrate: (dehydrated) => {
      const { pathname } = router.latestLocation;
      const [, params] = router.getMatchedRoutes(pathname);
      hydrateFromServer(dehydrated, pathname, params.mailbox);
    },
  });

  return router;
};

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
