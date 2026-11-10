// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TanStack Start instance: global request middleware.

import {
  createCsrfMiddleware,
  createMiddleware,
  createStart,
} from "@tanstack/react-start";

import { ENV } from "./env";
import {
  FRAME_PAGE_PATH,
  commonSecurityHeaders,
} from "./server/security-headers";

/** Adds the security headers to every response, pages and /auth/* alike. */
const securityHeaders = createMiddleware().server(
  async ({ next: handle, request }) => {
    const result = await handle();
    // The static build has no server: Stalwart sends its headers (only the
    // build's prerender of index.html runs this).
    if (import.meta.env.NEBULA_STATIC) {
      return result;
    }
    const headers = commonSecurityHeaders({
      framePage: new URL(request.url).pathname === FRAME_PAGE_PATH,
      https: new URL(ENV.APP_URL).protocol === "https:",
    });
    for (const [name, value] of Object.entries(headers)) {
      result.response.headers.set(name, value);
    }
    return result;
  }
);

/**
 * Server functions are HTTP endpoints too: only same-origin calls
 * (Sec-Fetch-Site / Origin). Ours only read (session state, preferences,
 * config), but this keeps it true for any added later. The /auth/* and
 * /api/* routes check the origin themselves (server/auth/handlers.ts).
 */
const csrf = createCsrfMiddleware({
  filter: (ctx) => ctx.handlerType === "serverFn",
});

export const startInstance = createStart(() => ({
  requestMiddleware: [securityHeaders, csrf],
}));
