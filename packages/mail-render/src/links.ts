// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Links.
//   - In the document: no `target`, `rel="noopener noreferrer"`, only
//     http(s)/mailto hrefs (and same-document fragments), tracking
//     parameters removed with tidy-url.
//   - When clicked: `resolveLinkForOpening` returns only http/https/mailto
//     from the URL as the browser resolved it, and `analyzeLink` flags what
//     the confirmation dialog must warn about.

import { TidyCleaner } from "@protontech/tidy-url";

import type { ReportBuilder } from "./report";
import { cleanDisplayText } from "./unicode";
import { normalizeUrlInput, trimEndChar } from "./urls";

export const OPENABLE_PROTOCOLS: ReadonlySet<string> = new Set([
  "http:",
  "https:",
  "mailto:",
]);

const LINK_REL = "noopener noreferrer";
const FRAGMENT = /^#[\w.:-]{1,100}$/u;

let cleaner: TidyCleaner | undefined;

const tidy = (): TidyCleaner => {
  if (cleaner === undefined) {
    cleaner = new TidyCleaner();
    // allowAMP as Proton does. `silent: false` is what keeps tidy-url quiet:
    // its logger prints when `silent !== false` (upstream bug in 1.18.6).
    cleaner.config.setMany({ allowAMP: true, silent: false });
  }
  return cleaner;
};

const parseAbsolute = (value: string): URL | null => {
  try {
    return new URL(value);
  } catch {
    return null;
  }
};

/**
 * Removes tracking parameters and unwraps known redirectors. Returns the
 * input unchanged unless the result is still an http(s) URL.
 */
export const cleanLink = (href: string): string => {
  const original = parseAbsolute(href);
  if (
    original === null ||
    (original.protocol !== "http:" && original.protocol !== "https:")
  ) {
    return href;
  }
  let cleaned: string;
  try {
    cleaned = tidy().clean(original.href).url;
  } catch {
    return href;
  }
  const parsed = parseAbsolute(cleaned);
  if (
    parsed === null ||
    (parsed.protocol !== "http:" && parsed.protocol !== "https:")
  ) {
    return href;
  }
  return parsed.href === original.href ? href : parsed.href;
};

export interface LinkContext {
  readonly cleanLinks: boolean;
  readonly report: ReportBuilder;
}

/** Normalized href to keep in the document, or null to remove it. */
const safeHref = (raw: string): string | null => {
  const value = normalizeUrlInput(raw);
  if (FRAGMENT.test(value)) {
    return value;
  }
  // Absolute only: a relative href would resolve against our app origin.
  const parsed = parseAbsolute(value);
  if (parsed === null || !OPENABLE_PROTOCOLS.has(parsed.protocol)) {
    return null;
  }
  return parsed.href;
};

/**
 * Links cleaned with tidy-url per email (it walks every rule for each one).
 * Past this, links keep their (safe) href as they are.
 */
export const MAX_CLEANED_LINKS = 2000;

export const processLinks = (doc: Document, ctx: LinkContext): void => {
  let cleaned = 0;
  for (const link of doc.querySelectorAll("a, area")) {
    link.removeAttribute("target");
    const raw = link.getAttribute("href");
    if (raw === null) {
      continue;
    }
    link.setAttribute("rel", LINK_REL);
    const href = safeHref(raw);
    if (href === null) {
      link.removeAttribute("href");
      ctx.report.unsafeLink();
      continue;
    }
    let final = href;
    if (
      ctx.cleanLinks &&
      !href.startsWith("#") &&
      cleaned < MAX_CLEANED_LINKS
    ) {
      cleaned += 1;
      final = cleanLink(href);
      if (final !== href) {
        ctx.report.linkCleaned(href, final);
      }
    }
    link.setAttribute("href", final);
  }
};

export interface OpenableLink {
  /** The URL to open (already resolved and serialized). */
  readonly url: string;
  readonly protocol: "http:" | "https:" | "mailto:";
}

/**
 * The only way the app should open a link from an email. Pass the URL the
 * browser resolved (`anchor.href`, see `findLinkFromEvent`), never the raw
 * attribute: `javascript:` in any obfuscation (`jAvAsCrIpT:`, `java\tscript:`,
 * entities, leading control characters) is normalized by the HTML and URL
 * parsers into the `javascript:` scheme, which is rejected here.
 * Opening the raw href instead would let a `javascript:` that slipped
 * through run in the app.
 */
