// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Links in the rich-text editor: only http, https and mailto (the same
// protocols the reader opens).

const ALLOWED = new Set(["http:", "https:", "mailto:"]);
const SCHEME = /^[a-z][\d+.a-z-]*:/iu;
const EMAIL = /^[^\s@/:]+@[^\s@/:]+\.[^\s@/:]+$/u;

/** Whether the editor may keep this href. */
export const isAllowedLink = (href: string): boolean => {
  try {
    return ALLOWED.has(new URL(href).protocol);
  } catch {
    return false;
  }
};

/**
 * What the user typed in the link field → an href, or null if it isn't a
 * usable link. "example.com" → "https://example.com/", "ana@x.com" →
 * "mailto:ana@x.com".
 */
export const normalizeLink = (input: string): string | null => {
  const text = input.trim();
  if (text === "" || /\s/u.test(text)) {
    return null;
  }
  let candidate = `https://${text}`;
  if (SCHEME.test(text) && !/^[^:]+:\d/u.test(text)) {
    candidate = text;
  } else if (EMAIL.test(text)) {
    candidate = `mailto:${text}`;
  }
  try {
    const url = new URL(candidate);
    if (!ALLOWED.has(url.protocol)) {
      return null;
    }
    if (url.protocol !== "mailto:" && !url.hostname.includes(".")) {
      // "https://foo" is almost always a typo, not a link.
      return url.hostname === "localhost" ? url.href : null;
    }
    return url.href;
  } catch {
    return null;
  }
};
