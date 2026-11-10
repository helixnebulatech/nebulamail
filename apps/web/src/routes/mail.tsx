// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Outlet, createFileRoute } from "@tanstack/react-router";

import { MailShell } from "../components/mail/shell";
import { requireAuth } from "../lib/auth-guard";
import { prefetchMailboxes } from "../lib/route-prefetch";

const MailLayout = () => (
  <MailShell>
    <Outlet />
  </MailShell>
);

export const Route = createFileRoute("/mail")({
  // Signed in? Once per page load, shared by every section (auth-guard.ts).
  beforeLoad: async ({ location }) => await requireAuth(location),
  // The shell's folders load with the route, not after it mounts.
  loader: ({ context: { queryClient } }) => {
    prefetchMailboxes(queryClient);
  },
  component: MailLayout,
});
