// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// HTML that goes into the compose editor: pasted HTML, the
// quote of the email being answered, signatures and reopened drafts.
//
// Same sanitizer as rendering (Proton + our removals + link rules), then a
// stricter allowlist: the result is rebuilt from scratch with only simple
// formatting elements and a handful of attributes. Nothing in it can load
// anything: no `<style>`, no `style` with URLs, no `srcset` or
// `background`. `style` keeps a few properties with checked values: hex
// colours, alignment, and only the editor's fonts and px sizes
// (compose-style.ts, rewritten canonically); `dir` only `ltr`/`rtl`. Remote and `data:` images are dropped, never fetched (not
// even kept as links: a quote must not carry the sender's trackers). The
// only image kept is `<img src="cid:…" alt="…">` for a Content-ID the
// caller lists (`inlineCids`: the inline parts of the email being quoted,
// which the reply or forward carries along); `cid:` never reaches the
// network. The editor's schema (TipTap) then drops whatever it doesn't
// know, a second filter.

import { isTextDirection, safeFontFamily, safeFontSize } from "./compose-style";
import { findCssHazard } from "./css-guard";
import {
  DEFAULT_LIMITS,
  checkHtmlLimits,
  measureTree,
  roughHtmlToText,
} from "./limits";
import type { RenderLimits } from "./limits";
import { OPENABLE_PROTOCOLS, processLinks } from "./links";
import { SanitizerUnavailableError, protonize } from "./proton";
import { ReportBuilder } from "./report";
import { removeDangerousContent } from "./sanitize";
import { parseInertHtml } from "./trusted-types";
import { cidUrl, normalizeCid, normalizeContentId, schemeOf } from "./urls";

/** Elements copied as they are. Anything else is unwrapped (its text stays). */
const KEPT_ELEMENTS: ReadonlySet<string> = new Set([
  "a",
  "b",
  "blockquote",
  "br",
  "code",
  "del",
  "div",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "i",
  "li",
  "mark",
  "ol",
  "p",
  "pre",
  "s",
  "span",
  "strike",
  "strong",
  "sub",
  "sup",
  "u",
  "ul",
]);

/** Block-level elements that become paragraphs when unwrapped (tables…). */
const BLOCK_LIKE: ReadonlySet<string> = new Set([
  "address",
  "article",
  "aside",
  "caption",
  "center",
  "dd",
  "details",
  "dl",
  "dt",
  "figcaption",
  "figure",
  "footer",
  "header",
  "main",
  "nav",
  "section",
  "summary",
  "table",
  "td",
  "th",
  "tr",
]);

/** Dropped with their content (nothing to show without loading something). */
const DROPPED_ELEMENTS: ReadonlySet<string> = new Set([
  "img",
  "picture",
  "image",
  "style",
  "map",
  "area",
  "canvas",
  "head",
  "title",
]);

/** Normalized Content-ID (normalizeContentId) → the Content-ID to write. */
type CidLookup = ReadonlyMap<string, string>;

const CID_SCHEME = /^\s*cid:/iu;

