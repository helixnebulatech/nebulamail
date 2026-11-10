// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Production dependencies of POST /api/unsubscribe: net-guard with
// HTTPS_POLICY (https on 443 only, RFC 8058 §3) and tighter limits, and an
// in-memory rate limit per user.

import { HTTPS_POLICY } from "@nebulamail/net-guard";
import { createGuardedFetch } from "@nebulamail/net-guard/node";

import type { UnsubscribeDeps } from "./unsubscribe";
import { createRateLimiter } from "./unsubscribe";

let deps: UnsubscribeDeps | undefined;

export const unsubscribeDeps = (): UnsubscribeDeps => {
  deps ??= {
    fetch: globalThis.fetch.bind(globalThis),
    guardedFetch: createGuardedFetch({
      limits: {
        maxBodyBytes: 64 * 1024,
        maxRedirects: 3,
        totalTimeoutMs: 8000,
      },
      policy: HTTPS_POLICY,
    }),
    // 20 per user every 10 minutes: plenty for a person, little for abuse.
    rateLimit: createRateLimiter(20, 10 * 60_000),
  };
  return deps;
};
