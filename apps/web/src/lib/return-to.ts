// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The one `returnTo` check (no open redirect), shared by the
// BFF login (server/auth/handlers.ts) and the static version
// (session/oauth.ts). Pure: no imports, runs anywhere.

const BASE = "https://app.invalid";

// Control characters (C0, DEL, C1) never belong in a path we go back to.
// oxlint-disable-next-line no-control-regex -- that is what it looks for.
const CONTROL = /[\u0000-\u001F\u007F-\u009F]/u;

/**
 * `returnTo` only accepts paths inside the app. Paths are the router's,
 * without the base path. `//evil.com`, `/\evil.com`, dot segments that
 * normalize into `//host` (`/.//evil.com`) and absolute URLs fall back to `/`.
 */
export const safeReturnTo = (value: string | null | undefined): string => {
  if (
    typeof value !== "string" ||
    !value.startsWith("/") ||
    value.startsWith("//") ||
    value.includes("\\") ||
    // Before parsing: the URL parser drops tabs and newlines ("/\t/evil").
    CONTROL.test(value)
  ) {
    return "/";
  }
  try {
    const url = new URL(value, BASE);
    // Dot segments can normalize into "//host" ("/.//evil.example").
    return url.origin === BASE &&
      !url.pathname.startsWith("//") &&
      !url.pathname.includes("\\")
      ? `${url.pathname}${url.search}${url.hash}`
      : "/";
  } catch {
    return "/";
  }
};
