// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { authConfig } from "../../../server/auth/config";
import { imageTicketConfig, ticketDeps } from "../../../server/images/config";
import { handleImageTickets } from "../../../server/images/tickets";

export const Route = createFileRoute("/api/images/tickets")({
  server: {
    handlers: {
      POST: async ({ request }) =>
        await handleImageTickets(
          request,
          await authConfig(),
          await imageTicketConfig(),
          ticketDeps()
        ),
    },
  },
});
