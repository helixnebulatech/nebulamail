// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test-only scanner of a rendered srcdoc for anything that could execute
// code or make a network request. Deliberately independent from
// src/verify.ts (different code, stricter rules), so the pipeline is not
// checked against itself. Playwright with real browsers (0 requests) is the
// next layer, see security/hostile-emails/README.md.

const XHTML = "http://www.w3.org/1999/xhtml";

/** Elements that must never appear in the output. */
const FORBIDDEN_ELEMENTS = new Set([
  "script",
  "iframe",
  "frame",
  "frameset",
  "object",
  "embed",
  "applet",
  "param",
  "form",
  "input",
  "button",
  "select",
  "option",
  "optgroup",
  "textarea",
  "datalist",
  "keygen",
  "output",
  "label",
  "selectedcontent",
  "base",
  "link",
  "svg",
  "math",
  "template",
  "noscript",
  "noembed",
  "noframes",
  "portal",
  "fencedframe",
  "video",
  "audio",
  "source",
  "track",
  "bgsound",
  "xmp",
  "plaintext",
  "title",
  "slot",
  "dialog",
  "isindex",
]);

/** Attributes that make the browser fetch, navigate, submit or ping. */
const FETCHING_ATTRIBUTES = new Set([
  "src",
  "srcset",
  "srcdoc",
  "href",
  "xlink:href",
  "background",
  "poster",
  "data",
  "action",
  "formaction",
  "codebase",
  "archive",
  "classid",
  "cite",
  "longdesc",
  "lowsrc",
  "dynsrc",
  "manifest",
  "icon",
  "ping",
  "profile",
  "itemtype",
  "usemap",
  "attributionsrc",
  "content",
  "http-equiv",
]);

