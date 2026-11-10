// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Address book for recipient suggestions: JMAP for Contacts
// (RFC 9610) when the server has it. Stalwart does, but its text search
// matches whole words only ("car" doesn't find "Carla"), so the
// cards are loaded once (names and addresses only, capped) and matched in
// the browser.

import type { ContactCard } from "@nebulamail/jmap-client";
import { CAPABILITIES, hasCapability } from "@nebulamail/jmap-client";

import { getClient } from "./client";
import { mailKeys } from "./queries";

/** The query of the address book's names and addresses. */
export const contactsKey = [...mailKeys.all, "contacts"] as const;

export interface Contact {
  readonly name: string;
  readonly email: string;
}

/**
 * More than enough for suggestions; a huge address book isn't loaded
 * whole. Also capped by the server's maxObjectsInGet (500 in Stalwart).
 */
export const MAX_CONTACT_CARDS = 500;

type CardFields = Pick<ContactCard, "name" | "emails">;

const cardName = (card: CardFields): string => {
  const full = card.name?.full?.trim();
  if (full !== undefined && full !== "") {
    return full;
  }
  return (card.name?.components ?? [])
    .filter((part) => part.kind !== "separator")
    .map((part) => part.value.trim())
    .filter((part) => part !== "")
    .join(" ");
};

/** One entry per email address of each card, in the card's order. */
export const contactsFromCards = (cards: readonly CardFields[]): Contact[] =>
  cards.flatMap((card) => {
    const name = cardName(card);
    return Object.values(card.emails ?? {})
      .toSorted((a, b) => (a.pref ?? 100) - (b.pref ?? 100))
      .map((entry) => entry.address.trim())
      .filter((email) => email !== "")
      .map((email) => ({ email, name }));
  });

export const listContacts = async (): Promise<Contact[]> => {
  const client = await getClient();
  const { session } = client;
  if (!hasCapability(session, CAPABILITIES.contacts)) {
    return [];
  }
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const [, cards] = await client.request((b) => {
    const query = b.call("ContactCard/query", {
      accountId,
      limit: Math.min(MAX_CONTACT_CARDS, session.core.maxObjectsInGet),
    });
    const get = b.call("ContactCard/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["name", "emails"],
    });
    return [query, get] as const;
  });
  return contactsFromCards(cards.list);
};
