// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Framework-free helpers for the iframe that shows an email.
// The app renders:
//   <iframe sandbox={IFRAME_SANDBOX} referrerPolicy="no-referrer"
//           style={{ contain: FRAME_CONTAIN }} />
// and sets `iframe.srcdoc = trustedSrcdoc(result.frame)` (or uses
// `fillFramePage(iframe, result.frame)`).
// One iframe per email, so one email's CSS cannot see another's.

import { parseRenderedSrcdoc } from "./rendered-frame";
import type { RenderedFrame } from "./rendered-frame";

/**
 * Exactly this and nothing more: no scripts, forms, popups, top
 * navigation. `allow-same-origin` only lets the app capture clicks and
 * measure the height; without `allow-scripts` the email cannot run code.
 */
export const IFRAME_SANDBOX = "allow-same-origin";
/**
 * The hidden iframe used to print one email: the same sandbox plus
 * `allow-modals`, because browsers ignore `print()` on a document whose
 * sandbox blocks modals, even when the parent calls it. Without
 * `allow-scripts` the email still cannot call `print()`, `alert()` or
 * anything else itself: only the app can.
 */
export const PRINT_IFRAME_SANDBOX = `${IFRAME_SANDBOX} allow-modals`;
export const IFRAME_REFERRER_POLICY = "no-referrer";
/** CSS `contain` for the iframe element (§9.1 heavy-CSS DoS); no `size`, so auto-height works. */
export const FRAME_CONTAIN = "layout paint style";

export interface FrameLink {
  /** The URL as resolved by the browser (`anchor.href`), to pass to resolveLinkForOpening. */
  readonly href: string;
  /** Visible text (or `alt` for `<area>`), for the text/target mismatch check. */
  readonly text: string;
  readonly element: Element;
}

const ELEMENT_NODE = 1;

const hasHref = (element: Element): element is Element & { href: unknown } =>
  "href" in element;

// Duck-typed guards: nodes and events come from the iframe's realm, where
// this realm's `instanceof Node` / `KeyboardEvent` checks fail.
const isNode = (value: unknown): value is Node =>
  typeof value === "object" &&
  value !== null &&
  "nodeType" in value &&
  "parentElement" in value;

const isElement = (node: Node): node is Element =>
  node.nodeType === ELEMENT_NODE && "closest" in node;

const isNonEnterKey = (event: Event): boolean =>
  event.type === "keydown" && "key" in event && event.key !== "Enter";

const preventDefault = (event: Event): void => {
  event.preventDefault();
};

/**
 * The link an event happened on, if any. Works across realms (the iframe
 * has its own `HTMLAnchorElement`, so no `instanceof`).
 */
export const findLinkFromEvent = (event: Event): FrameLink | null => {
  const { target } = event;
  if (!isNode(target)) {
    return null;
  }
  const element = isElement(target) ? target : target.parentElement;
  const link = element?.closest("a[href], area[href]") ?? null;
  if (link === null || !hasHref(link) || typeof link.href !== "string") {
    return null;
  }
  const text =
    link.localName === "area"
      ? (link.getAttribute("alt") ?? "")
      : (link.textContent ?? "");
  return { element: link, href: link.href, text: text.trim() };
};

export interface FrameHandlers {
  /** A link was activated (click, middle click, Enter). Default navigation is always prevented. */
  readonly onLink?: (
    link: FrameLink,
    event: MouseEvent | KeyboardEvent
  ) => void;
  /** Content height in CSS pixels, on load and whenever it changes. */
  readonly onHeight?: (height: number) => void;
}

/**
 * Wires click capture and auto-height on an email iframe. Call it before
 * setting `srcdoc` (it listens for `load`). Returns a cleanup function.
 */
export const attachFrame = (
  iframe: HTMLIFrameElement,
  handlers: FrameHandlers
): (() => void) => {
  let cleanupDocument: (() => void) | undefined;

  const onLoad = (): void => {
    cleanupDocument?.();
    const doc = iframe.contentDocument;
    if (doc === null) {
      return;
    }
    const onActivate = (event: MouseEvent | KeyboardEvent): void => {
      if (isNonEnterKey(event)) {
        return;
      }
      const link = findLinkFromEvent(event);
      if (link !== null) {
        event.preventDefault();
        event.stopPropagation();
        handlers.onLink?.(link, event);
      }
    };
    const onClick = (event: MouseEvent): void => {
      onActivate(event);
    };
    const onKey = (event: KeyboardEvent): void => {
      onActivate(event);
    };
    doc.addEventListener("click", onClick, true);
    doc.addEventListener("auxclick", onClick, true);
    doc.addEventListener("keydown", onKey, true);
    doc.addEventListener("submit", preventDefault, true);
    doc.addEventListener("dragstart", preventDefault, true);

    let observer: ResizeObserver | undefined;
    const report = (): void =>
      handlers.onHeight?.(doc.documentElement.scrollHeight);
    if (handlers.onHeight !== undefined) {
      report();
      if (typeof ResizeObserver !== "undefined") {
        observer = new ResizeObserver(report);
        observer.observe(doc.documentElement);
      }
    }
    cleanupDocument = (): void => {
      doc.removeEventListener("click", onClick, true);
      doc.removeEventListener("auxclick", onClick, true);
      doc.removeEventListener("keydown", onKey, true);
      doc.removeEventListener("submit", preventDefault, true);
      doc.removeEventListener("dragstart", preventDefault, true);
      observer?.disconnect();
    };
  };

  iframe.addEventListener("load", onLoad);
  return () => {
    iframe.removeEventListener("load", onLoad);
    cleanupDocument?.();
  };
};

/**
 * Shows a rendered email in a frame *page* instead of `srcdoc`: a
 * same-origin page with its own CSP, already loaded in `iframe` (sandboxed
 * as above). A `srcdoc` document shares the app's CSP, so it can only load
 * what the app may; the page's own CSP decides instead (the static build
 * loads images straight from the sender this way). The
 * email's document replaces the page's; the page's CSP and referrer policy
 * stay (they belong to the document, not to its elements), and the email
 * brings its own CSP too. Only documents made by `renderEmail` are taken.
 */
export const fillFramePage = (
  iframe: HTMLIFrameElement,
  frame: RenderedFrame
): boolean => {
  const doc = iframe.contentDocument;
  if (doc === null) {
    return false;
  }
  const parsed = parseRenderedSrcdoc(frame);
  doc.documentElement.replaceWith(doc.importNode(parsed.documentElement, true));
  return true;
};
