// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Trusted Types. With `require-trusted-types-for
// 'script'` the browser refuses plain strings in HTML sinks (`innerHTML`,
// `srcdoc`, `DOMParser.parseFromString`…). The policies the app uses:
//
// - `default`: for code we don't control (TipTap loads its content with
//   `parseFromString`). Every string goes through DOMPurify. Scripts and
//   script URLs from strings are refused, except same-origin script URLs
//   (the service worker).
// - `nebula-mail-parse`: parsing into an inert document (DOMParser: no
//   scripts run and nothing loads), for the email pipeline, which has its
//   own sanitizer and must see the HTML as it is. Only `parseInertHtml`
//   uses it, and it only returns the parsed Document.
// - `nebula-mail-srcdoc`: an email iframe's `srcdoc`. Only the HTML of a
//   `RenderedFrame` (made by `renderEmail` after its final check) is
//   accepted, at the moment `trustedSrcdoc` passes it. It lives in
//   rendered-frame.ts, next to the class whose private field it checks.
// - `dompurify` (DOMPurify's own, one per instance) and
//   `ProseMirrorClipboard` (ProseMirror reads pasted HTML into a separate
//   document) are created by those libraries.

// Only `dompurify` at run time: vite.config.ts loads this module with
// Node (through the CSP of the static build), which doesn't resolve
// relative imports without an extension. Keep it free of local imports;
// test/trusted-types-node.test.ts checks it.
import DOMPurify from "dompurify";

/** The `trusted-types` CSP directive value (policy names). */
export const TRUSTED_TYPES_POLICIES = [
  "default",
  "nebula-mail-parse",
  "nebula-mail-srcdoc",
  "dompurify",
  "ProseMirrorClipboard",
  // DOMPurify and ProseMirror may create their policy more than once.
  "'allow-duplicates'",
] as const;

/** The opaque value the browser accepts in an HTML sink. */
export type TrustedHtml = string;

export interface Policy {
  /** Really a TrustedHTML object, which every HTML sink accepts. */
  readonly createHTML: (input: string) => TrustedHtml;
}

export interface PolicyOptions {
  readonly createHTML?: (input: string) => string | null;
  readonly createScript?: (input: string) => string | null;
  readonly createScriptURL?: (input: string) => string | null;
}

interface Factory {
  readonly createPolicy: (name: string, options: PolicyOptions) => Policy;
}

const isFactory = (value: unknown): value is Factory =>
  typeof value === "object" &&
  value !== null &&
  "createPolicy" in value &&
  typeof value.createPolicy === "function";

const factory = (): Factory | null => {
  const value: unknown = Reflect.get(globalThis, "trustedTypes");
  return isFactory(value) ? value : null;
};

/** A Trusted Types policy, or null without them (or if the CSP refuses it). */
export const createPolicy = (
  name: string,
  options: PolicyOptions
): Policy | null => {
  try {
    return factory()?.createPolicy(name, options) ?? null;
  } catch {
    // Not allowed by the page's CSP: the sinks then refuse plain strings
    // and the problem shows up as a CSP error, never as unsafe HTML.
    return null;
  }
};

let parsePolicy: Policy | null | undefined;

/**
 * `html` as the value of an HTML sink: without Trusted Types the same
 * string, with them the policy's TrustedHTML.
 */
export const throughPolicy = (
  policy: Policy | null,
  html: string
): TrustedHtml => (policy === null ? html : policy.createHTML(html));

/** Parses HTML into an inert document (no scripts, no loading). */
export const parseInertHtml = (
  html: string,
  parser: Pick<DOMParser, "parseFromString"> = new DOMParser()
): Document => {
  parsePolicy ??= createPolicy("nebula-mail-parse", {
    createHTML: (input) => input,
  });
  return parser.parseFromString(throughPolicy(parsePolicy, html), "text/html");
};

/**
 * Same origin and, if `paths` is given, one of them: an exact path, or a
 * prefix ending in "/". No query or fragment then.
 */
const allowedScriptUrl = (
  input: string,
  paths: readonly string[] | undefined
): boolean => {
  let url: URL;
  try {
    url = new URL(input, globalThis.location.href);
  } catch {
    return false;
  }
  if (url.origin !== globalThis.location.origin) {
    return false;
  }
  if (paths === undefined) {
    return true;
  }
  return (
    url.search === "" &&
    url.hash === "" &&
    paths.some((path) =>
      path.endsWith("/") ? url.pathname.startsWith(path) : url.pathname === path
    )
  );
};

export interface TrustedTypesOptions {
  /**
   * Where scripts and workers may come from, when the same origin also
   * serves files that aren't ours (the static build: Stalwart serves
   * attachments there). Exact paths, or prefixes ending in "/".
   */
  readonly scriptUrlPaths?: readonly string[];
}

let installed = false;

/**
 * Creates the `default` policy (once, in the browser, before the editor
 * or anything else uses an HTML sink). Without Trusted Types it does
 * nothing.
 */
export const installTrustedTypes = (
  options: TrustedTypesOptions = {}
): void => {
  if (installed || factory() === null) {
    return;
  }
  installed = true;
  // Its own instance: Proton's sanitizer configures the default one.
  const purify = DOMPurify(globalThis.window);
  createPolicy("default", {
    // RETURN_TRUSTED_TYPE false: a string, which the browser accepts
    // from the default policy. DOMPurify parses with its own policy.
    createHTML: (input) =>
      purify.sanitize(input, { RETURN_TRUSTED_TYPE: false }),
    createScript: () => null,
    createScriptURL: (input) =>
      allowedScriptUrl(input, options.scriptUrlPaths) ? input : null,
  });
};
