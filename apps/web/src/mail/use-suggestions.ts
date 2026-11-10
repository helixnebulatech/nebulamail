// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Recipient suggestions for a To/Cc/Bcc field (see suggestions.ts).

import { useQuery } from "@tanstack/react-query";

import { contactsKey, listContacts } from "./contacts";
import type { Contact } from "./contacts";
import { mailKeys } from "./queries";
import type { RecentRecipient, RecipientSuggestion } from "./suggestions";
import { recentRecipients, suggestRecipients } from "./suggestions";

const recentKey = [...mailKeys.all, "recent-recipients"] as const;

// The same empty arrays while loading: suggestions.ts indexes by identity.
const NO_CONTACTS: readonly Contact[] = [];
const NO_RECENT: readonly RecentRecipient[] = [];

export const useRecipientSuggestions = (
  query: string,
  exclude: readonly string[]
): RecipientSuggestion[] => {
  // The address book changes rarely; a few minutes old is fine.
  const contacts = useQuery({
    queryFn: listContacts,
    queryKey: contactsKey,
    staleTime: 5 * 60 * 1000,
  });
  // Read again each time a compose window opens (it's local, cheap).
  const recent = useQuery({ queryFn: recentRecipients, queryKey: recentKey });
  return suggestRecipients(query, {
    contacts: contacts.data ?? NO_CONTACTS,
    exclude,
    recent: recent.data ?? NO_RECENT,
  });
};
