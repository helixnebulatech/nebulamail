// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The iframe document. The CSP `<meta>` must be the
// first thing in `<head>`: a meta CSP only applies to what is parsed after
// it. The srcdoc also inherits the app's own CSP (about:srcdoc shares the
// parent's policy container), so both policies apply.

import type { ImageSource } from "./images";

export class InvalidImageSourceError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "InvalidImageSourceError";
  }
}

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * Plain http is only for a proxy on this machine in development:
 * localhost, its subdomains (`img.localhost`, which browsers resolve to
 * loopback, RFC 6761) and loopback IPs.
 */
const isLocalDevHost = (hostname: string): boolean =>
  LOCAL_HOSTS.has(hostname) || hostname.endsWith(".localhost");

/**
 * The proxy origin must be a bare origin: https, or http on localhost for
 * development. Returns the serialized origin (it goes into the CSP).
 */
export const validateProxyOrigin = (origin: string): string => {
  let parsed: URL;
  try {
    parsed = new URL(origin);
  } catch {
    throw new InvalidImageSourceError(`Proxy origin is not a URL: ${origin}`);
  }
  const secure = parsed.protocol === "https:";
  const localDev =
    parsed.protocol === "http:" && isLocalDevHost(parsed.hostname);
  if (!secure && !localDev) {
    throw new InvalidImageSourceError(
      "Proxy origin must be https (or http://localhost in development)"
    );
  }
  if (origin !== parsed.origin && origin !== `${parsed.origin}/`) {
    throw new InvalidImageSourceError(
      `Proxy origin must be an origin without path or credentials: ${origin}`
    );
  }
  return parsed.origin;
};

/** Normalizes and validates an image source (throws InvalidImageSourceError). */
export const validateImageSource = (source: ImageSource): ImageSource =>
  source.kind === "proxy"
    ? { ...source, origin: validateProxyOrigin(source.origin) }
    : source;

/** `img-src` values: `data:` always, plus where the image source serves from. */
export const imageSourcesFor = (source: ImageSource): string[] => {
  switch (source.kind) {
    case "proxy": {
      return ["data:", source.origin];
    }
    case "batch": {
      return ["data:", "blob:"];
    }
    case "direct": {
      return ["data:", "https:"];
    }
    case "none": {
      return ["data:"];
    }
    default: {
      return ["data:"];
    }
  }
};

export const buildContentSecurityPolicy = (imgSrc: readonly string[]): string =>
  [
    "default-src 'none'",
    `img-src ${imgSrc.join(" ")}`,
    "style-src 'unsafe-inline'",
    "font-src data:",
    "form-action 'none'",
    "base-uri 'none'",
  ].join("; ");

/** Base style: our defaults first, so the email's own styles can override them. */
export const BASE_STYLE = [
  ":root{color-scheme:light}",
  "html,body{margin:0;padding:0}",
  // A sans-serif default like other mail apps (not the browser's serif):
  // the system's, since the CSP loads no fonts. The email's own styles win.
  'body{padding:8px;overflow-wrap:anywhere;word-break:break-word;font-family:system-ui,-apple-system,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;font-size:14px}',
  "img{max-width:100%}",
  "pre{white-space:pre-wrap}",
  "table{border-collapse:collapse}",
  ".wm-plain-text{white-space:pre-wrap;font-family:inherit;margin:0}",
  // Scrollbars like the app's (styles/app.css): thin, rounded, no arrows.
  // Fixed light colours: the email document is always light. Firefox and
  // other browsers without ::-webkit-scrollbar get the standard properties.
  "::-webkit-scrollbar{width:10px;height:10px}",
  "::-webkit-scrollbar-track,::-webkit-scrollbar-corner{background:transparent}",
  "::-webkit-scrollbar-button{display:none}",
  "::-webkit-scrollbar-thumb{border:3px solid transparent;border-radius:999px;background:#c9c8d3 padding-box}",
  ":hover::-webkit-scrollbar-thumb{background-color:#8f8fa3}",
  "@supports not selector(::-webkit-scrollbar){*{scrollbar-width:thin;scrollbar-color:#c9c8d3 transparent}}",
].join("");

/**
 * Colours of a plain-text email (no colours of its own), so it can follow
 * the app's theme instead of a white sheet. Fixed hex values: the frame
 * can't see the app's CSS variables. Match packages/ui themes.ts
 * (paperFg / accText of each theme).
 */
export type TextScheme = "light" | "dark";

const TEXT_SCHEME_STYLE: Readonly<Record<TextScheme, string>> = {
  dark: [
    ":root{color-scheme:dark}",
    "body{color:#E2E2EE}",
    "a{color:#A5A0FF}",
    "::-webkit-scrollbar-thumb{background-color:#3a3c58}",
    ":hover::-webkit-scrollbar-thumb{background-color:#5a5c7a}",
  ].join(""),
  light: ["body{color:#1F2033}", "a{color:#4338CA}"].join(""),
};

/**
 * Plain text reads like the app (reading size, no inner padding: the
 * reader aligns it with the header), in the theme's colours.
 */
export const textStyle = (scheme: TextScheme): string =>
  [
    "html,body{background:transparent}",
    "body{padding:0 0 2px;font-size:14.5px;line-height:1.6}",
    TEXT_SCHEME_STYLE[scheme],
  ].join("");

export interface FrameDocumentOptions {
  readonly csp: string;
  readonly lang?: string | null;
  readonly dir?: string | null;
  /**
   * The page's CSP nonce, set on our base `<style>` BEFORE it is inserted:
   * Chromium checks `style-src` when a `<style>` is connected, even in a
   * document that is never shown, and reports a violation.
   */
  readonly styleNonce?: string | undefined;
  /** Plain text only: adds `textStyle(scheme)` after the base style. */
  readonly textScheme?: TextScheme | undefined;
}

const LANG = /^[a-z]{2,8}(?:-[a-z\d]{1,8})*$/iu;

/**
 * A fresh document whose `<head>` starts with: CSP, referrer policy, DNS
 * prefetch off, base style. Content is appended by the caller after these.
 */
export const createFrameDocument = (
  implementation: DOMImplementation,
  options: FrameDocumentOptions
): Document => {
  const doc = implementation.createHTMLDocument("");
  const { head } = doc;
  while (head.firstChild !== null) {
    head.firstChild.remove();
  }
  const csp = doc.createElement("meta");
  csp.setAttribute("http-equiv", "Content-Security-Policy");
  csp.setAttribute("content", options.csp);
  const referrer = doc.createElement("meta");
  referrer.setAttribute("name", "referrer");
  referrer.setAttribute("content", "no-referrer");
  const prefetch = doc.createElement("meta");
  prefetch.setAttribute("http-equiv", "x-dns-prefetch-control");
  prefetch.setAttribute("content", "off");
  const style = doc.createElement("style");
  if (options.styleNonce !== undefined) {
    style.setAttribute("nonce", options.styleNonce);
  }
  style.textContent =
    options.textScheme === undefined
      ? BASE_STYLE
      : BASE_STYLE + textStyle(options.textScheme);
  // CSP first. No charset meta: srcdoc is already a string.
  head.append(csp, referrer, prefetch, style);
  if (typeof options.lang === "string" && LANG.test(options.lang)) {
    doc.documentElement.setAttribute("lang", options.lang);
  }
  if (
    options.dir === "rtl" ||
    options.dir === "ltr" ||
    options.dir === "auto"
  ) {
    doc.documentElement.setAttribute("dir", options.dir);
  }
  return doc;
};

export const serializeFrameDocument = (doc: Document): string =>
  `<!DOCTYPE html>${doc.documentElement.outerHTML}`;
