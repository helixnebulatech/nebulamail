// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Images. After Proton every loading
// reference is inert (`proton-src`, `proton-url(`…). Here each one is
// decided once and either restored to a safe value or removed:
//   - `data:image/*` rasters: always shown (SVG never, see urls.ts);
//   - `cid:`: shown when the caller resolved it to a `data:` URL;
//   - trackers (list + spy-pixel rule): always removed and reported;
//   - remote http(s): only when allowed, and only as the URL the image
//     source returns (proxy ticket, batch `blob:`), never the original;
//   - anything else (relative, protocol-relative, other schemes): removed.

import {
  HTTPS_POLICY,
  isNonPublicName,
  parseTargetUrl,
} from "@nebulamail/net-guard";

import { finalizeStyleText, neutralizeCss, rewriteCssUrls } from "./css";
import { createImageDecider } from "./image-policy";
import type {
  ImageDecider,
  ImageDecision,
  TrackerSettings,
} from "./image-policy";
import { SPY_PIXEL_SERVICE } from "./report";
import type { ReportBuilder } from "./report";
import type { SpyPixelInput } from "./trackers/spy-pixel";
import { classifyImageUrl, parseSrcset } from "./urls";

/** Turns a remote image URL from the email into the URL to load, or null. */
export type ImageUrlRewriter = (url: string) => string | null;

/**
 * Where remote images may come from:
 *   - `none`: never loaded;
 *   - `proxy`: through the image proxy; `rewrite` returns ticket URLs on `origin`;
 *   - `batch`: the app fetched them in one request and `rewrite`
 *     returns `blob:` URLs;
 *   - `direct`: straight from the sender (reveals the IP; only if the admin
 *     allows it and `loadThroughProxy` is off). Public https on 443 only.
 */
export type ImageSource =
  | { readonly kind: "none" }
  | {
      readonly kind: "proxy";
      readonly origin: string;
      readonly rewrite: ImageUrlRewriter;
    }
  | { readonly kind: "batch"; readonly rewrite: ImageUrlRewriter }
  | { readonly kind: "direct" };

/** `cid:` resolution: Content-ID (normalized, see `normalizeCid`) → `data:` URL. */
export type CidResolver =
  | ReadonlyMap<string, string>
  | ((cid: string) => string | null | undefined);

export interface ImageContext {
  readonly settings: TrackerSettings;
  readonly loadRemote: boolean;
  /** Already validated (see document.ts). */
  readonly source: ImageSource;
  readonly cids?: CidResolver;
  readonly report: ReportBuilder;
  readonly neededCids: Set<string>;
}

/** The context while walking one document: one decider per email. */
interface WalkContext extends ImageContext {
  readonly decide: ImageDecider;
}

const resolveCid = (
  resolver: CidResolver | undefined,
  cid: string
): string | null => {
  if (resolver === undefined) {
    return null;
  }
  const value =
    typeof resolver === "function" ? resolver(cid) : resolver.get(cid);
  return value ?? null;
};

/**
 * Direct mode: the user's browser fetches the sender's URL, from inside the
 * user's network. Only https on 443 to a public IP literal or a name that
 * can be public (net-guard's static checks: no `192.168.1.1`, `[::1]`,
 * `localhost`, `nas.local`, `router.lan`, `intranet`…). Otherwise an email
 * could make the browser send GETs to the LAN (like CVE-2026-48845 in
 * Roundcube). Left: a public name that resolves to a private
 * address, which the browser doesn't let us check (docs/static.md).
 */
const directUrl = (candidate: string): string | null => {
  const parsed = parseTargetUrl(candidate, HTTPS_POLICY);
  if (!parsed.ok) {
    return null;
  }
  const { host, href } = parsed.target;
  return host.kind === "name" && isNonPublicName(host.name) ? null : href;
};

/**
 * Only accept what the image source is supposed to return: a URL on the
 * proxy origin, a `blob:` URL in batch mode, a public https URL in direct mode,
 * or an inline raster `data:` URL. Anything else is dropped.
 */
export const validateLoadableUrl = (
  candidate: string | null,
  source: ImageSource
): string | null => {
  if (candidate === null) {
    return null;
  }
  const classified = classifyImageUrl(candidate);
  if (classified.kind === "inline") {
    return classified.url;
  }
  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    return null;
  }
  if (parsed.username !== "" || parsed.password !== "") {
    return null;
  }
  switch (source.kind) {
    case "proxy": {
      return parsed.origin === source.origin ? parsed.href : null;
    }
    case "batch": {
      return parsed.protocol === "blob:" ? parsed.href : null;
    }
    case "direct": {
      return directUrl(candidate);
    }
    case "none": {
      return null;
    }
    default: {
      return null;
    }
  }
};

const loadRemote = (url: string, ctx: ImageContext): string | null => {
  if (!ctx.loadRemote) {
    ctx.report.remoteImage(url, false);
    return null;
  }
  let candidate: string | null;
  switch (ctx.source.kind) {
    case "proxy":
    case "batch": {
      candidate = ctx.source.rewrite(url);
      break;
    }
    case "direct": {
      candidate = url;
      break;
    }
    case "none": {
      candidate = null;
      break;
    }
    default: {
      candidate = null;
    }
  }
  const loadable = validateLoadableUrl(candidate, ctx.source);
  ctx.report.remoteImage(url, loadable !== null);
  return loadable;
};

