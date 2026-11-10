// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TEST-ONLY factory. Not in package.json `exports`, and refused outside
// Vitest. Tests pass a fake network `fetch` that plays the DoH server and the
// image servers; the guard still validates every URL and DoH answer.

import type { WebFetch } from "@nebulamail/net-guard/workers";

import { buildWorkersProxy } from "./proxy";
import type { WorkersProxy, WorkersProxyOptions } from "./proxy";

export const createWorkersProxyForTests = (
  options: WorkersProxyOptions & { readonly net: WebFetch }
): WorkersProxy => {
  if (process.env.VITEST !== "true") {
    throw new Error("image proxy: the test factory is only available in tests");
  }
  return buildWorkersProxy(options, options.net);
};