/** Content-IDs we write back into `cid:` URLs (no quotes, spaces or `<>`). */
export const SAFE_CID = /^[\w.@+=$%!~*'()/-]+$/u;

/** The image's Content-ID as the caller listed it, or null if not listed. */
const keptCid = (element: Element, cids: CidLookup): string | null => {
  const src =
    element.getAttribute("proton-src") ?? element.getAttribute("src") ?? "";
  if (schemeOf(src) !== "cid") {
    return null;
  }
  return cids.get(normalizeCid(src.replace(CID_SCHEME, ""))) ?? null;
};

/** Marks the editor uses for its own blocks (signature, quote). */
const DATA_ATTRIBUTES = ["data-signature", "data-quote"] as const;

/** A plain hex colour: nothing to load, nothing to parse. */
const HEX_COLOR = /^#(?:[\da-f]{3}|[\da-f]{6})$/iu;

/** `rgb(r, g, b)` as browsers write colours back (no alpha, no percent). */
const RGB_COLOR =
  /^rgb\(\s*(?<red>\d{1,3})\s*,\s*(?<green>\d{1,3})\s*,\s*(?<blue>\d{1,3})\s*\)$/iu;

/** A colour as hex, or null: hex as it is, `rgb()` converted. */
const hexColor = (value: string): string | null => {
  if (HEX_COLOR.test(value)) {
    return value;
  }
  const groups = RGB_COLOR.exec(value)?.groups;
  if (groups === undefined) {
    return null;
  }
  const channels = [groups.red, groups.green, groups.blue].map(Number);
  return channels.every((channel) => channel <= 255)
    ? `#${channels.map((channel) => channel.toString(16).padStart(2, "0")).join("")}`
    : null;
};

/** Simple keyword-like values (weights, styles, decorations). */
const SIMPLE_VALUE = /^[\w\s#%.,-]+$/u;

/**
 * `style` properties kept (formatting the editor understands), each with
 * the value it keeps, or null. Colours are written as hex: a strict
 * `rgb(r, g, b)` (how browsers write them back) is converted; nothing else
 * with parentheses gets through.
 */
type StyleRule = RegExp | ((value: string) => string | null);

const KEPT_STYLE: Readonly<Record<string, StyleRule>> = {
  "background-color": hexColor,
  color: hexColor,
  // Only the editor's fonts and sizes, rewritten canonically (compose-style.ts).
  "font-family": safeFontFamily,
  "font-size": safeFontSize,
  "font-style": SIMPLE_VALUE,
  "font-weight": SIMPLE_VALUE,
  "text-align": /^(?:left|center|right|justify|start|end)$/iu,
  "text-decoration": SIMPLE_VALUE,
  "text-decoration-line": SIMPLE_VALUE,
};

/** The value to keep for a style rule (a pattern keeps it as it is). */
const applyRule = (rule: StyleRule, value: string): string | null => {
  if (typeof rule === "function") {
    return rule(value);
  }
  return rule.test(value) ? value : null;
};

// Anything that could load or smuggle something; checked on the raw value.
const UNSAFE_STYLE_VALUE = /url|image|@|\\|expression|var\(|attr\(/iu;

const safeStyle = (element: Element): string | null => {
  const style = element.getAttribute("style");
  if (style === null || style.trim() === "") {
    return null;
  }
  const kept: string[] = [];
  for (const declaration of style.split(";")) {
    const colon = declaration.indexOf(":");
    const property = declaration.slice(0, colon).trim().toLowerCase();
    const value = declaration.slice(colon + 1).trim();
    const rule = Object.hasOwn(KEPT_STYLE, property)
      ? KEPT_STYLE[property]
      : undefined;
    const clean =
      colon > 0 && rule !== undefined && !UNSAFE_STYLE_VALUE.test(value)
        ? applyRule(rule, value)
        : null;
    if (clean !== null) {
      kept.push(`${property}: ${clean}`);
    }
  }
  return kept.length === 0 ? null : kept.join("; ");
};

const safeHref = (element: Element): string | null => {
  const href = element.getAttribute("href");
  if (href === null) {
    return null;
  }
  try {
    const url = new URL(href);
    return OPENABLE_PROTOCOLS.has(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
};

const copyAttributes = (from: Element, to: Element): void => {
  for (const name of DATA_ATTRIBUTES) {
    if (from.hasAttribute(name)) {
      to.setAttribute(name, "");
    }
  }
  const dir = from.getAttribute("dir")?.trim().toLowerCase();
  if (dir === "ltr" || dir === "rtl") {
    to.setAttribute("dir", dir);
  }
  const style = safeStyle(from);
  if (style !== null) {
    to.setAttribute("style", style);
  }
  // A highlight's colour (TipTap reads it back from here). `dataset` is
  // only on HTMLElement; these are plain Elements of a parsed document.
  // oxlint-disable-next-line unicorn/prefer-dom-node-dataset -- Element has no dataset.
  const highlight = from.getAttribute("data-color") ?? "";
  if (from.localName === "mark" && HEX_COLOR.test(highlight)) {
    // oxlint-disable-next-line unicorn/prefer-dom-node-dataset -- Element has no dataset.
    to.setAttribute("data-color", highlight);
  }
  if (from.localName === "a") {
    const href = safeHref(from);
    if (href !== null) {
      to.setAttribute("href", href);
    }
  }
};

const isElement = (node: Node): node is Element => node.nodeType === 1;
const isText = (node: Node): boolean => node.nodeType === 3;

/** Rebuilds `source`'s children under `target` with the allowlist. */
const rebuild = (
  source: Node,
  target: Element,
  doc: Document,
  cids: CidLookup
): void => {
  // Iterative (depth-first, in order), so a deep tree can't overflow the
  // stack.
  const stack: [Node, Element][] = [...source.childNodes]
    .toReversed()
    .map((child) => [child, target]);
  for (let entry = stack.pop(); entry !== undefined; entry = stack.pop()) {
    const [node, parent] = entry;
    if (isText(node)) {
      parent.append(doc.createTextNode(node.textContent ?? ""));
    } else if (isElement(node) && node.localName === "img") {
      const cid = keptCid(node, cids);
      if (cid !== null) {
        const image = doc.createElement("img");
        image.setAttribute("src", cidUrl(cid));
        image.setAttribute("alt", node.getAttribute("alt") ?? "");
        parent.append(image);
      }
    } else if (isElement(node) && !DROPPED_ELEMENTS.has(node.localName)) {
      // Comments, processing instructions and dropped elements go.
      let next: Element = parent;
      if (KEPT_ELEMENTS.has(node.localName)) {
        const copy = doc.createElement(node.localName);
        copyAttributes(node, copy);
        parent.append(copy);
        next = copy;
      } else if (BLOCK_LIKE.has(node.localName)) {
        const copy = doc.createElement("div");
        parent.append(copy);
        next = copy;
      }
      for (const child of [...node.childNodes].toReversed()) {
        stack.push([child, next]);
      }
    }
  }
};

const ESCAPES: Readonly<Record<string, string>> = {
  '"': "&quot;",
  "&": "&amp;",
  "'": "&#39;",
  "<": "&lt;",
  ">": "&gt;",
};

/** Escapes text for HTML (text content and attribute values). */
export const escapeHtml = (text: string): string =>
  text.replaceAll(/["&'<>]/gu, (char) => ESCAPES[char] ?? char);

/**
 * Plain text → editor HTML: one paragraph per line (empty lines kept),
 * escaped. Pure (no DOM).
 */
export const textToComposeHtml = (text: string): string => {
  const lines = text.replaceAll("\r\n", "\n").split("\n");
  return lines
    .map((line) => (line === "" ? "<p></p>" : `<p>${escapeHtml(line)}</p>`))
    .join("");
};

/** A kept inline image: `src` is one listed `cid:`, and only `alt` besides. */
const isCleanImage = (element: Element, cids: CidLookup): boolean =>
  keptCid(element, cids) !== null &&
  element.getAttributeNames().every((name) => name === "src" || name === "alt");

/** Every element and attribute in `html` is one we would produce. */
const isClean = (html: string, parser: DOMParser, cids: CidLookup): boolean => {
  const doc = parseInertHtml(`<body>${html}</body>`, parser);
  for (const element of doc.body.querySelectorAll("*")) {
    if (element.localName === "img") {
      if (!isCleanImage(element, cids)) {
        return false;
      }
      continue;
    }
    if (!KEPT_ELEMENTS.has(element.localName)) {
      return false;
    }
    for (const name of element.getAttributeNames()) {
      const allowed =
        DATA_ATTRIBUTES.some((data) => data === name) ||
        (name === "dir" && isTextDirection(element.getAttribute(name))) ||
        (name === "data-color" &&
          element.localName === "mark" &&
          HEX_COLOR.test(element.getAttribute(name) ?? "")) ||
        // Exactly what safeStyle keeps (it is idempotent on its output).
        (name === "style" &&
          !UNSAFE_STYLE_VALUE.test(element.getAttribute(name) ?? "") &&
          safeStyle(element) === element.getAttribute(name)) ||
        (name === "href" &&
          element.localName === "a" &&
          safeHref(element) !== null);
      if (!allowed) {
        return false;
      }
    }
  }
  return true;
};

export interface ComposeHtmlOptions {
  readonly limits?: Partial<RenderLimits>;
  /**
   * Content-IDs of the inline parts that travel with the message (the
   * quoted email's, or a reopened draft's): `<img src="cid:…">` for these
   * is kept, with `alt` only. Every other image is dropped.
   */
  readonly inlineCids?: readonly string[];
}

const cidLookup = (cids: readonly string[]): CidLookup =>
  new Map(
    cids
      .filter((cid) => SAFE_CID.test(cid))
      .map((cid) => [normalizeContentId(cid), cid] as const)
  );

const sanitizeParsed = (
  html: string,
  limits: RenderLimits,
  options: ComposeHtmlOptions,
  asText: () => string
): string => {
  if (findCssHazard(parseInertHtml(html, new DOMParser()))) {
    return asText();
  }
  const sanitized = protonize(html);
  // DOMPurify drops a leading <frameset>: then there is no body at all.
  const sanitizedBody = sanitized.body as HTMLElement | null;
  if (sanitizedBody === null) {
    return asText();
  }
  const measured = measureTree(sanitized.documentElement);
  if (
    measured.elements > limits.maxElements ||
    measured.depth > limits.maxNestingDepth
  ) {
    return asText();
  }
  const report = new ReportBuilder();
  removeDangerousContent(sanitized, report);
  processLinks(sanitized, { cleanLinks: false, report });

  const cids = cidLookup(options.inlineCids ?? []);
  const output = sanitized.implementation.createHTMLDocument("");
  rebuild(sanitizedBody, output.body, output, cids);
  const result = output.body.innerHTML;
  return isClean(result, new DOMParser(), cids) ? result : asText();
};

/**
 * Sanitizes HTML for the compose editor. Never throws for bad input: HTML
 * that is too big, too deep, or fails the final check becomes escaped text.
 * Needs a DOM (browser, or jsdom in tests).
 */
export const sanitizeComposeHtml = (
  html: string,
  options: ComposeHtmlOptions = {}
): string => {
  const limits: RenderLimits = { ...DEFAULT_LIMITS, ...options.limits };
  const asText = (): string =>
    textToComposeHtml(roughHtmlToText(html, limits.maxTextLength).trim());
  if (html.trim() === "") {
    return "";
  }
  if (checkHtmlLimits(html, limits) !== null) {
    return asText();
  }
  try {
    return sanitizeParsed(html, limits, options, asText);
  } catch (error) {
    // A hostile email can make the sanitizer or the DOM throw (a form
    // that clobbers DOM properties): escaped text then. Only a missing DOM
    // (configuration) propagates.
    if (error instanceof SanitizerUnavailableError) {
      throw error;
    }
    return asText();
  }
};
