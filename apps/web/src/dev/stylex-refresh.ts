// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Development only (__root.tsx loads it when import.meta.env.DEV): keeps
// StyleX's dev stylesheet up to date without a flash of unstyled page.
//
// StyleX's own `css-only` client changes the `href` of the stylesheet link
// in place; the browser drops the old sheet at once and the page is
// unstyled (white, black text) until the new one arrives. Here the new link
// is added next to the old one, and the old one is removed only once the
// new one has loaded, like Vite does with its CSS. Still no inline <style>,
// so the strict CSP works in dev too.

const CSS_PATH = "/virtual:stylex.css";
/** Marks the links this script added (the others belong to React). */
const OWNED = "data-stylex-refresh";

let generation = 0;

const stylesheets = (): HTMLLinkElement[] =>
  [
    ...document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]'),
  ].filter((link) => new URL(link.href, location.origin).pathname === CSS_PATH);

const refresh = () => {
  const latest = stylesheets().at(-1);
  if (latest === undefined) {
    return;
  }
  generation += 1;
  const mine = generation;
  const next = document.createElement("link");
  next.rel = "stylesheet";
  next.href = `${CSS_PATH}?t=${Date.now()}`;
  next.setAttribute(OWNED, "");
  const settle = () => {
    if (mine !== generation) {
      // A newer refresh started meanwhile; it will clean up.
      next.remove();
      return;
    }
    // The newest sheet is in: remove our older ones, and switch off the
    // server-rendered one (React owns that node, so it stays in place).
    for (const link of stylesheets()) {
      if (link === next) {
        continue;
      }
      if (link.hasAttribute(OWNED)) {
        link.remove();
      } else {
        link.disabled = true;
      }
    }
  };
  next.addEventListener("load", settle, { once: true });
  next.addEventListener("error", settle, { once: true });
  latest.after(next);
};

// The server-rendered link may be older than the rules collected since.
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", refresh, { once: true });
} else {
  refresh();
}

import.meta.hot?.on("stylex:css-update", refresh);
