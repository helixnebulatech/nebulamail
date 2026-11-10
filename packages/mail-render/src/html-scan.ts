// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Minimal linear HTML tokenizer (tags, attribute quoting, comments and
// raw-text elements), close enough to the WHATWG tokenizer that the limits
// prefilter (html-estimate.ts) sees the tags the parser will see. It never
// restarts a failed match, so adversarial input like `<a<a<a…` stays O(n)
// (regexes such as `<[^>]*>` are quadratic on it), and it only lower-cases
// ASCII, so its indices always refer to the original string.

export interface TagToken {
  /** ASCII-lower-cased tag name. */
  readonly name: string;
  readonly closing: boolean;
  /** `/>`: only meaningful for void and foreign (svg, math) elements. */
  readonly selfClosing: boolean;
  /** The raw attribute text, for telling identical formatting tags apart. */
  readonly attributes: string;
}

/**
 * What `onTag` asks of the scanner: `"stop"` ends the scan, `"markup"`
 * parses the content of a raw-text element as markup (inside svg and math,
 * `<style>` and `<script>` are ordinary elements).
 */
export type TagAction = "continue" | "stop" | "markup";

const RAW_TEXT = new Set([
  "script",
  "style",
  "textarea",
  "title",
  "xmp",
  "iframe",
  "noembed",
  "noframes",
  // `noscript` is not here: DOMParser and jsdom run without scripting, so
  // they parse its content as HTML.
]);

/** Once open, never closed: the rest of the document is its text. */
const PLAINTEXT = "plaintext";

const isAsciiAlpha = (code: number): boolean =>
  (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
const isTagSpace = (ch: string | undefined): boolean =>
  ch === " " || ch === "\t" || ch === "\n" || ch === "\r" || ch === "\f";

/** Lower-cases ASCII letters only, like the tokenizer (`İ` stays `İ`). */
export const asciiLower = (text: string): string =>
  text.replaceAll(/[A-Z]+/gu, (upper) => upper.toLowerCase());

type AttrState =
  | "before-name"
  | "name"
  | "after-name"
  | "before-value"
  | "unquoted"
  | "quoted";

/**
 * The `>` that ends a tag whose attributes start at `from`, following the
 * tokenizer's attribute states: a quote only opens a value right after
 * `=` (`<a b=c">` is an unquoted value `c"`). -1 at EOF.
 */
const tagEnd = (
  html: string,
  from: number
): { end: number; selfClosing: boolean } => {
  let state: AttrState = "before-name";
  let quote = "";
  let solidus = false;
  for (let j = from; j < html.length; j += 1) {
    const ch = html[j];
    if (state === "quoted") {
      if (ch === quote) {
        state = "before-name";
      }
    } else if (ch === ">") {
      return { end: j, selfClosing: solidus };
    } else if (state === "unquoted") {
      if (isTagSpace(ch)) {
        state = "before-name";
      }
    } else if (state === "before-value") {
      if (ch === '"' || ch === "'") {
        quote = ch;
        state = "quoted";
      } else if (!isTagSpace(ch)) {
        state = "unquoted";
      }
    } else if (ch === "/") {
      state = "before-name";
    } else if (isTagSpace(ch)) {
      if (state === "name") {
        state = "after-name";
      }
    } else if (ch === "=" && state !== "before-name") {
      state = "before-value";
    } else {
      state = "name";
    }
    // `/` right before `>` (outside values) marks a self-closing tag.
    solidus = ch === "/" && state === "before-name";
  }
  return { end: -1, selfClosing: false };
};

const tagNameEnd = (html: string, from: number): number => {
  let j = from;
  while (
    j < html.length &&
    !isTagSpace(html[j]) &&
    html[j] !== "/" &&
    html[j] !== ">"
  ) {
    j += 1;
  }
  return j;
};

/**
 * Comments, doctypes, processing instructions and `</` not followed by a
 * letter: the index after them, or -1 when they run to EOF.
 */
const skipMarkupDeclaration = (html: string, lt: number): number => {
  if (html.startsWith("<!--", lt)) {
    // `<!-->` and `<!--->` end at once (abrupt closing of empty comment).
    if (html.startsWith(">", lt + 4)) {
      return lt + 5;
    }
    if (html.startsWith("->", lt + 4)) {
      return lt + 6;
    }
    const dashes = html.indexOf("-->", lt + 4);
    const bang = html.indexOf("--!>", lt + 4);
    if (bang !== -1 && (dashes === -1 || bang < dashes)) {
      return bang + 4;
    }
    return dashes === -1 ? -1 : dashes + 3;
  }
  // `</>` is dropped; the rest are bogus comments up to the next `>`.
  const end = html.indexOf(">", lt);
  return end === -1 ? -1 : end + 1;
};

/**
 * Where the raw text of `name` (already lower-case) ends: the next `</name`
 * followed by a space, `/` or `>`, compared ASCII-case-insensitively.
 * -1 when it runs to EOF.
 */
const rawTextEnd = (html: string, from: number, name: string): number => {
  let j = html.indexOf("</", from);
  while (j !== -1) {
    const after = j + 2 + name.length;
    const next = html[after];
    if (
      asciiLower(html.slice(j + 2, after)) === name &&
      (next === "/" || next === ">" || isTagSpace(next))
    ) {
      return j;
    }
    j = html.indexOf("</", j + 2);
  }
  return -1;
};

/** Where scanning goes on after a start tag: past its raw text, if any. */
const afterStartTag = (
  html: string,
  from: number,
  name: string,
  onText: ((text: string) => void) | undefined
): number => {
  if (name === PLAINTEXT) {
    onText?.(html.slice(from));
    return -1;
  }
  return RAW_TEXT.has(name) ? rawTextEnd(html, from, name) : from;
};

/**
 * Calls `onTag` for each start and end tag and `onText` for the text
 * between them (not for raw text). Linear time.
 */
export const scanHtml = (
  html: string,
  onTag: (tag: TagToken) => TagAction,
  onText?: (text: string) => void
): void => {
  let i = 0;
  while (i < html.length) {
    const lt = html.indexOf("<", i);
    if (lt === -1) {
      onText?.(html.slice(i));
      return;
    }
    if (lt > i) {
      onText?.(html.slice(i, lt));
    }
    const next = html[lt + 1];
    const closing = next === "/";
    const nameStart = lt + (closing ? 2 : 1);
    if (isAsciiAlpha(html.codePointAt(nameStart) ?? 0)) {
      const nameEnd = tagNameEnd(html, nameStart);
      const { end, selfClosing } = tagEnd(html, nameEnd);
      if (end === -1) {
        // EOF inside a tag: the tokenizer drops it.
        return;
      }
      const name = asciiLower(html.slice(nameStart, nameEnd));
      const action = onTag({
        attributes: html.slice(nameEnd, end).trim(),
        closing,
        name,
        selfClosing,
      });
      if (action === "stop") {
        return;
      }
      i =
        closing || action === "markup"
          ? end + 1
          : afterStartTag(html, end + 1, name, onText);
    } else if (next === "!" || next === "?" || closing) {
      i = skipMarkupDeclaration(html, lt);
    } else {
      onText?.("<");
      i = lt + 1;
    }
    if (i === -1) {
      return;
    }
  }
};
