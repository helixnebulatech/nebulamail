// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// URL templates (RFC 6570 level 1) and the origin guard.
//
// Why the guard exists: the session document tells the client where to send
// API calls, downloads, uploads and push. If that document is malicious or
// compromised (or a proxy rewrote it), following `apiUrl` to another origin
// would hand our Bearer token to a third party. So every server-provided URL
// must stay on an allowed origin (by default only the session URL's origin).

import { JmapError } from "./errors";

/** Origins allowed for server-provided URLs (normalised `URL.origin`s). */
export interface OriginPolicy {
  readonly allowAny: boolean;
  readonly origins: readonly string[];
  /**
   * Other names of the session URL's own server: URLs on them are moved to
   * the session URL's origin (see `ConnectOptions.serverAliases`).
   */
  readonly aliases: readonly string[];
  /** The session URL's origin, where aliased URLs are moved to. */
  readonly home: string;
}

export const makeOriginPolicy = (
  sessionUrl: URL,
  allowCrossOriginUrls: boolean | readonly string[] | undefined,
  serverAliases: readonly string[] = []
): OriginPolicy => {
  const aliases = serverAliases
    .map((origin) => new URL(origin).origin)
    .filter((origin) => origin !== sessionUrl.origin);
  const home = sessionUrl.origin;
  if (allowCrossOriginUrls === true) {
    return { aliases, allowAny: true, home, origins: [home] };
  }
  const extra =
    allowCrossOriginUrls === undefined || allowCrossOriginUrls === false
      ? []
      : allowCrossOriginUrls.map((origin) => new URL(origin).origin);
  return { aliases, allowAny: false, home, origins: [home, ...extra] };
};

/** The same URL on the session's origin, if it is on one of its aliases. */
const unalias = (url: URL, policy: OriginPolicy): URL => {
  if (!policy.aliases.includes(url.origin)) {
    return url;
  }
  const moved = new URL(policy.home);
  moved.pathname = url.pathname;
  moved.search = url.search;
  moved.hash = url.hash;
  return moved;
};

const isHttp = (url: URL): boolean =>
  url.protocol === "https:" || url.protocol === "http:";

/**
 * Resolves `raw` (absolute or relative to `base`) and checks scheme, embedded
 * credentials and origin. Throws `invalidUrl` / `crossOrigin`.
 */
export const checkUrl = (raw: string, base: URL, policy: OriginPolicy): URL => {
  let url: URL | undefined = undefined;
  try {
    url = new URL(raw, base);
  } catch (error) {
    throw new JmapError({ kind: "invalidUrl", url: raw }, { cause: error });
  }
  // `user:pass@` in a server-provided URL is never legitimate here and would
  // make fetch send Basic credentials alongside our Bearer token.
  if (!isHttp(url) || url.username !== "" || url.password !== "") {
    throw new JmapError({ kind: "invalidUrl", url: raw });
  }
  url = unalias(url, policy);
  if (!policy.allowAny && !policy.origins.includes(url.origin)) {
    throw new JmapError({
      allowedOrigins: policy.origins,
      kind: "crossOrigin",
      url: url.href,
    });
  }
  return url;
};

// Level 1 only allows `{name}`; anything else (operators, modifiers, nested
// braces) is rejected so a template can't smuggle unexpected syntax.
const EXPRESSION = /\{(?<name>[^{}]*)\}/gu;
const VARNAME = /^[A-Za-z]\w*$/u;
const NEEDS_ESCAPE = /[!'()*]/gu;

/**
 * RFC 6570 "simple string expansion": everything outside the unreserved set
 * (ALPHA / DIGIT / "-" / "." / "_" / "~") is percent-encoded as UTF-8.
 * `encodeURIComponent` leaves `!'()*` alone, so those are encoded by hand.
 * Lone surrogates are replaced (U+FFFD) instead of throwing.
 */
export const encodeTemplateValue = (value: string): string =>
  encodeURIComponent(value.toWellFormed()).replaceAll(
    NEEDS_ESCAPE,
    (char) => `%${char.codePointAt(0)?.toString(16).toUpperCase() ?? ""}`
  );

/** Variable names used by a level-1 template; `undefined` if malformed. */
export const templateVariables = (template: string): string[] | undefined => {
  const names: string[] = [];
  for (const match of template.matchAll(EXPRESSION)) {
    const name = match.groups?.name ?? "";
    if (!VARNAME.test(name)) {
      return undefined;
    }
    names.push(name);
  }
  // Stray braces left after removing valid expressions mean bad syntax.
  if (/[{}]/u.test(template.replaceAll(EXPRESSION, ""))) {
    return undefined;
  }
  return names;
};

/** Expands a level-1 template. Undefined variables expand to "". */
export const expandTemplate = (
  template: string,
  values: Readonly<Record<string, string>>
): string =>
  template.replaceAll(EXPRESSION, (_whole, name: string) => {
    const value = Object.hasOwn(values, name) ? values[name] : undefined;
    return value === undefined ? "" : encodeTemplateValue(value);
  });

/**
 * Validates a server URL template at session time: well-formed, includes the
 * required variables, and (with dummy values) resolves to an allowed origin.
 */
export const checkTemplate = (
  template: string,
  required: readonly string[],
  base: URL,
  policy: OriginPolicy
): string => {
  const names = templateVariables(template);
  if (names === undefined || required.some((name) => !names.includes(name))) {
    throw new JmapError({ kind: "invalidUrl", url: template });
  }
  const sample = Object.fromEntries(names.map((name) => [name, "x"]));
  checkUrl(expandTemplate(template, sample), base, policy);
  // An absolute template on an alias: the same template on the home origin
  // (the origin is the part before the first `/` after `//`, no variables).
  const alias = policy.aliases.find((origin) =>
    template.startsWith(`${origin}/`)
  );
  return alias === undefined
    ? template
    : `${policy.home}${template.slice(alias.length)}`;
};

/** RFC 6901 escaping for one JSON Pointer / PatchObject path segment. */
export const escapePointerSegment = (segment: string): string =>
  segment.replaceAll("~", "~0").replaceAll("/", "~1");
