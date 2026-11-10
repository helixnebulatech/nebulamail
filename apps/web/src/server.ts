// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Server entry: runs every request inside Paraglide's middleware so
// server-rendered messages use the visitor's language.

import handler from "@tanstack/react-start/server-entry";

import { paraglideMiddleware } from "./paraglide/server.js";
import { checkJmapProxyConfig } from "./server/auth/config";
import { adminImagePolicy } from "./server/images/admin";

// The admin's remote images variables make sense together,
// or the server doesn't start. The static build has no server: only its
// prerender runs this file.
if (!import.meta.env.NEBULA_STATIC) {
  adminImagePolicy();
  // JMAP_PROXY and JMAP_INTERNAL_URL too (server/jmap-proxy/config.ts).
  checkJmapProxyConfig();
}

export default {
  fetch: async (request: Request): Promise<Response> =>
    await paraglideMiddleware(
      request,
      async () => await handler.fetch(request)
    ),
};
