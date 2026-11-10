// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sharing our address books (RFC 9610 §2 `AddressBook.shareWith`, with the
// people of `Principal/query`, RFC 9670), like the folders (mail/
// sharing.ts). Books shared with us are other accounts of the session
// (api.ts lists their cards next to ours).
//
// Stalwart (its jmap-proto addressbook.rs): `shareWith` is set whole or
// per principal (`shareWith/<id>`), changing it needs the Share right, and
// the rights map to its ACLs: mayRead = Read + ReadItems, mayWrite =
// Modify + AddItems + ModifyItems + RemoveItems, mayDelete = Delete (the
// book itself), mayShare = Share; every myRights/shareWith also has
// `maySetMetadata`. So "write" also deletes contacts and "manage" also
// deletes the book. The bridge puts deleting contacts in mayDelete (DAV
// unbind), so there "delete contacts" needs "manage"; the levels below say
// so.

import type { AddressBookId, AddressBookRights } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  PrincipalId,
  hasAccountCapability,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import type { Person, Share, ShareLevel } from "../mail/sharing";
import { SHARE_LEVELS } from "../mail/sharing";

const RIGHT_NAMES = [
  "mayDelete",
  "mayRead",
  "mayShare",
  "mayWrite",
] as const satisfies readonly (keyof AddressBookRights)[];

/**
 * - read: see the contacts;
 * - write: also add and change contacts (and delete them, Stalwart);
 * - manage: also delete contacts and the book.
 * Re-sharing (`mayShare`) is never given.
 */
export const bookRightsFor = (level: ShareLevel): AddressBookRights => ({
  mayDelete: level === "manage",
  mayRead: true,
  mayShare: false,
  mayWrite: level !== "read",
});

/** The level these rights match, or "custom" (set by another client). */
export const bookLevelOf = (rights: AddressBookRights): ShareLevel | "custom" =>
  SHARE_LEVELS.find((level) => {
    const wanted = bookRightsFor(level);
    return RIGHT_NAMES.every(
      (name) => (rights[name] ?? false) === (wanted[name] ?? false)
    );
  }) ?? "custom";

/** Whether our books can be shared: the server has principals. */
export const bookSharingSupported = async (): Promise<boolean> => {
  const client = await getClient();
  try {
    const accountId = client.primaryAccountId(CAPABILITIES.contacts);
    return hasAccountCapability(
      client.session,
      CAPABILITIES.principals,
      accountId
    );
  } catch {
    return false;
  }
};

/** Who one of our books is shared with, and how. */
export const getBookShares = async (
  bookId: AddressBookId,
  people: readonly Person[]
): Promise<Share[]> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const [result] = await client.request((b) => [
    b.call("AddressBook/get", {
      accountId,
      ids: [bookId],
      properties: ["shareWith"],
    }),
  ]);
  const shareWith = result.list[0]?.shareWith ?? {};
  return Object.entries(shareWith).flatMap(([id, rights]) => {
    if (rights === undefined) {
      return [];
    }
    const person = people.find((candidate) => candidate.id === id) ?? {
      email: null,
      id: PrincipalId.parse(id),
      name: id,
      type: "individual",
    };
    return [{ level: bookLevelOf(rights), person }];
  });
};

/** Shares one of our books (or changes how); `null` stops sharing. */
export const setBookShare = async (
  bookId: AddressBookId,
  principalId: PrincipalId,
  level: ShareLevel | null
): Promise<void> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.principals);
    return [
      b.call("AddressBook/set", {
        accountId,
        update: {
          [bookId]: {
            [`shareWith/${principalId}`]:
              level === null ? null : bookRightsFor(level),
          },
        },
      }),
    ];
  });
  if (result.notUpdated[bookId] !== undefined) {
    throw new Error("The server did not change the sharing");
  }
};
