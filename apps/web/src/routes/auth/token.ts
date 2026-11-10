// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../server/auth/config";
import { handleToken } from "../../server/auth/handlers";

export const Route = createFileRoute("/auth/token")({
  server: {
    handlers: {
      POST: async ({ request }) =>
        await handleToken(request, await authConfig()),
    },
  },
});
