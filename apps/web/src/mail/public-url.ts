// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where the static build may send a one-click unsubscribe from the browser
// (static/unsubscribe-page.ts), checked by the app and again by the page.

/**
 * An https URL on a public host name: default port, no credentials, no IP
 * address (the URL parser already turns every IPv4 spelling into dotted
 * form), no single-label or local name. The browser can't resolve names,
 * so this is as far as a page can check. Self-contained on purpose: its
 * source also runs inside unsubscribe.html.
 */
export const isPublicHttpsUrl = (value: string): boolean => {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  const host = url.hostname.toLowerCase().replace(/\.$/u, "");
  const local = [
    "localhost",
    "local",
    "internal",
    "lan",
    "home.arpa",
    "test",
    "invalid",
    "example",
    "onion",
  ];
  return (
    url.protocol === "https:" &&
    url.username === "" &&
    url.password === "" &&
    url.port === "" &&
    host.includes(".") &&
    !host.startsWith("[") &&
    !/^\d+\.\d+\.\d+\.\d+$/u.test(host) &&
    !local.some((name) => host === name || host.endsWith(`.${name}`))
  );
};
