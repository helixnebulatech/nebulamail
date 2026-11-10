// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Recipient suggestions: the address book plus the people
// recently written to, remembered on this device only (localStorage, per
// account, capped). Ranking: names or addresses that start with what was
// typed first, then the most used and most recent.

import { z } from "zod";

import { accountKey } from "./account-key";
import type { Recipient } from "./compose";
import type { Contact } from "./contacts";

export interface RecentRecipient {
  readonly email: string;
  readonly name?: string | undefined;
  /** Messages sent to this address. */
  readonly count: number;
  /** When the last one was sent (ms since epoch). */
  readonly lastUsed: number;
}

/** How many recent recipients are kept per account. */
export const MAX_RECENT = 200;

const RecentSchema = z.array(
  z.object({
    count: z.number().int().positive(),
    email: z.string().min(3).max(320),
    lastUsed: z.number().nonnegative(),
    name: z.string().max(200).optional(),
  })
);

const key = (accountId: string): string => `nm-recent-recipients:${accountId}`;

const sameEmail = (a: string, b: string): boolean =>
  a.trim().toLowerCase() === b.trim().toLowerCase();

/** Adds a sent message's recipients (pure). */
export const addRecent = (
  current: readonly RecentRecipient[],
  recipients: readonly Recipient[],
  now: number
): RecentRecipient[] => {
  const next = [...current];
  for (const recipient of recipients) {
    const email = recipient.email.trim();
    const index = next.findIndex((item) => sameEmail(item.email, email));
    const previous = index === -1 ? undefined : next[index];
    const typed = recipient.name?.trim() ?? "";
    const name = typed === "" ? previous?.name : typed;
    const entry: RecentRecipient = {
      count: (previous?.count ?? 0) + 1,
      email,
      lastUsed: now,
      ...(name === undefined || name === "" ? {} : { name }),
    };
    if (index === -1) {
      next.push(entry);
    } else {
      next[index] = entry;
    }
  }
  return next.toSorted((a, b) => b.lastUsed - a.lastUsed).slice(0, MAX_RECENT);
};

export const readRecent = (accountId: string): RecentRecipient[] => {
  try {
    const raw = globalThis.localStorage.getItem(key(accountId));
    const parsed = RecentSchema.safeParse(raw === null ? [] : JSON.parse(raw));
    return parsed.success ? parsed.data : [];
  } catch {
    // Storage disabled or corrupted: no suggestions from history.
    return [];
  }
};

export const rememberRecipients = (
  accountId: string,
  recipients: readonly Recipient[],
  now = Date.now()
): void => {
  try {
    globalThis.localStorage.setItem(
      key(accountId),
      JSON.stringify(addRecent(readRecent(accountId), recipients, now))
    );
  } catch {
    // Storage full or disabled: suggestions just won't learn.
  }
};

/** Forgets every account's recent recipients on this device (logout). */
export const forgetRecent = (): void => {
  try {
    const storage = globalThis.localStorage;
    const keys = Array.from({ length: storage.length }, (_, index) =>
      storage.key(index)
    );
    for (const name of keys) {
      if (name?.startsWith("nm-recent-recipients:") === true) {
        storage.removeItem(name);
      }
    }
  } catch {
    // Nothing stored.
  }
};

export const recentRecipients = async (): Promise<RecentRecipient[]> =>
  readRecent(await accountKey());

/** After a successful send: remember who it went to. */
export const rememberSent = async (
  recipients: readonly Recipient[]
): Promise<void> => {
  rememberRecipients(await accountKey(), recipients);
};

export interface RecipientSuggestion {
  readonly email: string;
  readonly name: string;
}

/** Lower case, no accents: "José" matches "jose". */
const fold = (text: string): string =>
  text.normalize("NFD").replaceAll(/\p{M}/gu, "").toLowerCase();

interface Person {
  /** Lower-case address, the key. */
  readonly id: string;
  readonly email: string;
  readonly name: string;
  readonly count: number;
  readonly lastUsed: number;
  /** Folded once (fold is slow-ish) for every keystroke's matching. */
  readonly foldedEmail: string;
  readonly foldedName: string;
  readonly words: readonly string[];
}

/**
 * Everyone suggestible, merged and folded: built once per address book and
 * history (the query data arrays), not on every keystroke.
 */
const indexes = new WeakMap<
  readonly Contact[],
  WeakMap<readonly RecentRecipient[], readonly Person[]>
>();

const buildIndex = (
  contacts: readonly Contact[],
  recent: readonly RecentRecipient[]
): readonly Person[] => {
  const people = new Map<
    string,
    { email: string; name: string; count: number; lastUsed: number }
  >();
  for (const item of recent) {
    people.set(item.email.toLowerCase(), {
      count: item.count,
      email: item.email,
      lastUsed: item.lastUsed,
      name: item.name ?? "",
    });
  }
  for (const contact of contacts) {
    const id = contact.email.toLowerCase();
    const known = people.get(id);
    // The address book's name wins over the one typed in a message.
    people.set(id, {
      count: known?.count ?? 0,
      email: contact.email,
      lastUsed: known?.lastUsed ?? 0,
      name: contact.name === "" ? (known?.name ?? "") : contact.name,
    });
  }
  return [...people.entries()].map(([id, person]) => {
    const foldedEmail = fold(person.email);
    const foldedName = fold(person.name);
    return {
      ...person,
      foldedEmail,
      foldedName,
      id,
      words: [...foldedName.split(/\s+/u), ...foldedEmail.split(/[@._+-]/u)],
    };
  });
};

const peopleOf = (
  contacts: readonly Contact[],
  recent: readonly RecentRecipient[]
): readonly Person[] => {
  let byRecent = indexes.get(contacts);
  if (byRecent === undefined) {
    byRecent = new WeakMap();
    indexes.set(contacts, byRecent);
  }
  let people = byRecent.get(recent);
  if (people === undefined) {
    people = buildIndex(contacts, recent);
    byRecent.set(recent, people);
  }
  return people;
};

/** 2: the name or address starts with it; 1: a word does; 0: no match. */
const matchScore = (query: string, person: Person): number => {
  if (
    person.foldedEmail.startsWith(query) ||
    person.foldedName.startsWith(query)
  ) {
    return 2;
  }
  return person.words.some((word) => word.startsWith(query)) ? 1 : 0;
};

export const suggestRecipients = (
  query: string,
  {
    contacts,
    exclude,
    limit = 6,
    recent,
  }: {
    readonly contacts: readonly Contact[];
    readonly recent: readonly RecentRecipient[];
    /** Already in the field. */
    readonly exclude: readonly string[];
    readonly limit?: number;
  }
): RecipientSuggestion[] => {
  const typed = fold(query.trim());
  if (typed === "") {
    return [];
  }
  const excluded = new Set(exclude.map((email) => email.trim().toLowerCase()));
  return peopleOf(contacts, recent)
    .filter((person) => !excluded.has(person.id))
    .map((person) => ({ person, score: matchScore(typed, person) }))
    .filter(({ score }) => score > 0)
    .toSorted(
      (a, b) =>
        b.score - a.score ||
        b.person.count - a.person.count ||
        b.person.lastUsed - a.person.lastUsed ||
        (a.person.name || a.person.email).localeCompare(
          b.person.name || b.person.email
        )
    )
    .slice(0, limit)
    .map(({ person }) => ({ email: person.email, name: person.name }));
};
