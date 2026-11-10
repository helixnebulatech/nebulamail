// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The proof that a srcdoc was made (and verified) by `renderEmail`: an
// object only this module can create, checked with a private field (which
// a look-alike object or a copy doesn't have). Not in the package exports;
// only render.ts mints them. Trust travels with the result, so there is
// no list of remembered strings to evict: a frame is valid as long as
// someone holds it (an open conversation can have 300 emails).
//
// The `nebula-mail-srcdoc` policy is here too (not in trusted-types.ts):
// the check needs this class's private field, and trusted-types.ts must
// not import local modules at run time (vite.config.ts loads it with Node).

import { createPolicy, throughPolicy } from "./trusted-types";
import type { Policy, TrustedHtml } from "./trusted-types";

const MINT = Symbol("mint");

export class RenderedFrame {
  readonly #html: string;

  constructor(mint: symbol, html: string) {
    // `new (frame.constructor)(html)` from outside must not work either.
    if (mint !== MINT) {
      throw new TypeError("RenderedFrame is only made by renderEmail");
    }
    this.#html = html;
  }

  /** The verified srcdoc. */
  get html(): string {
    return this.#html;
  }

  static isRendered(value: unknown): value is RenderedFrame {
    return typeof value === "object" && value !== null && #html in value;
  }
}

/**
 * The same object for the same srcdoc while someone holds it, so effects
 * that depend on it don't reload the iframe after a re-render.
 */
const frames = new Map<string, WeakRef<RenderedFrame>>();
const registry = new FinalizationRegistry<string>((html) => {
  // A newer frame for the same srcdoc may be there already: keep it.
  if (frames.get(html)?.deref() === undefined) {
    frames.delete(html);
  }
});

/** Only for the srcdoc `renderEmail` returns, after its final check. */
export const mintRenderedFrame = (html: string): RenderedFrame => {
  const known = frames.get(html)?.deref();
  if (known !== undefined) {
    return known;
  }
  const frame = new RenderedFrame(MINT, html);
  frames.set(html, new WeakRef(frame));
  registry.register(frame, html);
  return frame;
};

let srcdocPolicy: Policy | null | undefined;

/** The HTML `trustedSrcdoc` is passing to the srcdoc policy right now. */
let pending: string | undefined;

/** The `frame` of a `renderEmail` result, ready for `iframe.srcdoc`. */
export const trustedSrcdoc = (frame: RenderedFrame): TrustedHtml => {
  if (!RenderedFrame.isRendered(frame)) {
    throw new Error("Not a srcdoc made by renderEmail");
  }
  const { html } = frame;
  srcdocPolicy ??= createPolicy("nebula-mail-srcdoc", {
    createHTML: (input) => (input === pending ? input : null),
  });
  pending = html;
  try {
    return throughPolicy(srcdocPolicy, html);
  } finally {
    pending = undefined;
  }
};

/**
 * The `frame` of a `renderEmail` result as an inert document, to move into
 * a frame page (frame.ts `fillFramePage`). Same rule as `trustedSrcdoc`.
 */
export const parseRenderedSrcdoc = (
  frame: RenderedFrame,
  parser: Pick<DOMParser, "parseFromString"> = new DOMParser()
): Document => parser.parseFromString(trustedSrcdoc(frame), "text/html");
