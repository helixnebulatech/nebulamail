// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Last line of defence before the srcdoc leaves this package: parse the
// serialized document again (serialization/re-parse differences are how
// mutation XSS works) and check that it is inert. Any finding makes
// renderEmail fall back to plain text. Tests use a separate, stricter
// scanner (test/network-sinks.ts) so this is not checked against itself.

import { recurringUnescapeCSSEncoding } from "../vendor/proton-sanitize/src/escape";
import { stripCssComments } from "./css";
import { validateLoadableUrl } from "./images";
import type { ImageSource } from "./images";
import { OPENABLE_PROTOCOLS } from "./links";
import { REMOVED_ELEMENTS, UNWRAPPED_ELEMENTS } from "./sanitize";
import { parseSrcset } from "./urls";

const OUR_META_COUNT = 3;
const CSS_URL =
  /url\(\s*(?:"(?<double>(?:[^"\\]|\\.)*)"|'(?<single>(?:[^'\\]|\\.)*)'|(?<bare>[^)\s]*))/giu;

const unescapeCssString = (value: string): string =>
  value.replaceAll(/\\(?<hex>[\da-f]{1,6})\s?/giu, (_m, hex: string) =>
    String.fromCodePoint(Number.parseInt(hex, 16))
  );

const urlOf = (match: RegExpMatchArray): string =>
  match.groups?.double ?? match.groups?.single ?? match.groups?.bare ?? "";

const checkCss = (
  css: string,
  source: ImageSource,
  where: string,
  problems: string[]
): void => {
  const decoded = stripCssComments(recurringUnescapeCSSEncoding(css));
  if (/@import\b/iu.test(decoded)) {
    problems.push(`${where}: @import`);
  }
  for (const match of css.matchAll(CSS_URL)) {
    const url = unescapeCssString(urlOf(match));
    if (validateLoadableUrl(url, source) === null) {
      problems.push(`${where}: url(${url.slice(0, 80)})`);
    }
  }
  if (decoded !== css) {
    for (const match of decoded.matchAll(CSS_URL)) {
      const url = urlOf(match);
      if (validateLoadableUrl(url, source) === null) {
        problems.push(`${where}: escaped url(${url.slice(0, 80)})`);
      }
    }
  }
};

const checkHref = (element: Element, problems: string[]): void => {
  const href = element.getAttribute("href");
  if (href === null) {
    return;
  }
  if (element.localName !== "a" && element.localName !== "area") {
    problems.push(`<${element.localName}> href`);
    return;
  }
  if (href.startsWith("#")) {
    return;
  }
  try {
    if (!OPENABLE_PROTOCOLS.has(new URL(href).protocol)) {
      problems.push(`href scheme ${href.slice(0, 40)}`);
    }
  } catch {
    problems.push(`relative href ${href.slice(0, 40)}`);
  }
};

const checkAttribute = (
  attr: Attr,
  source: ImageSource,
  problems: string[]
): void => {
  const attrName = attr.name.toLowerCase();
  if (attrName.startsWith("on")) {
    problems.push(`event handler ${attrName}`);
  } else if (attrName === "src" || attrName === "background") {
    if (validateLoadableUrl(attr.value, source) === null) {
      problems.push(`${attrName}=${attr.value.slice(0, 80)}`);
    }
  } else if (attrName === "srcset") {
    for (const candidate of parseSrcset(attr.value)) {
      if (validateLoadableUrl(candidate.url, source) === null) {
        problems.push(`srcset ${candidate.url.slice(0, 80)}`);
      }
    }
  } else if (attrName === "style") {
    checkCss(attr.value, source, "style attribute", problems);
  }
};

export const findInertnessProblems = (
  doc: Document,
  csp: string,
  source: ImageSource
): string[] => {
  const problems: string[] = [];
  const first = doc.head.firstElementChild;
  if (
    first?.localName !== "meta" ||
    first.getAttribute("http-equiv")?.toLowerCase() !==
      "content-security-policy" ||
    first.getAttribute("content") !== csp
  ) {
    problems.push("CSP meta is not the first element of head");
  }
  if (doc.querySelectorAll("meta").length !== OUR_META_COUNT) {
    problems.push("unexpected meta element");
  }
  for (const element of doc.querySelectorAll("*")) {
    const name = element.localName;
    if (
      name !== "meta" &&
      (REMOVED_ELEMENTS.has(name) || UNWRAPPED_ELEMENTS.has(name))
    ) {
      problems.push(`element <${name}>`);
    }
    if (element.namespaceURI !== "http://www.w3.org/1999/xhtml") {
      problems.push(`foreign element <${name}>`);
    }
    for (const attr of element.attributes) {
      checkAttribute(attr, source, problems);
    }
    checkHref(element, problems);
    if (name === "style") {
      checkCss(element.textContent, source, "style element", problems);
    }
  }
  return problems;
};