/** CSS functions/at-rules that fetch (any occurrence outside `url(<allowed>)` fails). */
const CSS_FETCHERS =
  /(?<![\w-])(?:image-set|-webkit-image-set|image|cross-fade|-webkit-cross-fade|src|element|-moz-element|paint|expression)\s*\(|@import\b|(?<![\w-])(?:behavior|-moz-binding)\s*:/iu;

// oxlint-disable no-control-regex -- control characters are what browsers strip from URLs
const URL_STRIPPED = /[\s\u0000-\u001F]/gu;
// oxlint-enable no-control-regex

const SCRIPT_SCHEMES = /(?:java|vb|live)script\s*:|data\s*:\s*text\/html/iu;

export interface SinkPolicy {
  /** May this URL appear in src/srcset/background/CSS url()? */
  readonly allowLoad: (url: string) => boolean;
  /** Expected exact CSP content. */
  readonly csp: string;
}

export interface Finding {
  readonly where: string;
  readonly what: string;
}

/** CSS escape decoding per CSS Syntax §4.3.7, plus comment removal. */
export const decodeCss = (css: string): string =>
  css
    .replaceAll(
      /\\(?<hex>[\da-f]{1,6})(?:\r\n|[ \t\n\r\f])?/giu,
      (_m, hex: string) => {
        const code = Number.parseInt(hex, 16);
        return code === 0 ||
          code > 0x10_ff_ff ||
          (code >= 0xd8_00 && code <= 0xdf_ff)
          ? "\uFFFD"
          : String.fromCodePoint(code);
      }
    )
    .replaceAll(/\\(?<char>[^\n\da-f])/giu, "$<char>")
    .replaceAll(/\/\*[\s\S]*?\*\//gu, "");

const CSS_URL =
  /url\(\s*(?:"(?<double>[^"]*)"|'(?<single>[^']*)'|(?<bare>[^)\s]*))\s*\)/giu;

const scanCss = (
  css: string,
  where: string,
  policy: SinkPolicy,
  findings: Finding[]
): void => {
  const decoded = decodeCss(css);
  if (CSS_FETCHERS.test(decoded)) {
    findings.push({
      what: `fetching CSS construct in ${decoded.slice(0, 120)}`,
      where,
    });
  }
  const loose = /url\s*\(/giu;
  const strict = [...decoded.matchAll(CSS_URL)];
  const looseCount = [...decoded.matchAll(loose)].length;
  if (looseCount !== strict.length) {
    findings.push({ what: "unparseable url(", where });
  }
  for (const match of strict) {
    const url = (
      match.groups?.double ??
      match.groups?.single ??
      match.groups?.bare ??
      ""
    ).trim();
    if (!policy.allowLoad(url)) {
      findings.push({ what: `css url(${url.slice(0, 120)})`, where });
    }
  }
};

const scanHref = (
  element: Element,
  value: string,
  findings: Finding[]
): void => {
  const where = `<${element.localName} href>`;
  if (element.localName !== "a" && element.localName !== "area") {
    findings.push({ what: "href on a non-link element", where });
    return;
  }
  if (/^#[\w.:-]+$/u.test(value)) {
    return;
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    findings.push({ what: `relative href ${value}`, where });
    return;
  }
  if (!["http:", "https:", "mailto:"].includes(url.protocol)) {
    findings.push({ what: `href scheme ${url.protocol}`, where });
  }
};

const scanSrcset = (
  value: string,
  policy: SinkPolicy,
  findings: Finding[]
): void => {
  for (const candidate of value.split(/,\s+/u)) {
    const url = candidate.trim().split(/\s+/u)[0] ?? "";
    if (!policy.allowLoad(url)) {
      findings.push({ what: `srcset ${url.slice(0, 120)}`, where: "srcset" });
    }
  }
};

const scanAttribute = (
  element: Element,
  attr: Attr,
  policy: SinkPolicy,
  findings: Finding[]
): void => {
  const name = element.localName;
  const attrName = attr.name.toLowerCase();
  const where = `<${name} ${attrName}>`;
  if (attrName.startsWith("on")) {
    findings.push({ what: "event handler", where });
  }
  if (SCRIPT_SCHEMES.test(attr.value.replaceAll(URL_STRIPPED, ""))) {
    findings.push({
      what: `script scheme in ${attr.value.slice(0, 80)}`,
      where,
    });
  }
  if (attrName === "style") {
    scanCss(attr.value, where, policy, findings);
  } else if (attrName === "href") {
    scanHref(element, attr.value, findings);
  } else if (attrName === "src" || attrName === "background") {
    if (!policy.allowLoad(attr.value)) {
      findings.push({
        what: `${attrName}=${attr.value.slice(0, 120)}`,
        where,
      });
    }
  } else if (attrName === "srcset") {
    scanSrcset(attr.value, policy, findings);
  } else if (FETCHING_ATTRIBUTES.has(attrName) && name !== "meta") {
    findings.push({ what: `fetching attribute ${attrName}`, where });
  } else if (
    attrName !== "alt" &&
    attrName !== "title" &&
    /(?:^|\s)(?:[a-z][a-z\d+.-]*:)?\/\/\S/iu.test(attr.value)
  ) {
    findings.push({
      what: `URL-like value ${attr.value.slice(0, 80)}`,
      where,
    });
  }
};

const scanElement = (
  element: Element,
  policy: SinkPolicy,
  findings: Finding[]
): void => {
  const name = element.localName;
  if (element.namespaceURI !== XHTML) {
    findings.push({ what: "foreign-namespace element", where: `<${name}>` });
  }
  if (FORBIDDEN_ELEMENTS.has(name)) {
    findings.push({ what: "forbidden element", where: `<${name}>` });
  }
  for (const attr of element.attributes) {
    scanAttribute(element, attr, policy, findings);
  }
  if (name === "style") {
    scanCss(element.textContent ?? "", "<style>", policy, findings);
    if ((element.textContent ?? "").includes("</")) {
      findings.push({
        what: "closing tag inside style text",
        where: "<style>",
      });
    }
  }
};

const scanHead = (
  doc: Document,
  policy: SinkPolicy,
  findings: Finding[]
): void => {
  const metas = [...doc.querySelectorAll("meta")];
  const first = doc.head.firstElementChild;
  if (
    first === null ||
    first !== metas[0] ||
    first.getAttribute("http-equiv") !== "Content-Security-Policy" ||
    first.getAttribute("content") !== policy.csp
  ) {
    findings.push({
      what: "CSP meta is not the first child of head",
      where: "<head>",
    });
  }
  const allowedMetas = new Set([
    `http-equiv=Content-Security-Policy|${policy.csp}`,
    "name=referrer|no-referrer",
    "http-equiv=x-dns-prefetch-control|off",
  ]);
  for (const meta of metas) {
    const key = meta.hasAttribute("http-equiv")
      ? `http-equiv=${meta.getAttribute("http-equiv")}|${meta.getAttribute("content")}`
      : `name=${meta.getAttribute("name")}|${meta.getAttribute("content")}`;
    if (!allowedMetas.has(key) || meta.parentElement !== doc.head) {
      findings.push({ what: `unexpected meta ${key}`, where: "<meta>" });
    }
    allowedMetas.delete(key);
  }
};

/** Scans a srcdoc string; returns every finding (empty = inert). */
export const scanNetworkSinks = (
  srcdoc: string,
  policy: SinkPolicy
): Finding[] => {
  const findings: Finding[] = [];
  if (!srcdoc.startsWith("<!DOCTYPE html>")) {
    findings.push({ what: "missing doctype (quirks mode)", where: "document" });
  }
  const doc = new DOMParser().parseFromString(srcdoc, "text/html");
  scanHead(doc, policy, findings);
  for (const element of doc.querySelectorAll("*")) {
    if (element.localName !== "meta") {
      scanElement(element, policy, findings);
    }
  }
  // Serialize → parse → serialize must be stable, or the browser would see
  // a different tree from the one we checked (mXSS).
  const again = `<!DOCTYPE html>${doc.documentElement.outerHTML}`;
  const twice = new DOMParser().parseFromString(again, "text/html");
  if (`<!DOCTYPE html>${twice.documentElement.outerHTML}` !== again) {
    findings.push({ what: "serialization is not stable", where: "document" });
  }
  return findings;
};

const INLINE_RASTER =
  /^data:image\/(?:png|gif|jpe?g|webp|avif|bmp|x-icon|apng)[;,]/iu;

/** Images off: only inline rasters may load. */
export const imagesOffPolicy = (csp: string): SinkPolicy => ({
  allowLoad: (url) => INLINE_RASTER.test(url.trim()),
  csp,
});

/** Images on: inline rasters or URLs on the proxy origin. */
export const imagesOnPolicy = (
  csp: string,
  proxyOrigin: string
): SinkPolicy => ({
  allowLoad: (url) => {
    const value = url.trim();
    if (INLINE_RASTER.test(value)) {
      return true;
    }
    try {
      return new URL(value).origin === proxyOrigin;
    } catch {
      return false;
    }
  },
  csp,
});
