// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The session, whichever way this build logs in (session/types.ts): the
// `#session` alias points at session/bff.ts or session/browser.ts.

export {
  SessionExpiredError,
  beginLogin,
  completeLogin,
  getAuthState,
  getGrant,
  logout,
  proxySessionUrl,
  setJmapProxyMode,
} from "#session";
export type { Grant } from "#session";
