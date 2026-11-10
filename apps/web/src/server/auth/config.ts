// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Builds the AuthConfig from the environment once per server process.

import { ENV } from "../../env.server";
import { oauthScope, parseOAuthServices } from "../../lib/oauth-scope";
import { adminImagePolicy } from "../images/admin";
import {
  makeUpstream,
  parseInternalUrl,
  parseJmapProxy,
  upstreamFetch,
} from "../jmap-proxy/config";
import type { AuthConfig } from "./handlers";
import { DEFAULT_LIMITS } from "./handlers";
import type { ProviderMetadata } from "./oidc";
import { discover } from "./oidc";
import { parseSealKeys } from "./seal";

const METADATA_TTL_MS = 60 * 60 * 1000;

const withoutTrailingSlash = (url: string): string => url.replace(/\/+$/u, "");

const build = async (): Promise<AuthConfig> => {
  const appUrl = new URL(withoutTrailingSlash(ENV.APP_URL));
  const issuer = new URL(withoutTrailingSlash(ENV.JMAP_SERVER));
  // OIDC Discovery §4.3 / RFC 8414 §3.3: compared as written, no trimming.
  const issuerId = ENV.JMAP_SERVER;
  // JMAP_INTERNAL_URL: this server's own calls (OIDC included) go there;
  // the browser still goes to the public issuer for the login page.
  const upstream = makeUpstream(
    issuer,
    parseInternalUrl(ENV.JMAP_INTERNAL_URL)
  );
  const oidc = {
    clientId: ENV.OAUTH_CLIENT_ID,
    clientSecret: ENV.OAUTH_CLIENT_SECRET,
    fetch: upstreamFetch(globalThis.fetch.bind(globalThis), upstream),
    issuer,
    issuerId,
    now: Date.now,
    redirectUri: new URL("/auth/callback", appUrl),
    // Exactly the scopes registered for this client (lib/oauth-scope.ts).
    scope: oauthScope(parseOAuthServices(ENV.OAUTH_SERVICES)),
  };

  let cached:
    | { readonly metadata: ProviderMetadata; readonly at: number }
    | undefined;
  const metadata = async (): Promise<ProviderMetadata> => {
    if (cached === undefined || Date.now() - cached.at > METADATA_TTL_MS) {
      cached = { at: Date.now(), metadata: await discover(oidc) };
    }
    return cached.metadata;
  };

  return {
    appUrl,
    imagePolicy: adminImagePolicy(),
    jmapProxy: parseJmapProxy(ENV.JMAP_PROXY),
    jmapSessionUrl: new URL("/.well-known/jmap", issuer),
    keys: await parseSealKeys(ENV.SESSION_KEYS),
    limits: DEFAULT_LIMITS,
    metadata,
    oidc,
    upstream,
    verifiedSenderBadge: ENV.VERIFIED_SENDER_BADGE ?? null,
  };
};

let config: Promise<AuthConfig> | undefined;

export const authConfig = async (): Promise<AuthConfig> => {
  config ??= build();
  return await config;
};

/**
 * The JMAP_PROXY and JMAP_INTERNAL_URL values parse, or this throws: called
 * when the server starts (server.ts), so a typo stops it instead of
 * silently running without the proxy.
 */
export const checkJmapProxyConfig = (): void => {
  parseJmapProxy(ENV.JMAP_PROXY);
  parseInternalUrl(ENV.JMAP_INTERNAL_URL);
};
