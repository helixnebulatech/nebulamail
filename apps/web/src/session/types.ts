// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The session layer. Two implementations, chosen at
// build time through the `#session` alias (vite.config.ts):
//   - bff.ts: our server keeps the refresh token in an HttpOnly cookie;
//   - browser.ts: static build served by Stalwart, OAuth in the browser,
//     the refresh token only in a worker's memory.
// The rest of the app only uses lib/auth.ts, which re-exports one of them.

import { z } from "zod";

import { AdminImagePolicySchema } from "../mail/image-policy";

/** A short access token: the browser talks JMAP with it itself. */
export const TokenGrantSchema = z.object({
  /** Never set here: see ProxyGrantSchema. */
  jmapProxy: z.undefined().optional(),
  accessToken: z.string(),
  expiresAt: z.number(),
  sessionUrl: z.url(),
  /** Forced by the admin (true/false) or null: each user decides. */
  verifiedSenderBadge: z.boolean().nullable(),
  /**
   * The admin's remote images settings (mail/image-policy.ts). Only from
   * our server: the static build has its own choice (mail/remote-images.ts).
   */
  imagePolicy: AdminImagePolicySchema.optional(),
  /**
   * Other origins of the JMAP server (static build: its public name, while
   * the app talks to it on its own origin; see JmapClient `serverAliases`).
   */
  serverAliases: z.array(z.url()).optional(),
});

/**
 * Full BFF mode (JMAP_PROXY=on): no token. `sessionUrl` is the
 * proxy on the page's own origin, and the session cookie authenticates.
 */
export const ProxyGrantSchema = z.object({
  jmapProxy: z.literal(true),
  /** When to ask again whether the session is still there. */
  expiresAt: z.number(),
  sessionUrl: z.url(),
  /**
   * The JMAP server's public host name (JMAP_SERVER). The session the page
   * sees points at the proxy on this origin, but Stalwart writes its own
   * name in Authentication-Results (mail/auth-results.ts).
   */
  serverHostname: z.string().min(1).max(253).optional(),
  verifiedSenderBadge: z.boolean().nullable(),
  imagePolicy: AdminImagePolicySchema.optional(),
});

export const GrantSchema = z.union([ProxyGrantSchema, TokenGrantSchema]);

export type Grant = z.infer<typeof GrantSchema>;

export class SessionExpiredError extends Error {
  override readonly name = "SessionExpiredError";
}

/** Why a login failed (the login page shows one message per kind). */
export type LoginError = "denied" | "expired" | "invalid" | "unavailable";

export type LoginCompletion =
  /** Not a login callback: show the login page. */
  | { readonly kind: "none" }
  | { readonly kind: "done"; readonly returnTo: string }
  | { readonly kind: "failed"; readonly error: LoginError };

export interface SessionApi {
  readonly getAuthState: () => Promise<{ readonly authenticated: boolean }>;
  readonly getGrant: (options?: {
    readonly forceRefresh?: boolean;
  }) => Promise<Grant>;
  readonly logout: () => Promise<void>;
  /** Starts the login: leaves the page for the server's login form. */
  readonly beginLogin: (returnTo: string | undefined) => Promise<void>;
  /** On the login page: finishes a login if the URL is a callback. */
  readonly completeLogin: (search: string) => Promise<LoginCompletion>;
}

/** Margin before expiry at which a new access token is asked for. */
export const GRANT_MARGIN_MS = 60_000;
