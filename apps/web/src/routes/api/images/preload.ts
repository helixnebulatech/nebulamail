// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../../server/auth/config";
import { preloadPrefsConfig } from "../../../server/images/config";
import { handlePreloadPreference } from "../../../server/images/preload";

export const Route = createFileRoute("/api/images/preload")({
  server: {
    handlers: {
      POST: async ({ request }) =>
        await handlePreloadPreference(
          request,
          await authConfig(),
          await preloadPrefsConfig(),
          { fetch: globalThis.fetch.bind(globalThis) }
        ),
    },
  },
});
