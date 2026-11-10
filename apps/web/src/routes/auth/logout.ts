// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../server/auth/config";
import { handleLogout } from "../../server/auth/handlers";

export const Route = createFileRoute("/auth/logout")({
  server: {
    handlers: {
      POST: async ({ request }) =>
        await handleLogout(request, await authConfig()),
    },
  },
});
