// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The sign-in check of the app's sections (/mail, /contacts, /calendar,
// /tasks, /settings), shared by all of them: one check per page load.
//
// Only a convenience (go to the login instead of an empty screen): the
// server checks the session on every token, proxy and JMAP request. The
// check runs on every server render, and in the browser until it passes
// once; after that an expired session shows up as a 401 from the token
// endpoint and SessionWatch sends the user to the login. Logging out and
// an expired session both leave with a full page load
// (lib/session-expired.ts, top-bar signOut), which forgets this flag.

import { redirect } from "@tanstack/react-router";

import { getAuthState } from "./auth";

let checkedInBrowser = false;
/** Concurrent checks (a navigation and link preloads) share one request. */
let pending: Promise<boolean> | undefined;

const inBrowser = (): boolean => typeof document !== "undefined";

/**
 * The server render of this page already checked the session (router.tsx
 * `hydrate`): the browser doesn't ask again on its first navigation.
 */
export const markAuthChecked = (): void => {
  if (inBrowser()) {
    checkedInBrowser = true;
  }
};

type AuthCheck = () => Promise<{ readonly authenticated: boolean }>;

const isAuthenticated = async (check: AuthCheck): Promise<boolean> => {
  if (!inBrowser()) {
    // On the server: a check per request, never shared between users.
    const { authenticated } = await check();
    return authenticated;
  }
  pending ??= check()
    .then(({ authenticated }) => authenticated)
    .finally(() => {
      pending = undefined;
    });
  return await pending;
};

/**
 * For a section's `beforeLoad` (its result is the route context): to the
 * login if nobody is signed in. `authChecked` tells router.tsx that this
 * server render checked the session.
 */
export const requireAuth = async (
  location: { readonly href: string },
  /** The session check (tests pass their own). */
  check: AuthCheck = getAuthState
): Promise<{ readonly authChecked: true }> => {
  if (inBrowser() && checkedInBrowser) {
    return { authChecked: true };
  }
  if (!(await isAuthenticated(check))) {
    throw redirect({ search: { returnTo: location.href }, to: "/login" });
  }
  markAuthChecked();
  return { authChecked: true };
};

/** For tests: a new page. */
export const resetAuthCheck = (): void => {
  checkedInBrowser = false;
  pending = undefined;
};
