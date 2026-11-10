// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Outlet, createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { ListPanel } from "../components/mail/list-panel";
import { ListProvider } from "../components/mail/list-state";
import { prefetchMailboxes } from "../lib/route-prefetch";
import { parseMailboxParam } from "../mail/account-scope";
import { READ_FILTERS } from "../mail/views";

// `q`: search (operators in search.ts); `split`: split-inbox tab;
// `filter`: read filter. Bad values fall back to the defaults.
const Search = z.object({
  filter: z.enum(READ_FILTERS).optional().catch(undefined),
  q: z.string().max(500).optional().catch(undefined),
  split: z.string().max(64).optional().catch(undefined),
});

const MailboxPage = () => (
  <ListProvider>
    <ListPanel />
    <Outlet />
  </ListProvider>
);

export const Route = createFileRoute("/mail/$mailbox")({
  component: MailboxPage,
  // The folders of the account in the URL (a shared one's too). Not the
  // list itself: its query depends on the resolved folder, split, filter
  // and search (list-state.tsx); the folders are what it waits for.
  loader: ({ context: { queryClient }, params }) => {
    prefetchMailboxes(queryClient, parseMailboxParam(params.mailbox).accountId);
  },
  validateSearch: Search,
});
