// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../server/auth/config";
import { handleUnsubscribe } from "../../server/unsubscribe";
import { unsubscribeDeps } from "../../server/unsubscribe-config";

export const Route = createFileRoute("/api/unsubscribe")({
  server: {
    handlers: {
      POST: async ({ request }) =>
        await handleUnsubscribe(request, await authConfig(), unsubscribeDeps()),
    },
  },
});
