// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TEST-ONLY factory. Not in package.json `exports`, and refused outside
// Vitest. Tests pass a guarded fetch built with net-guard's own test hook
// (`createGuardedFetchForTests`), so the guard still validates every URL and
// DNS answer; only the TCP connect of a validated address is redirected.

import type { GuardedFetch } from "@nebulamail/net-guard/node";

import { buildVercelProxy } from "./proxy";
import type { VercelProxy, VercelProxyOptions } from "./proxy";

export const createVercelProxyForTests = (
  options: VercelProxyOptions & { readonly guardedFetch: GuardedFetch }
): VercelProxy => {
  if (process.env.VITEST !== "true") {
    throw new Error("image proxy: the test factory is only available in tests");
  }
  return buildVercelProxy(options, options.guardedFetch);
};
