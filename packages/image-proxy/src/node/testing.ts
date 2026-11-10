// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TEST-ONLY factory. Not in package.json `exports`, and refused outside
// Vitest. Tests pass a guarded fetch built with net-guard's own test hook
// (`createGuardedFetchForTests`), so the guard still validates every URL and
// DNS answer; only the final TCP connect of an already validated public
// address is redirected to a local server.

import type { GuardedFetch } from "@nebulamail/net-guard/node";

import { buildNodeProxy } from "./proxy";
import type { NodeProxy, NodeProxyOptions } from "./proxy";

export const createNodeProxyForTests = (
  options: NodeProxyOptions & { readonly guardedFetch: GuardedFetch }
): NodeProxy => {
  if (process.env.VITEST !== "true") {
    throw new Error("image proxy: the test factory is only available in tests");
  }
  return buildNodeProxy(options, options.guardedFetch);
};
