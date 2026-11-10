// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Size and nesting limits for email HTML.
// Over a limit, the pipeline shows the text part instead.

import { estimateHtml } from "./html-estimate";
import { scanHtml } from "./html-scan";

export interface RenderLimits {
  /** Maximum HTML length in UTF-16 code units, checked before parsing. */
  readonly maxHtmlLength: number;
  /** Maximum estimated nesting depth, checked on the string before parsing. */
  readonly maxNestingDepth: number;
  /**
   * Maximum number of elements: estimated before parsing, and counted
   * after parsing (and after sanitizing in the browser).
   */
  readonly maxElements: number;
  /** Plain text longer than this is truncated. */
  readonly maxTextLength: number;
}

export const DEFAULT_LIMITS: RenderLimits = {
  maxElements: 50_000,
  maxHtmlLength: 2_000_000,
  maxNestingDepth: 400,
  maxTextLength: 2_000_000,
};

export type LimitViolation =
  | "too-large"
  | "too-deep"
  | "too-many-elements"
  // CSS the vendored sanitizer would be too slow on (css.ts cssCommentHazard).
  | "too-complex";

/**
 * Estimated nesting depth of the tree the parser will build, in one
 * linear pass, without building a DOM (see html-estimate.ts). A prefilter:
 * the real check is `measureTree` on the parsed document.
 */
export const estimateNestingDepth = (
  html: string,
  stopAt = Number.POSITIVE_INFINITY
): number => estimateHtml(html, stopAt).depth;

/**
 * Limits checked on the string before any parsing: length, then the
 * estimated depth and element count (which include the formatting elements
 * the parser rebuilds, so a few bytes cannot make it build a huge tree).
 */
export const checkHtmlLimits = (
  html: string,
  limits: RenderLimits = DEFAULT_LIMITS
): LimitViolation | null => {
  if (html.length > limits.maxHtmlLength) {
    return "too-large";
  }
  const { exceeded } = estimateHtml(
    html,
    limits.maxNestingDepth,
    limits.maxElements
  );
  if (exceeded === "depth") {
    return "too-deep";
  }
  return exceeded === "elements" ? "too-many-elements" : null;
};

/** Element count and real depth of a parsed tree, without recursion. */
/** What [`measureTree`] reads of a node: a DOM `Element`, or a light tree. */
export interface TreeNode {
  readonly children: Iterable<TreeNode>;
}

export const measureTree = (
  root: TreeNode
): { elements: number; depth: number } => {
  let elements = 0;
  let depth = 0;
  const stack: [TreeNode, number][] = [[root, 1]];
  let item = stack.pop();
  while (item !== undefined) {
    const [element, level] = item;
    elements += 1;
    depth = Math.max(depth, level);
    for (const child of element.children) {
      stack.push([child, level + 1]);
    }
    item = stack.pop();
  }
  return { depth, elements };
};

const ENTITY =
  /&(?:#(?<dec>\d{1,7})|#x(?<hex>[\da-f]{1,6})|(?<named>amp|lt|gt|quot|apos|nbsp));/giu;
const NAMED: Readonly<Record<string, string>> = {
  amp: "&",
  apos: "'",
  gt: ">",
  lt: "<",
  nbsp: "\u00A0",
  quot: '"',
};

// Named groups still arrive positionally: (match, dec, hex, named).
const decodeEntity = (
  _match: string,
  dec?: string,
  hex?: string,
  named?: string
): string => {
  if (named !== undefined) {
    return NAMED[named.toLowerCase()] ?? "";
  }
  const code =
    dec === undefined
      ? Number.parseInt(hex ?? "", 16)
      : Math.trunc(Number(dec));
  return Number.isFinite(code) && code > 0 && code <= 0x10_ff_ff
    ? String.fromCodePoint(code)
    : "";
};

const BLOCK_BREAK = new Set([
  "br",
  "p",
  "div",
  "tr",
  "li",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "table",
  "blockquote",
]);
const SKIP_CONTENT = new Set(["script", "style", "title", "head"]);

/**
 * Last-resort text for HTML we refuse to parse (no text/plain part). Linear
 * time. The result is only ever inserted as text nodes, so leftovers are
 * harmless.
 */
export const roughHtmlToText = (
  html: string,
  maxLength = DEFAULT_LIMITS.maxTextLength
): string => {
  const parts: string[] = [];
  let skipping: string | null = null;
  scanHtml(
    html.slice(0, maxLength),
    (tag) => {
      if (skipping !== null) {
        if (tag.closing && tag.name === skipping) {
          skipping = null;
        }
      } else if (!tag.closing && SKIP_CONTENT.has(tag.name)) {
        skipping = tag.name;
      } else if (BLOCK_BREAK.has(tag.name)) {
        parts.push("\n");
      }
      return "continue";
    },
    (text) => {
      if (skipping === null) {
        parts.push(text);
      }
    }
  );
  return parts
    .join("")
    .replaceAll(ENTITY, decodeEntity)
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .replaceAll(/\n{3,}/gu, "\n\n")
    .trim();
};
