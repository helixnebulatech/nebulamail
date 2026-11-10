// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where the app goes when the session ends under it (refresh token expired
// or revoked, or a logout in another tab): the login, with a message and the
// way back. One place for every page (calendar and tasks too) and for the
// static build's session worker (session/browser.ts).

import type { QueryClient } from "@tanstack/react-query";

import { SessionExpiredError } from "../session/types";

interface PageLocation {
  readonly pathname: string;
  readonly search: string;
}

/**
 * The router's path of a page in the app: without the base path
 * ("/nebulamail/calendar" → "/calendar" in the static build). `safeReturnTo`
 * and the router work with these, and add the base themselves.
 */
export const routerPath = (
  { pathname, search }: PageLocation,
  base: string = import.meta.env.BASE_URL
): string => {
  const root = base.endsWith("/") ? base.slice(0, -1) : base;
  let path = pathname;
  if (root !== "" && pathname === root) {
    path = "/";
  } else if (root !== "" && pathname.startsWith(`${root}/`)) {
    path = pathname.slice(root.length);
  }
  return `${path}${search}`;
};

/** The login page saying the session expired, coming back to `returnTo`. */
export const expiredLoginUrl = (
  returnTo: string,
  base: string = import.meta.env.BASE_URL
): string =>
  `${base}login?error=expired&returnTo=${encodeURIComponent(returnTo)}`;

const isLoginPage = (path: string): boolean =>
  path === "/login" || path.startsWith("/login?");

let leaving = false;

/**
 * Leaves for the login (a full load: nothing of the previous session stays
 * in memory). Once per page, and not from the login itself.
 */
export const leaveForExpiredLogin = (
  location: PageLocation & {
    readonly assign: (url: string) => void;
  } = globalThis.location,
  base: string = import.meta.env.BASE_URL
): void => {
  const path = routerPath(location, base);
  if (leaving || isLoginPage(path)) {
    return;
  }
  leaving = true;
  location.assign(expiredLoginUrl(path, base));
};

/** For tests: a new page. */
export const resetLeaving = (): void => {
  leaving = false;
};

/**
 * Calls `onExpired` when a query or a mutation fails because the session
 * is gone. Returns the unsubscribe.
 */
export const watchSession = (
  queryClient: QueryClient,
  onExpired: () => void = leaveForExpiredLogin
): (() => void) => {
  const queries = queryClient.getQueryCache().subscribe((event) => {
    if (
      event.type === "updated" &&
      event.query.state.error instanceof SessionExpiredError
    ) {
      onExpired();
    }
  });
  const mutations = queryClient.getMutationCache().subscribe((event) => {
    if (
      event.type === "updated" &&
      event.mutation.state.error instanceof SessionExpiredError
    ) {
      onExpired();
    }
  });
  return () => {
    queries();
    mutations();
  };
};
