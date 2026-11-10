// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Programados». A static route: it wins over
// `/mail/$mailbox`, so no folder id can take its place.

import { createFileRoute } from "@tanstack/react-router";

import { ScheduledPage } from "../components/mail/scheduled-page";

export const Route = createFileRoute("/mail/scheduled")({
  component: ScheduledPage,
});
