// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * The page's CSP nonce, in the browser. TanStack Start puts it in
 * <meta property="csp-nonce">. Used by Base UI's injected styles and by the
 * email iframes (srcdoc inherits the page CSP, so their <style> need it).
 */
export const readCspNonce = (): string | undefined => {
  const content = document
    .querySelector('meta[property="csp-nonce"]')
    ?.getAttribute("content");
  return content === null || content === "" ? undefined : content;
};
