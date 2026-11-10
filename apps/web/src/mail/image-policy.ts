// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Remote images in the build with our server:
// two user choices, synced, within what the admin allows.
//   - When: "never" / "ask" / "trusted-senders" / "always" (default ask).
//   - How: through the image proxy, or straight from the sender (reveals
//     the IP address and can reach the user's network; with a proxy,
//     only if the admin allows it). Default: the proxy.
// The admin (server/images/policy.ts) sets the defaults, can forbid
// "direct" and can force "never"; without an image proxy "proxy" isn't
// offered. Pure functions: the reader (components/mail/reader.tsx) and
// Settings use them through use-image-policy.ts.
//
// Rules that keep this safe:
//   - "never" (or no way to load) never loads anything: no button, no
//     tickets, no request to the proxy or the sender.
//   - Loading straight from the sender without a click needs that route
//     chosen on purpose: by the user, or by the admin's explicit default.
//     A route that is direct only because there is no proxy asks first,
//     with the IP warning.
//   - A trusted sender (isTrustedSender) needs a sender check that didn't
//     fail and an email outside Junk: a forged From isn't trusted.

import { z } from "zod";

import type { AuthVerdict } from "./auth-results";
import { sameAddress } from "./recipients";

export const IMAGE_LOAD_MODES = [
  "never",
  "ask",
  "trusted-senders",
  "always",
] as const;
export type ImageLoadMode = (typeof IMAGE_LOAD_MODES)[number];
export const ImageLoadModeSchema = z.enum(IMAGE_LOAD_MODES);

export const IMAGE_ROUTES = ["proxy", "direct"] as const;
export type ImageRoute = (typeof IMAGE_ROUTES)[number];
export const ImageRouteSchema = z.enum(IMAGE_ROUTES);

/** What the server tells the browser (in the grant, server/auth). */
export const AdminImagePolicySchema = z.object({
  /** The users' default for "when". */
  defaultLoad: ImageLoadModeSchema,
  /** The admin's explicit default route; null: the proxy if there is one. */
  defaultRoute: ImageRouteSchema.nullable(),
  /** Users may load straight from the sender. */
  direct: z.boolean(),
  /** The admin forced "never" for everyone. */
  forceNever: z.boolean(),
  /** An image proxy is configured (IMAGE_PROXY_ORIGIN and _KEYS). */
  proxy: z.boolean(),
});
export type AdminImagePolicy = z.infer<typeof AdminImagePolicySchema>;

/** What the server's defaults are without any variable set. */
export const DEFAULT_ADMIN_IMAGE_POLICY: AdminImagePolicy = {
  defaultLoad: "ask",
  defaultRoute: null,
  direct: false,
  forceNever: false,
  proxy: true,
};

export interface UserImageChoice {
  /** Undefined: never chosen (the admin's default applies). */
  readonly load?: ImageLoadMode | undefined;
  readonly route?: ImageRoute | undefined;
}

export interface ImagePolicy {
  /** The "when" in effect. */
  readonly load: ImageLoadMode;
  /** The "how" in effect; null: there is no way (no proxy, no direct). */
  readonly route: ImageRoute | null;
  /**
   * The route was chosen on purpose (by the user, or the admin's explicit
   * default): only then may "direct" load without a click.
   */
  readonly routeChosen: boolean;
  /** What Settings offers. */
  readonly loadModes: readonly ImageLoadMode[];
  readonly routes: readonly ImageRoute[];
  /** The admin forced "never". */
  readonly forced: boolean;
}

export const resolveImagePolicy = (
  admin: AdminImagePolicy,
  user: UserImageChoice
): ImagePolicy => {
  const routes: ImageRoute[] = [];
  if (admin.proxy) {
    routes.push("proxy");
  }
  if (admin.direct) {
    routes.push("direct");
  }
  if (admin.forceNever || routes.length === 0) {
    return {
      forced: admin.forceNever,
      load: "never",
      loadModes: ["never"],
      route: null,
      routeChosen: false,
      routes: admin.forceNever ? [] : routes,
    };
  }
  const userRoute =
    user.route !== undefined && routes.includes(user.route)
      ? user.route
      : undefined;
  const adminRoute =
    admin.defaultRoute !== null && routes.includes(admin.defaultRoute)
      ? admin.defaultRoute
      : undefined;
  const route = userRoute ?? adminRoute ?? routes[0] ?? null;
  return {
    forced: false,
    load: user.load ?? admin.defaultLoad,
    loadModes: IMAGE_LOAD_MODES,
    route,
    routeChosen: userRoute !== undefined || adminRoute !== undefined,
    routes,
  };
};

/**
 * For one email: load at once ("auto"), offer the button ("ask"), or
 * nothing at all ("never").
 */
export type ImageDecision = "auto" | "ask" | "never";

export const decideRemoteImages = (
  policy: ImagePolicy,
  context: { readonly senderTrusted: boolean }
): ImageDecision => {
  if (policy.load === "never" || policy.route === null) {
    return "never";
  }
  const automatic =
    policy.load === "always" ||
    (policy.load === "trusted-senders" && context.senderTrusted);
  if (!automatic) {
    return "ask";
  }
  // Direct only because there is no proxy: ask first, with the warning.
  return policy.route === "direct" && !policy.routeChosen ? "ask" : "auto";
};

/** At most this many addresses in the synced list (prefs-sync/model.ts). */
export const MAX_TRUSTED_SENDERS = 1000;

/** The list with `address` added (lower case, once) or removed. */
export const withTrustedSender = (
  list: readonly string[],
  address: string,
  on: boolean
): string[] => {
  const normalized = address.trim().toLowerCase();
  const rest = list.filter((entry) => !sameAddress(entry, normalized));
  return on ? [...rest, normalized].slice(-MAX_TRUSTED_SENDERS) : rest;
};

export interface SenderFacts {
  /** The email's From addresses. */
  readonly from: readonly string[];
  /** The sender check (auth-results.ts); "fail" is never trusted. */
  readonly verdict: AuthVerdict | null;
  /** Folders known and the email isn't in Junk. */
  readonly outsideJunk: boolean;
}

/**
 * A trusted sender: one From address, in the user's list ("Always show
 * from this sender") or in their contacts, with a sender check that
 * didn't fail and outside Junk (anyone can write any From).
 */
export const isTrustedSender = (
  facts: SenderFacts,
  trusted: {
    readonly list: readonly string[];
    readonly contacts: readonly string[];
  }
): boolean => {
  const [address] = facts.from;
  if (
    facts.from.length !== 1 ||
    address === undefined ||
    facts.verdict === "fail" ||
    !facts.outsideJunk
  ) {
    return false;
  }
  return [...trusted.list, ...trusted.contacts].some((entry) =>
    sameAddress(entry, address)
  );
};
