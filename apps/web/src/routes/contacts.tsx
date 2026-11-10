// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Outlet, createFileRoute } from "@tanstack/react-router";

import { ContactsPage } from "../components/contacts/contacts-page";
import {
  ContactsBar,
  ContactsFab,
  contactsSidebar,
} from "../components/contacts/contacts-shell";
import { ContactsUiProvider } from "../components/contacts/contacts-ui";
import { MailShell } from "../components/mail/shell";
import { requireAuth } from "../lib/auth-guard";
import { prefetchContacts, prefetchMailboxes } from "../lib/route-prefetch";
import { m } from "../paraglide/messages.js";

// Contacts' own sidebar, top bar and floating button in the shared shell
// (contacts-shell.tsx).
const ContactsLayout = () => (
  <ContactsUiProvider>
    <MailShell
      bar={<ContactsBar />}
      drawerLabel={m.contacts_nav()}
      fab={<ContactsFab />}
      sidebar={contactsSidebar}
    >
      <ContactsPage>
        <Outlet />
      </ContactsPage>
    </MailShell>
  </ContactsUiProvider>
);

export const Route = createFileRoute("/contacts")({
  // Signed in? Once per page load, shared by every section (auth-guard.ts).
  beforeLoad: async ({ location }) => await requireAuth(location),
  // The shell's folders and the contacts load with the route, not after
  // it mounts.
  loader: ({ context: { queryClient } }) => {
    prefetchMailboxes(queryClient);
    prefetchContacts(queryClient);
  },
  component: ContactsLayout,
});
