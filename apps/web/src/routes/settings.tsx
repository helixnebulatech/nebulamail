// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Outlet, createFileRoute } from "@tanstack/react-router";

import { MailShell } from "../components/mail/shell";
import { SettingsPage } from "../components/settings/settings-page";
import {
  SettingsBar,
  settingsSidebar,
} from "../components/settings/settings-shell";
import { requireAuth } from "../lib/auth-guard";
import { prefetchMailboxes } from "../lib/route-prefetch";
import { m } from "../paraglide/messages.js";

// Settings' own sidebar (its sections) and top bar (settings-shell.tsx).
const SettingsLayout = () => (
  <MailShell
    bar={<SettingsBar />}
    drawerLabel={m.settings_sections()}
    fab={null}
    sidebar={settingsSidebar}
  >
    <SettingsPage>
      <Outlet />
    </SettingsPage>
  </MailShell>
);

export const Route = createFileRoute("/settings")({
  // Signed in? Once per page load, shared by every section (auth-guard.ts).
  beforeLoad: async ({ location }) => await requireAuth(location),
  // The shell's folders load with the route, not after it mounts.
  loader: ({ context: { queryClient } }) => {
    prefetchMailboxes(queryClient);
  },
  component: SettingsLayout,
});
