// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The email body: one sandboxed srcdoc iframe per email.
// `allow-same-origin` only, so the app can capture link clicks and
// measure the height; no scripts, forms, popups or top navigation.

import type { FrameLink, RenderedFrame } from "@nebulamail/mail-render";
import {
  FRAME_CONTAIN,
  IFRAME_REFERRER_POLICY,
  IFRAME_SANDBOX,
  attachFrame,
  fillFramePage,
  trustedSrcdoc,
} from "@nebulamail/mail-render";
import * as stylex from "@stylexjs/stylex";
import { useEffect, useEffectEvent, useRef, useState } from "react";

const styles = stylex.create({
  frame: {
    backgroundColor: "transparent",
    borderStyle: "none",
    display: "block",
    width: "100%",
  },
  // The same scheme as the document inside: when they differ the browser
  // paints an opaque backdrop behind it (CSS Color Adjust §2.3).
  light: { colorScheme: "light" },
  dark: { colorScheme: "dark" },
});

export interface EmailFrameProps {
  /** `renderEmail(…).frame`: the same object while the render is the same. */
  readonly rendered: RenderedFrame;
  /**
   * Show it in this same-origin page instead of as `srcdoc` (mail-render
   * `fillFramePage`): its CSP, not the app's, decides what loads.
   */
  readonly page?: string | undefined;
  /** Accessible name, e.g. the subject. */
  readonly title: string;
  /** A link was activated inside the email; the app decides what to do. */
  readonly onLink: (link: FrameLink) => void;
  /** The email has loaded and been fitted to the width (shown for real). */
  readonly onReady?: () => void;
  /**
   * The document's colour scheme: "dark" only for plain text rendered with
   * mail-render `textScheme: "dark"`. Default "light".
   */
  readonly scheme?: "light" | "dark";
}

/**
 * Until the content is measured, and the least it takes: one line, so a
 * short email isn't a tall empty sheet.
 */
const MIN_HEIGHT = 24;
/** Below this the text is unreadable; the rest scrolls sideways instead. */
const MIN_ZOOM = 0.4;

/**
 * Emails built for desktop (fixed 600-800 px tables) are zoomed out to the
 * width of the iframe, like mail apps do on phones. Only our own `zoom` on
 * the root of the already sanitized document; the email's HTML is untouched.
 */
const fitWidth = (iframe: HTMLIFrameElement): number | null => {
  // No document yet, or an empty one (no root element).
  const root = iframe.contentDocument?.documentElement;
  if (root === undefined || root === null) {
    return null;
  }
  root.style.removeProperty("zoom");
  const available = iframe.clientWidth;
  const natural = root.scrollWidth;
  if (available > 0 && natural > available + 1) {
    root.style.setProperty(
      "zoom",
      String(Math.max(MIN_ZOOM, available / natural))
    );
  }
  // The new content height: zoom doesn't resize the root, so the height
  // observer of attachFrame doesn't see it.
  return root.getBoundingClientRect().height;
};

/**
 * Puts the email in the iframe: as `srcdoc`, or in the frame page. Call it
 * after every other `load` listener of the effect is registered except
 * these: the page's own `load` must fill it before they run.
 */
const showEmail = (
  iframe: HTMLIFrameElement,
  rendered: RenderedFrame,
  page: string | undefined
): (() => void) | undefined => {
  if (page === undefined) {
    iframe.srcdoc = trustedSrcdoc(rendered);
    return undefined;
  }
  const fill = (): void => {
    // Once: the page is only loaded once.
    iframe.removeEventListener("load", fill);
    fillFramePage(iframe, rendered);
  };
  iframe.addEventListener("load", fill);
  // `srcdoc` wins over `src`: drop it if this iframe had one.
  iframe.removeAttribute("srcdoc");
  iframe.src = page;
  return () => {
    iframe.removeEventListener("load", fill);
  };
};

export const EmailFrame = ({
  onLink,
  onReady,
  page,
  rendered,
  scheme = "light",
  title,
}: EmailFrameProps) => {
  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(MIN_HEIGHT);
  const handleLink = useEffectEvent((link: FrameLink) => {
    onLink(link);
  });
  const handleReady = useEffectEvent(() => {
    onReady?.();
  });

  useEffect(() => {
    const iframe = frame.current;
    // In page mode the page is filled first, on its own `load`.
    const prepared =
      iframe === null || page === undefined
        ? undefined
        : showEmail(iframe, rendered, page);
    // Attached before srcdoc is set below, so the first `load` is seen.
    const detach =
      iframe === null
        ? undefined
        : attachFrame(iframe, {
            onHeight: (next) => {
              setHeight(Math.max(MIN_HEIGHT, Math.ceil(next)));
            },
            onLink: (link) => {
              handleLink(link);
            },
          });
    // Keyboard shortcuts keep working after a click inside the email: key
    // presses in the iframe document are replayed on the app window.
    const forwardKeys = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.key === "Tab") {
        return;
      }
      const replay = new KeyboardEvent("keydown", {
        altKey: event.altKey,
        bubbles: true,
        cancelable: true,
        code: event.code,
        ctrlKey: event.ctrlKey,
        key: event.key,
        metaKey: event.metaKey,
        shiftKey: event.shiftKey,
      });
      globalThis.dispatchEvent(replay);
      if (replay.defaultPrevented) {
        event.preventDefault();
      }
    };
    // Refit when the iframe gets narrower or wider (rotation, panels),
    // not when only its height follows the content.
    let fittedWidth = 0;
    const refit = (): void => {
      if (iframe !== null && iframe.clientWidth !== fittedWidth) {
        fittedWidth = iframe.clientWidth;
        const content = fitWidth(iframe);
        if (content !== null) {
          setHeight(Math.max(MIN_HEIGHT, Math.ceil(content)));
        }
      }
    };
    const resize =
      iframe === null || typeof ResizeObserver === "undefined"
        ? undefined
        : new ResizeObserver(refit);
    const onLoad = (): void => {
      iframe?.contentDocument?.addEventListener("keydown", forwardKeys);
      fittedWidth = 0;
      refit();
      handleReady();
    };
    iframe?.addEventListener("load", onLoad);
    if (iframe !== null) {
      resize?.observe(iframe);
    }
    const shown =
      iframe === null || page !== undefined
        ? prepared
        : showEmail(iframe, rendered, undefined);
    return () => {
      shown?.();
      detach?.();
      resize?.disconnect();
      iframe?.removeEventListener("load", onLoad);
      iframe?.contentDocument?.removeEventListener("keydown", forwardKeys);
    };
  }, [page, rendered]);

  return (
    <iframe
      ref={frame}
      referrerPolicy={IFRAME_REFERRER_POLICY}
      sandbox={IFRAME_SANDBOX}
      title={title}
      // Height follows the content (ResizeObserver inside attachFrame).
      // `contain` comes from mail-render (one source of truth), which StyleX
      // can't read at build time. Client-only element: set through the
      // CSSOM, so the page CSP (no inline style attributes) isn't involved.
      // oxlint-disable-next-line react/forbid-dom-props -- dynamic value; StyleX can't express a per-email pixel height.
      style={{ contain: FRAME_CONTAIN, height }}
      {...stylex.props(styles.frame, styles[scheme])}
    />
  );
};
