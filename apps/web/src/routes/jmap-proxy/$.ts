// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The JMAP proxy of the full BFF mode (server/jmap-proxy/proxy.ts). With
// JMAP_PROXY off every path here answers 404.

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../server/auth/config";
import { handleJmapProxy } from "../../server/jmap-proxy/proxy";
import type { ProxyDeps } from "../../server/jmap-proxy/proxy";

const deps: ProxyDeps = {
  fetch: globalThis.fetch.bind(globalThis),
  now: Date.now,
};

const handle = async ({
  request,
}: {
  readonly request: Request;
}): Promise<Response> =>
  await handleJmapProxy(request, await authConfig(), deps);

export const Route = createFileRoute("/jmap-proxy/$")({
  server: {
    handlers: {
      GET: handle,
      POST: handle,
    },
  },
});