export interface ResolveOptions {
  /**
   * The app's own origin. A srcdoc document resolves relative URLs against
   * the parent page, so a relative href that slipped through would point
   * into the app; such links are refused.
   */
  readonly appOrigin?: string;
}

export const resolveLinkForOpening = (
  href: string,
  base?: string,
  options: ResolveOptions = {}
): OpenableLink | null => {
  let parsed: URL;
  try {
    parsed = base === undefined ? new URL(href) : new URL(href, base);
  } catch {
    return null;
  }
  const { protocol } = parsed;
  if (protocol !== "http:" && protocol !== "https:" && protocol !== "mailto:") {
    return null;
  }
  if (
    protocol !== "mailto:" &&
    (parsed.hostname === "" || parsed.origin === options.appOrigin)
  ) {
    return null;
  }
  return { protocol, url: parsed.href };
};

export type LinkWarning =
  /** The link text names a domain other than the target's. */
  | "text-domain-mismatch"
  /** The target host has an IDN (punycode `xn--`) label. */
  | "punycode"
  /** `https://paypal.com@evil.example/`: userinfo hides the real host. */
  | "userinfo"
  /** The host is an IP address. */
  | "ip-address"
  /** Plain http. */
  | "insecure";

export interface LinkAnalysis {
  readonly url: string;
  /** Host as serialized by the URL parser (punycode, never Unicode). */
  readonly host: string;
  readonly registrableDomain: string;
  readonly warnings: readonly LinkWarning[];
  /** Domains found in the link text. */
  readonly textDomains: readonly string[];
}

// Common second-level public suffixes. This is a heuristic, not the Public
// Suffix List: for suffixes not listed here (e.g. `city.kawasaki.jp`,
// `github.io` style private suffixes) two different sites can share the
// same "registrable domain" (missed warning) or one site's subdomain can
// look like a different domain (extra warning). Both only change whether
// the dialog shows a warning; the real target is always displayed.
const SECOND_LEVEL_SUFFIXES = new Set([
  "ac.jp",
  "ac.uk",
  "co.il",
  "co.in",
  "co.jp",
  "co.kr",
  "co.nz",
  "co.uk",
  "co.za",
  "com.ar",
  "com.au",
  "com.br",
  "com.cn",
  "com.co",
  "com.es",
  "com.hk",
  "com.mx",
  "com.pe",
  "com.pl",
  "com.sg",
  "com.tr",
  "com.tw",
  "com.uy",
  "com.ve",
  "edu.au",
  "gob.es",
  "gob.mx",
  "gov.au",
  "gov.uk",
  "ne.jp",
  "net.au",
  "net.br",
  "or.jp",
  "org.au",
  "org.br",
  "org.es",
  "org.uk",
  "gv.at",
  "ltd.uk",
  "plc.uk",
  "me.uk",
]);

const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/u;

const isIpHost = (host: string): boolean =>
  host.startsWith("[") || IPV4.test(host);

/** eTLD+1 heuristic (see SECOND_LEVEL_SUFFIXES for the limits). */
export const registrableDomain = (host: string): string => {
  const h = trimEndChar(host.toLowerCase(), ".");
  if (isIpHost(h)) {
    return h;
  }
  const labels = h.split(".");
  if (labels.length <= 2) {
    return h;
  }
  const lastTwo = labels.slice(-2).join(".");
  return SECOND_LEVEL_SUFFIXES.has(lastTwo)
    ? labels.slice(-3).join(".")
    : lastTwo;
};

// Something that looks like a domain in link text: labels of letters
// (any script), digits and hyphens, ending in a 2+ letter TLD.
const TEXT_DOMAIN =
  /(?:[a-z][a-z\d+.-]*:\/\/)?(?<domain>(?:[\p{L}\p{N}](?:[\p{L}\p{N}-]{0,61}[\p{L}\p{N}])?\.)+\p{L}{2,63})(?![\p{L}\p{N}-])/giu;
const MAX_TEXT = 500;
// Only for finding domains, never for display: characters that render as
// nothing ("paypal\u200D.com" reads as paypal.com; cleanDisplayText keeps
// ZWJ/ZWNJ for emoji and scripts) and dots UTS 46 maps to ".".
const IGNORABLE = /\p{Default_Ignorable_Code_Point}/gu;
const DOT_LIKE = /[\u3002\uFF0E\uFF61]/gu;

