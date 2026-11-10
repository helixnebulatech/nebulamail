// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Printing one email: a hidden iframe with the same srcdoc
// the reader shows (already sanitized, same CSP, only the images the user
// already allowed) and PRINT_IFRAME_SANDBOX (the reader's sandbox plus
// `allow-modals`, which `print()` needs; still no scripts). The app adds a
// small header (subject, from, to, date) as DOM nodes with textContent,
// never as HTML, then calls print() on the frame and removes it once the
// browser has printed (`afterprint`, see PrintFrame).

import type { RenderedFrame } from "@nebulamail/mail-render";
import {
  IFRAME_REFERRER_POLICY,
  PRINT_IFRAME_SANDBOX,
  fillFramePage,
  trustedSrcdoc,
} from "@nebulamail/mail-render";
import * as stylex from "@stylexjs/stylex";
import { useEffect, useEffectEvent, useRef } from "react";

const styles = stylex.create({
  // Out of sight but laid out (visibility doesn't cross into the frame's
  // document, so the printout isn't blank).
  frame: {
    borderStyle: "none",
    bottom: 0,
    height: 0,
    pointerEvents: "none",
    position: "fixed",
    right: 0,
    visibility: "hidden",
    width: 0,
  },
});

export interface PrintHeader {
  readonly subject: string;
  /** Label/value rows, e.g. ["From", "Ana <ana@x.com>"]. */
  readonly rows: readonly (readonly [string, string])[];
}

/** Builds the header with DOM APIs and CSSOM styles (no HTML strings, no CSP impact). */
const buildHeader = (doc: Document, header: PrintHeader): HTMLElement => {
  const box = doc.createElement("header");
  Object.assign(box.style, {
    borderBottom: "1px solid #ccc",
    font: "13px/1.4 system-ui, sans-serif",
    marginBottom: "16px",
    paddingBottom: "8px",
  });
  const title = doc.createElement("h1");
  title.textContent = header.subject;
  Object.assign(title.style, { fontSize: "18px", margin: "0 0 6px" });
  box.append(title);
  for (const [label, value] of header.rows) {
    if (value === "") {
      continue;
    }
    const row = doc.createElement("div");
    const name = doc.createElement("strong");
    name.textContent = `${label}: `;
    row.append(name, doc.createTextNode(value));
    box.append(row);
  }
  return box;
};

/**
 * If `afterprint` never comes (print() ignored, a headless browser), the
 * frame goes after this long anyway. By then Firefox has made its copy.
 */
const AFTERPRINT_TIMEOUT_MS = 60_000;

/** In a frame page the images load after `load`: wait for them (a while). */
const IMAGES_TIMEOUT_MS = 10_000;

const imagesLoaded = async (doc: Document): Promise<void> => {
  const { promise, resolve } = Promise.withResolvers<undefined>();
  const timer = setTimeout(resolve, IMAGES_TIMEOUT_MS);
  await Promise.race([
    Promise.allSettled(
      [...doc.images].map(async (image) => {
        await image.decode();
      })
    ),
    promise,
  ]);
  clearTimeout(timer);
};

export const PrintFrame = ({
  header,
  onDone,
  page,
  rendered,
  title,
}: {
  /** `renderEmail(…).frame`, the same one the reader shows. */
  readonly rendered: RenderedFrame;
  /** The reader's frame page, if it shows the email in one (EmailFrame). */
  readonly page?: string | undefined;
  readonly header: PrintHeader;
  readonly title: string;
  /** Printing finished or was cancelled (the frame can go). */
  readonly onDone: () => void;
}) => {
  const frame = useRef<HTMLIFrameElement>(null);
  /** Stops waiting for `afterprint` (unmounted, or done). */
  const stopWaiting = useRef<(() => void) | null>(null);
  const print = useEffectEvent((iframe: HTMLIFrameElement) => {
    const doc = iframe.contentDocument;
    const win = iframe.contentWindow;
    if (doc === null || win === null) {
      onDone();
      return;
    }
    doc.body.prepend(buildHeader(doc, header));
    // Chromium and Safari block in print() until the dialog closes, and
    // `afterprint` has come when it returns. Firefox returns at once and
    // opens its preview later (`beforeprint`, then `afterprint` when it
    // closes): removing the frame right away leaves it nothing to print.
    // Only listeners added before print() hear `afterprint` there (the
    // frame is frozen after it). Checked in headless Firefox, 2026-11-03.
    let returned = false;
    let printedAlready = false;
    const done = (): void => {
      stopWaiting.current?.();
      // From the page's own task, not the frame's print event: called
      // from there, headless Chromium often never re-rendered (the frame
      // stayed; checked 2026-11-03).
      setTimeout(onDone, 0);
    };
    const onAfterPrint = (): void => {
      if (returned) {
        done();
      } else {
        printedAlready = true;
      }
    };
    win.addEventListener("afterprint", onAfterPrint, { once: true });
    win.print();
    returned = true;
    if (printedAlready) {
      onDone();
      return;
    }
    const timer = setTimeout(done, AFTERPRINT_TIMEOUT_MS);
    stopWaiting.current = () => {
      clearTimeout(timer);
      win.removeEventListener("afterprint", onAfterPrint);
      stopWaiting.current = null;
    };
  });

  const finish = useEffectEvent(() => {
    onDone();
  });

  useEffect(() => {
    const iframe = frame.current;
    let active = true;
    // `load` waits for the allowed images, so they are in the printout.
    const onLoad = (): void => {
      if (iframe === null) {
        return;
      }
      if (page === undefined) {
        print(iframe);
        return;
      }
      const doc = iframe.contentDocument;
      let filled = false;
      try {
        filled = doc !== null && fillFramePage(iframe, rendered);
      } catch {
        // Not a document renderEmail made: nothing to print.
      }
      if (doc === null || !filled) {
        finish();
        return;
      }
      void imagesLoaded(doc).then(() => {
        if (active) {
          print(iframe);
        }
        return null;
      });
    };
    iframe?.addEventListener("load", onLoad, { once: true });
    if (iframe !== null) {
      if (page === undefined) {
        iframe.srcdoc = trustedSrcdoc(rendered);
      } else {
        iframe.src = page;
      }
    }
    return () => {
      active = false;
      iframe?.removeEventListener("load", onLoad);
      stopWaiting.current?.();
    };
  }, [page, rendered]);

  return (
    <iframe
      aria-hidden
      ref={frame}
      referrerPolicy={IFRAME_REFERRER_POLICY}
      sandbox={PRINT_IFRAME_SANDBOX}
      tabIndex={-1}
      title={title}
      {...stylex.props(styles.frame)}
    />
  );
};