/** Applies a decision: returns the URL to put in the document, or null. */
const apply = (decision: ImageDecision, ctx: ImageContext): string | null => {
  switch (decision.kind) {
    case "inline": {
      return decision.url;
    }
    case "cid": {
      ctx.neededCids.add(decision.cid);
      const resolved = resolveCid(ctx.cids, decision.cid);
      if (resolved === null) {
        return null;
      }
      const classified = classifyImageUrl(resolved);
      return classified.kind === "inline" ? classified.url : null;
    }
    case "tracker": {
      ctx.report.tracker(decision.service, decision.url);
      return null;
    }
    case "spy-pixel": {
      ctx.report.tracker(SPY_PIXEL_SERVICE, decision.url);
      return null;
    }
    case "remote": {
      return loadRemote(decision.url, ctx);
    }
    case "blocked": {
      ctx.report.unsafeImage(decision.reason);
      return null;
    }
    default: {
      return null;
    }
  }
};

const isTracking = (decision: ImageDecision): boolean =>
  decision.kind === "tracker" || decision.kind === "spy-pixel";

export const dimensionsOf = (element: Element): SpyPixelInput => ({
  height: element.getAttribute("height"),
  style: element.getAttribute("style"),
  width: element.getAttribute("width"),
});

/** Proton's renamed loading attributes that we never restore. */
const DROPPED_PROTON_ATTRIBUTES = [
  "proton-data-src",
  "proton-poster",
  "proton-xlink:href",
  "proton-href",
];

const processImg = (img: Element, ctx: WalkContext): void => {
  const dims = dimensionsOf(img);
  const src = img.getAttribute("proton-src");
  const srcset = img.getAttribute("proton-srcset");
  img.removeAttribute("proton-src");
  img.removeAttribute("proton-srcset");
  const srcDecision = src === null ? null : ctx.decide(src, dims);
  const candidates = (srcset === null ? [] : parseSrcset(srcset)).map(
    (candidate) => ({
      decision: ctx.decide(candidate.url, dims),
      descriptor: candidate.descriptor,
    })
  );
  const decisions = [
    ...(srcDecision === null ? [] : [srcDecision]),
    ...candidates.map((c) => c.decision),
  ];
  if (decisions.some(isTracking)) {
    // A tracking image is removed with its element (and its other
    // candidates are not loaded either), so nothing reveals it was there.
    for (const decision of decisions.filter(isTracking)) {
      apply(decision, ctx);
    }
    img.remove();
    return;
  }
  if (srcDecision !== null) {
    const url = apply(srcDecision, ctx);
    if (url !== null) {
      img.setAttribute("src", url);
    }
  }
  const kept: string[] = [];
  for (const { decision, descriptor } of candidates) {
    const url = apply(decision, ctx);
    // srcset URLs end at whitespace and commas split candidates: values
    // with either (data: URLs) cannot be written back safely.
    if (url !== null && !/[\s,]/u.test(url)) {
      kept.push(descriptor === "" ? url : `${url} ${descriptor}`);
    }
  }
  if (kept.length > 0) {
    img.setAttribute("srcset", kept.join(", "));
  }
};

const processBackground = (element: Element, ctx: WalkContext): void => {
  const background = element.getAttribute("proton-background");
  element.removeAttribute("proton-background");
  if (background === null) {
    return;
  }
  const url = apply(ctx.decide(background, dimensionsOf(element)), ctx);
  if (url !== null) {
    element.setAttribute("background", url);
  }
};

/** Neutralize, then rewrite every `proton-url(…)` reference in `css`. */
const processCss = (
  css: string,
  ctx: WalkContext,
  dims?: SpyPixelInput
): string =>
  rewriteCssUrls(neutralizeCss(css), (ref) => {
    if (ref.context !== "image") {
      // Stylesheet imports and web fonts never load (CSP: style-src
      // 'unsafe-inline', font-src data:), and are not images to ticket.
      ctx.report.unsafeImage(ref.context);
      return null;
    }
    return apply(ctx.decide(ref.url, dims), ctx);
  });

export const processImages = (doc: Document, context: ImageContext): void => {
  const ctx: WalkContext = {
    ...context,
    decide: createImageDecider(context.settings),
  };
  for (const element of doc.querySelectorAll("*")) {
    if (!element.isConnected) {
      continue;
    }
    const style = element.getAttribute("style");
    if (style !== null) {
      const rewritten = processCss(style, ctx, dimensionsOf(element));
      if (rewritten.trim() === "") {
        element.removeAttribute("style");
      } else if (rewritten !== style) {
        element.setAttribute("style", rewritten);
      }
    }
    if (element.localName === "style") {
      element.textContent = finalizeStyleText(
        processCss(element.textContent, ctx)
      );
    }
    for (const name of DROPPED_PROTON_ATTRIBUTES) {
      element.removeAttribute(name);
    }
    processBackground(element, ctx);
    if (element.localName === "img") {
      processImg(element, ctx);
    } else {
      element.removeAttribute("proton-src");
      element.removeAttribute("proton-srcset");
    }
  }
};
