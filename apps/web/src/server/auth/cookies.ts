// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `__Host-` cookies: Secure, Path=/, no Domain, so no other subdomain (like
// the image proxy origin) can set or read them. Browsers accept
// Secure cookies on http://localhost, so development uses the same names.

export const SESSION_COOKIE = "__Host-wm-session";
export const LOGIN_COOKIE = "__Host-wm-login";
/**
 * JMAP_PROXY=on only: SameSite=Strict, required next to the session cookie
 * on every /jmap-proxy/ request (server/auth/handlers.ts, hasProxyBinding).
 */
export const PROXY_COOKIE = "__Host-wm-proxy";

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * The cookie name for this app's origin. Browsers share cookies between the
 * ports of one host, so two copies of the app on localhost (one against
 * Stalwart, one against the bridge) would overwrite each other's session
 * and sign each other out. On localhost the port is part of the name; any
 * other host keeps the plain name.
 */
export const cookieName = (base: string, appUrl: URL): string =>
  LOCAL_HOSTS.has(appUrl.hostname) && appUrl.port !== ""
    ? `${base}-${appUrl.port}`
    : base;

export const parseCookies = (
  header: string | null
): ReadonlyMap<string, string> => {
  const cookies = new Map<string, string>();
  for (const part of header?.split(";") ?? []) {
    const separator = part.indexOf("=");
    if (separator > 0) {
      const name = part.slice(0, separator).trim();
      // First value wins, like browsers send the most specific cookie first.
      if (!cookies.has(name)) {
        cookies.set(name, part.slice(separator + 1).trim());
      }
    }
  }
  return cookies;
};

/** Our values are base64url and dots only, so they never need escaping. */
export const serializeCookie = (
  name: string,
  value: string,
  maxAgeSeconds: number,
  sameSite: "Lax" | "Strict" = "Lax"
): string => {
  if (!/^[\w.-]*$/u.test(value)) {
    throw new Error("Cookie value must be base64url");
  }
  return [
    `${name}=${value}`,
    `Max-Age=${Math.max(0, Math.floor(maxAgeSeconds))}`,
    "Path=/",
    "Secure",
    "HttpOnly",
    `SameSite=${sameSite}`,
  ].join("; ");
};

export const clearCookie = (
  name: string,
  sameSite: "Lax" | "Strict" = "Lax"
): string => serializeCookie(name, "", 0, sameSite);
