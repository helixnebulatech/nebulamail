// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// CSS URL handling on top of Proton's escaping.
// Proton turns every `url(` into `proton-url(` after decoding escapes and
// comments. Here we (1) re-check the decoded CSS for anything that could
// still load a resource, (2) find the `proton-url(` references and their
// context, and (3) rewrite them to `url("<allowed>")` or `none`. DOM-free.

import {
  neutralizeStyleBreakout,
  recurringUnescapeCSSEncoding,
} from "../vendor/proton-sanitize/src/escape";

export type CssUrlContext = "image" | "import" | "font";

export interface CssUrlRef {
  readonly start: number;
  readonly end: number;
  readonly url: string;
  readonly context: CssUrlContext;
}

// A literal (no backtracking), searched on the original string so its
// indices are the original's (`toLowerCase` can change the length: İ).
const PROTON_URL = /proton-url\(/giu;

// Functions and at-rules that can fetch something, when NOT already
// prefixed by Proton (lookbehind excludes `proton-url(`).
const RESIDUAL_LOADERS =
  /(?<![\w-])(?<fn>url|image-set|-webkit-image-set|image|cross-fade|-webkit-cross-fade|src|element|-moz-element|paint|expression)(?<open>\s*\()/giu;
const IMPORT_RULE = /@import\b/giu;
const LEGACY_BINDINGS =
  /(?<![\w-])(?<fn>behavior|-moz-binding)(?<open>\s*:)/giu;

/**
 * `css` without comments, in linear time (the usual lazy regex is
 * quadratic on many unclosed `/*`). An unclosed comment runs to the end
 * (CSS Syntax §4.3.2).
 */
export const stripCssComments = (css: string): string => {
  let out = "";
  let from = 0;
  let open = css.indexOf("/*");
  while (open !== -1) {
    out += css.slice(from, open);
    const close = css.indexOf("*/", open + 2);
    if (close === -1) {
      return out;
    }
    from = close + 2;
    open = css.indexOf("/*", from);
  }
  return out + css.slice(from);
};

/**
 * More unclosed `/*` than this after Proton's decoding makes its vendored
 * comment regex (`escapeURLinStyle`, which we may not modify) too slow:
 * its cost is about (unclosed openers) × length.
 */
const MAX_UNCLOSED_COMMENTS = 16;

/**
 * Whether Proton's `escapeURLinStyle` would be quadratic on `css`: too
 * many `/*` after the last `*\/` of the decoded text. Such CSS is refused
 * (the browser shows the text part; the server extracts nothing).
 */
export const cssCommentHazard = (css: string): boolean => {
  if (!css.includes("/")) {
    return false;
  }
  const decoded = recurringUnescapeCSSEncoding(css);
  let i = Math.max(0, decoded.lastIndexOf("*/"));
  let count = 0;
  i = decoded.indexOf("/*", i);
  while (i !== -1) {
    count += 1;
    if (count > MAX_UNCLOSED_COMMENTS) {
      return true;
    }
    i = decoded.indexOf("/*", i + 1);
  }
  return false;
};

const hasResidualLoader = (css: string): boolean => {
  RESIDUAL_LOADERS.lastIndex = 0;
  IMPORT_RULE.lastIndex = 0;
  LEGACY_BINDINGS.lastIndex = 0;
  return (
    RESIDUAL_LOADERS.test(css) ||
    IMPORT_RULE.test(css) ||
    LEGACY_BINDINGS.test(css)
  );
};

/**
 * Defence in depth after Proton: decode escapes and drop comments the way
 * the CSS parser would, and if anything that loads resources is left
 * (including Proton's own `@import proton-url(…)`), return the decoded CSS
 * with those functions renamed so they are invalid. Otherwise return the
 * input unchanged. Returns "" when the input is escaped too deeply to decode.
 */
export const neutralizeCss = (css: string): string => {
  const decoded = recurringUnescapeCSSEncoding(css);
  if (decoded === "" && css !== "") {
    return "";
  }
  const plain = stripCssComments(decoded);
  if (!hasResidualLoader(plain)) {
    return css;
  }
  return plain
    .replace(RESIDUAL_LOADERS, "blocked-$<fn>$<open>")
    .replace(IMPORT_RULE, "@blocked-import")
    .replace(LEGACY_BINDINGS, "blocked-$<fn>$<open>");
};

/** Character ranges of `@font-face { … }` blocks (strings are skipped). */
const fontFaceRanges = (css: string): [number, number][] => {
  const ranges: [number, number][] = [];
  const re = /@font-face\b/giu;
  let match = re.exec(css);
  while (match !== null) {
    const open = css.indexOf("{", match.index);
    if (open === -1) {
      ranges.push([match.index, css.length]);
      break;
    }
    let depth = 0;
    let end = css.length;
    let quote: string | null = null;
    for (let i = open; i < css.length; i += 1) {
      const ch = css[i];
      if (quote !== null) {
        if (ch === "\\") {
          i += 1;
        } else if (ch === quote) {
          quote = null;
        }
      } else if (ch === '"' || ch === "'") {
        quote = ch;
      } else if (ch === "{") {
        depth += 1;
      } else if (ch === "}") {
        depth -= 1;
        if (depth === 0) {
          end = i + 1;
          break;
        }
      }
    }
    ranges.push([match.index, end]);
    re.lastIndex = end;
    match = re.exec(css);
  }
  return ranges;
};

const readArgument = (
  css: string,
  from: number
): { url: string; end: number } => {
  let i = from;
  while (i < css.length && /\s/u.test(css[i] ?? "")) {
    i += 1;
  }
  const quote = css[i];
  let url = "";
  if (quote === '"' || quote === "'") {
    i += 1;
    while (i < css.length && css[i] !== quote) {
      if (css[i] === "\\" && i + 1 < css.length) {
        i += 1;
      }
      url += css[i] ?? "";
      i += 1;
    }
    i += 1;
  } else {
    while (i < css.length && css[i] !== ")" && !/\s/u.test(css[i] ?? "")) {
      url += css[i] ?? "";
      i += 1;
    }
  }
  const close = css.indexOf(")", i);
  return { end: close === -1 ? css.length : close + 1, url: url.trim() };
};

const IMPORT_BEFORE = /@(?:blocked-)?import\s*$/iu;

/**
 * Every `proton-url(…)` reference with its context. The `@font-face`
 * ranges are sorted and disjoint and the references come in increasing
 * order, so one pointer walks both (linear, not references × fonts).
 */
export const findCssUrls = (css: string): CssUrlRef[] => {
  const refs: CssUrlRef[] = [];
  const fonts = fontFaceRanges(css);
  let font = 0;
  const re = new RegExp(PROTON_URL.source, "giu");
  let match = re.exec(css);
  while (match !== null) {
    const start = match.index;
    const { url, end } = readArgument(css, start + match[0].length);
    while (font < fonts.length && (fonts[font]?.[1] ?? 0) <= start) {
      font += 1;
    }
    const range = fonts[font];
    let context: CssUrlContext = "image";
    const before = css.slice(Math.max(0, start - 32), start);
    if (before.includes("@") && IMPORT_BEFORE.test(before)) {
      context = "import";
    } else if (range !== undefined && start >= range[0]) {
      context = "font";
    }
    refs.push({ context, end, start, url });
    re.lastIndex = end;
    match = re.exec(css);
  }
  return refs;
};

/** A CSS `url("…")` literal that cannot break out of the string or the style element. */
const mustEscape = (ch: string, code: number): boolean =>
  ch === '"' ||
  ch === "\\" ||
  ch === "<" ||
  ch === ">" ||
  code < 0x20 ||
  code === 0x7f;

export const cssUrl = (url: string): string => {
  let escaped = "";
  for (const ch of url) {
    const code = ch.codePointAt(0) ?? 0;
    escaped += mustEscape(ch, code) ? `\\${code.toString(16)} ` : ch;
  }
  return `url("${escaped}")`;
};

/**
 * Replaces every `proton-url(…)` reference: with `url("<value>")` when
 * `decide` returns a string, otherwise with `none` (nothing to load).
 */
export const rewriteCssUrls = (
  css: string,
  decide: (ref: CssUrlRef) => string | null
): string => {
  const refs = findCssUrls(css);
  if (refs.length === 0) {
    return css;
  }
  let out = "";
  let last = 0;
  for (const ref of refs) {
    const replacement = decide(ref);
    out += css.slice(last, ref.start);
    out += replacement === null ? "none" : cssUrl(replacement);
    last = ref.end;
  }
  out += css.slice(last);
  return out;
};

/** Final step for `<style>` text: a literal `</` must not close the element. */
export const finalizeStyleText = (css: string): string =>
  neutralizeStyleBreakout(css);