/**
 * Domains named in a link's visible text, normalized to punycode. Read
 * the way the user reads it: without invisible characters, and only the
 * first 500 characters. Text hidden with CSS is not detected (accepted:
 * like a link whose text names no domain).
 */
export const domainsInText = (text: string): string[] => {
  const found = new Set<string>();
  const visible = cleanDisplayText(
    text.replaceAll(IGNORABLE, "").replaceAll(DOT_LIKE, ".")
  ).slice(0, MAX_TEXT);
  for (const match of visible.matchAll(TEXT_DOMAIN)) {
    const candidate = match.groups?.domain;
    if (candidate === undefined) {
      continue;
    }
    // "user@bank.com" yields bank.com, which is what the reader sees.
    const parsed = parseAbsolute(`http://${candidate}`);
    if (parsed !== null) {
      found.add(parsed.hostname);
    }
  }
  return [...found];
};

export const analyzeLink = (url: string, text = ""): LinkAnalysis | null => {
  const parsed = parseAbsolute(url);
  if (parsed === null) {
    return null;
  }
  const warnings: LinkWarning[] = [];
  const host = parsed.hostname;
  const target = registrableDomain(host);
  const textDomains = domainsInText(text);
  if (parsed.protocol === "http:" || parsed.protocol === "https:") {
    if (
      // Any named domain that is not the target: "paypal.com evil.example"
      // pointing to evil.example still warns.
      textDomains.some((domain) => registrableDomain(domain) !== target)
    ) {
      warnings.push("text-domain-mismatch");
    }
    if (host.split(".").some((label) => label.startsWith("xn--"))) {
      warnings.push("punycode");
    }
    if (parsed.username !== "" || parsed.password !== "") {
      warnings.push("userinfo");
    }
    if (isIpHost(host)) {
      warnings.push("ip-address");
    }
    if (parsed.protocol === "http:") {
      warnings.push("insecure");
    }
  }
  return {
    host,
    registrableDomain: target,
    textDomains,
    url: parsed.href,
    warnings,
  };
};

/** Whether the confirmation dialog must be shown (setting `linkConfirmation`). */
export const needsConfirmation = (
  mode: "always" | "suspicious" | "never",
  analysis: LinkAnalysis | null
): boolean => {
  if (analysis === null) {
    return true;
  }
  switch (mode) {
    case "always": {
      return true;
    }
    case "suspicious": {
      return analysis.warnings.length > 0;
    }
    case "never": {
      // Even with "never", deceptive links are confirmed: hiding a
      // phishing warning is not a privacy preference.
      return analysis.warnings.some((w) => w !== "insecure");
    }
    default: {
      return true;
    }
  }
};

export interface MailtoFields {
  readonly to: readonly string[];
  readonly cc: readonly string[];
  readonly bcc: readonly string[];
  readonly subject: string;
  readonly body: string;
}

const splitAddresses = (value: string): string[] =>
  value
    .split(",")
    .map((address) => address.trim())
    .filter((address) => address !== "");

const decode = (value: string): string => {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
};

/**
 * Parses a `mailto:` URL for our composer, keeping only `to`, `cc`, `bcc`,
 * `subject` and `body`: `attach` and friends are ignored.
 */
export const parseMailto = (href: string): MailtoFields | null => {
  const parsed = parseAbsolute(href);
  if (parsed?.protocol !== "mailto:") {
    return null;
  }
  const to = splitAddresses(decode(parsed.pathname));
  const cc: string[] = [];
  const bcc: string[] = [];
  let subject = "";
  let body = "";
  // In mailto (RFC 6068) "+" is a literal plus, not a space.
  for (const [key, value] of new URLSearchParams(
    parsed.search.replaceAll("+", "%2B")
  )) {
    switch (key.toLowerCase()) {
      case "to": {
        to.push(...splitAddresses(value));
        break;
      }
      case "cc": {
        cc.push(...splitAddresses(value));
        break;
      }
      case "bcc": {
        bcc.push(...splitAddresses(value));
        break;
      }
      case "subject": {
        subject ||= value;
        break;
      }
      case "body": {
        body ||= value;
        break;
      }
      default:
    }
  }
  return { bcc, body, cc, subject, to };
};
