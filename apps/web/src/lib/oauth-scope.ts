// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The OAuth scopes the app asks for, with the BFF (server/auth/oidc.ts) and
// in the static build (session/oauth.ts). These are the interoperable scopes
// of the OAuth Profile for Open Public Clients: since 1.0, Stalwart gives a
// token only the JMAP capabilities of its scopes, and a registered client
// may only ask for the scopes it was given (`invalid_scope` otherwise), so
// the app must ask for exactly what the operator registered.
// Servers that don't know them ignore them (RFC 6749
// §3.3), the bridge included.
//
// The operator picks the services (OAUTH_SERVICES with the BFF,
// NEBULA_OAUTH_SERVICES for the static build): all four by default. Without
// one the app hides what needs it, because the session no longer announces
// the capability: contacts (address books), calendars (calendar, tasks, invitations) or files (synced settings
// go to a mail folder instead, prefs-sync/remote.ts).
//
// No `offline_access`: Stalwart 1.0 sends a refresh token whatever the scope
// (it only adds a "stay connected" line to the consent screen), and so does
// the bridge. No `openid`: we don't use ID tokens.

export const OAUTH_SERVICES = [
  "mail",
  "contacts",
  "calendars",
  "files",
] as const;

export type OAuthService = (typeof OAUTH_SERVICES)[number];

const isService = (name: string): name is OAuthService =>
  (OAUTH_SERVICES as readonly string[]).includes(name);

/**
 * Parses a list of services ("mail contacts", commas or spaces). Empty or
 * missing means all four. `mail` is required (it is a mail app); an unknown
 * name throws, so a typo fails at start (or at build) instead of at every
 * login.
 */
export const parseOAuthServices = (
  value: string | undefined
): readonly OAuthService[] => {
  const names = (value ?? "").split(/[\s,]+/u).filter((name) => name !== "");
  if (names.length === 0) {
    return OAUTH_SERVICES;
  }
  const unknown = names.filter((name) => !isService(name));
  if (unknown.length > 0) {
    throw new Error(
      `Unknown OAuth service ${unknown.join(", ")}: use ${OAUTH_SERVICES.join(", ")}`
    );
  }
  if (!names.includes("mail")) {
    throw new Error("The OAuth services must include mail");
  }
  // Fixed order and no repeats: the same list always gives the same scope.
  return OAUTH_SERVICES.filter((service) => names.includes(service));
};

/** The `scope` parameter for these services. */
export const oauthScope = (services: readonly OAuthService[]): string =>
  services.map((service) => `urn:ietf:params:oauth:scope:${service}`).join(" ");

/** All four: what the app uses when the operator doesn't choose. */
export const OAUTH_SCOPE = oauthScope(OAUTH_SERVICES);
