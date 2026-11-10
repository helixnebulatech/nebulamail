// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser side of the real-browser hostile-email tests: renders one email
// with the real pipeline into the real sandboxed srcdoc iframe, the way the
// app does. Bundled by e2e/hostile.spec.ts with `bun build`.

import {
  DEFAULT_PRIVACY,
  IFRAME_REFERRER_POLICY,
  IFRAME_SANDBOX,
  renderEmail,
} from "../src/index";

export type Layer = "all" | "no-page-csp" | "sanitizer-only";

export interface HarnessOptions {
  /** Show remote images, through the (fake) proxy origin. */
  readonly images: boolean;
  /**
   * Which defences stay on. "sanitizer-only" also strips the iframe's own
   * CSP meta, to prove the sanitizer alone loads nothing.
   */
  readonly layer: Layer;
  readonly nonce: string;
  readonly proxyOrigin: string;
  /** Control only: put the HTML in the iframe without the pipeline. */
  readonly raw?: boolean;
}

export interface HarnessResult {
  readonly view: "html" | "text";
  readonly fallback: string | null;
  readonly text: string;
}

const CSP_META = /<meta http-equiv="Content-Security-Policy"[^>]*>/iu;

const render = async (
  html: string,
  options: HarnessOptions
): Promise<HarnessResult> => {
  const result = renderEmail(
    { html },
    DEFAULT_PRIVACY,
    options.images
      ? {
          images: {
            kind: "proxy",
            origin: options.proxyOrigin,
            rewrite: (url) =>
              `${options.proxyOrigin}/i/${encodeURIComponent(url)}`,
          },
          styleNonce: options.nonce,
          userApproved: true,
        }
      : { styleNonce: options.nonce }
  );
  const rendered =
    options.layer === "sanitizer-only"
      ? result.srcdoc.replace(CSP_META, "")
      : result.srcdoc;
  const srcdoc = options.raw === true ? html : rendered;
  const iframe = document.createElement("iframe");
  iframe.setAttribute("sandbox", IFRAME_SANDBOX);
  iframe.referrerPolicy = IFRAME_REFERRER_POLICY;
  const loaded = Promise.withResolvers<undefined>();
  iframe.addEventListener("load", () => {
    loaded.resolve(undefined);
  });
  iframe.srcdoc = srcdoc;
  document.body.append(iframe);
  await loaded.promise;
  // Give lazy loads, CSS animations and timers a chance to fire.
  const settled = Promise.withResolvers<undefined>();
  setTimeout(() => {
    settled.resolve(undefined);
  }, 400);
  await settled.promise;
  return {
    fallback: result.fallbackReason ?? null,
    text: iframe.contentDocument?.body.textContent ?? "",
    view: result.view,
  };
};

declare global {
  // oxlint-disable-next-line typescript/consistent-type-definitions -- global augmentation must be an interface.
  interface Window {
    renderHostile: typeof render;
  }
}

globalThis.window.renderHostile = render;
