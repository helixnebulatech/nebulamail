// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Searching the address book in the browser. Stalwart's `text` filter
// matches whole words only ("car" doesn't find "Carla"), so
// the cards are loaded and matched here, like the composer's suggestions:
// every word typed must start a word of the name, an address, a phone or
// the organization (accents and case don't matter).

import type { ContactSummary } from "./api";

/** Lower case, no accents: "José" matches "jose". */
export const fold = (text: string): string =>
  text.normalize("NFD").replaceAll(/\p{M}/gu, "").toLowerCase();

const digits = (text: string): string => text.replaceAll(/\D/gu, "");

/** A contact's text as the search compares it, worked out once. */
interface Folded {
  readonly name: string;
  readonly emails: readonly string[];
  /** Words of the name, the addresses and the organization. */
  readonly words: readonly string[];
  readonly phones: readonly string[];
}

/**
 * By card object: the list is the same objects until it is fetched again,
 * so typing doesn't fold thousands of cards on every key.
 */
const foldedCards = new WeakMap<ContactSummary, Folded>();

const foldedOf = (contact: ContactSummary): Folded => {
  let folded = foldedCards.get(contact);
  if (folded === undefined) {
    const name = fold(contact.name);
    const emails = contact.emails.map((email) => fold(email));
    folded = {
      emails,
      name,
      phones: contact.phones.map((phone) => digits(phone)),
      words: [
        ...name.split(/\s+/u),
        ...emails.flatMap((email) => email.split(/[@._+-]/u)),
        ...emails,
        ...fold(contact.organization).split(/\s+/u),
      ].filter((word) => word !== ""),
    };
    foldedCards.set(contact, folded);
  }
  return folded;
};

/**
 * 2: the name or an address starts with the query; 1: all words match, or
 * the digits typed appear in a phone number.
 */
const score = (
  contact: ContactSummary,
  query: string,
  words: string[],
  typed: string | null
) => {
  const folded = foldedOf(contact);
  if (typed !== null && folded.phones.some((phone) => phone.includes(typed))) {
    return 1;
  }
  if (
    folded.name.startsWith(query) ||
    folded.emails.some((email) => email.startsWith(query))
  ) {
    return 2;
  }
  return words.every((word) =>
    folded.words.some((candidate) => candidate.startsWith(word))
  )
    ? 1
    : 0;
};

/** Contacts matching `query`, best first; all of them for an empty query. */
export const searchContacts = (
  contacts: readonly ContactSummary[],
  query: string
): ContactSummary[] => {
  const folded = fold(query.trim());
  if (folded === "") {
    return [...contacts];
  }
  const words = folded.split(/\s+/u).filter((word) => word !== "");
  // Digits typed (3 or more) are looked for in phone numbers.
  const typed = digits(folded);
  const phone =
    /^[\d\s+().-]+$/u.test(folded) && typed.length >= 3 ? typed : null;
  return contacts
    .map((contact) => ({
      contact,
      value: score(contact, folded, words, phone),
    }))
    .filter((entry) => entry.value > 0)
    .toSorted((a, b) => b.value - a.value)
    .map((entry) => entry.contact);
};

const collators = new Map<string, Intl.Collator>();

/** One collator per locale (building one is slow). */
const collatorFor = (locale: string | undefined): Intl.Collator => {
  const key = locale ?? "";
  let collator = collators.get(key);
  if (collator === undefined) {
    collator = new Intl.Collator(locale, { sensitivity: "base" });
    collators.set(key, collator);
  }
  return collator;
};

/** A–Z order by name, "nameless" ones (no letters) last. */
export const sortContacts = (
  contacts: readonly ContactSummary[],
  locale?: string
): ContactSummary[] => {
  const collator = collatorFor(locale);
  return contacts.toSorted((a, b) => {
    const aEmpty = a.name === "";
    const bEmpty = b.name === "";
    if (aEmpty !== bEmpty) {
      return aEmpty ? 1 : -1;
    }
    return collator.compare(a.name, b.name);
  });
};

/** The letter a contact is listed under ("#" for digits and symbols). */
export const initialOf = (name: string): string => {
  const first = fold(name).trim().at(0)?.toUpperCase() ?? "";
  return /\p{L}/u.test(first) ? first : "#";
};
