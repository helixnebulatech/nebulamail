// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which card, in which account. Address books other people share with us
// are other accounts of the JMAP session, and card ids are only unique
// within an account. In the URL (`/contacts/<key>`) our own cards keep
// their plain id; a shared card is `<account>.<card>` (a JMAP id can't
// have a ".", like the shared folders' URLs).

import {
  AccountId,
  AddressBookId,
  ContactCardId,
} from "@nebulamail/jmap-client";

export interface ContactRef {
  /** null: our own contacts account. */
  readonly accountId: AccountId | null;
  readonly id: ContactCardId;
}

export const contactKey = (ref: ContactRef): string =>
  ref.accountId === null ? ref.id : `${ref.accountId}.${ref.id}`;

const split = (key: string): [string | null, string] | null => {
  const parts = key.split(".");
  if (parts.length === 1) {
    return [null, key];
  }
  return parts.length === 2 ? [parts[0] ?? "", parts[1] ?? ""] : null;
};

export const parseContactKey = (key: string): ContactRef | null => {
  const parts = split(key);
  if (parts === null) {
    return null;
  }
  const [account, card] = parts;
  const id = ContactCardId.safeParse(card);
  const accountId = account === null ? null : AccountId.safeParse(account);
  if (!id.success || (accountId !== null && !accountId.success)) {
    return null;
  }
  return { accountId: accountId?.data ?? null, id: id.data };
};

export interface BookRef {
  readonly accountId: AccountId | null;
  readonly id: AddressBookId;
}

export const bookKey = (ref: BookRef): string =>
  ref.accountId === null ? ref.id : `${ref.accountId}.${ref.id}`;

export const parseBookKey = (key: string): BookRef | null => {
  const parts = split(key);
  if (parts === null) {
    return null;
  }
  const [account, book] = parts;
  const id = AddressBookId.safeParse(book);
  const accountId = account === null ? null : AccountId.safeParse(account);
  if (!id.success || (accountId !== null && !accountId.success)) {
    return null;
  }
  return { accountId: accountId?.data ?? null, id: id.data };
};
