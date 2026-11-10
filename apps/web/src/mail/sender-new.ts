// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Primera vez que te escribe»: a
// sender you have never exchanged mail with. Phishing often comes from a
// fresh address with a familiar name; a quiet hint helps to notice it.
//
// The signal, cheap and on the user's own server only (nothing leaves it):
//   - not in the address book (the contacts already loaded for «Añadir a
//     contactos», same cache), and
//   - no earlier email in the account from that address, or to it or with
//     it in Cc (mail we sent them counts as knowing them): one
//     `Email/query` with `before` = this email's arrival, plus `Email/get`
//     of the From/To/Cc of what it finds, in the same request. The text
//     filters (`from`, `to`, `cc`) may match more than the exact address
//     (Stalwart searches words), so the hits are checked here; a hit that
//     isn't really that address doesn't count.
// "Earlier" is what makes the answer stable: the first email of a sender
// says so even after more arrive, and the second never does.
//
// Only for the user's own account and mail that isn't ours, and only with
// the setting on (use-new-sender-hint.ts; off: no request at all); one
// request per opened email (cached for the session), none in list rows. Any
// error, an unsupported filter or a server without search: no hint (it
// is a hint, never a warning to rely on).

import type { EmailId, JmapClient } from "@nebulamail/jmap-client";

import { getClient } from "./client";
import { mailKeys } from "./queries";
import { sameAddress } from "./recipients";

/**
 * Whether an email gets the first-contact check at all (contacts and the
 * search): the setting is on, one From that isn't one of our addresses,
 * in our own account (`accountId` null), not a draft.
 */
export const firstContactEligible = (
  email: {
    readonly from: readonly { readonly email: string }[];
    readonly accountId: unknown;
    readonly draft: boolean;
  },
  own: readonly string[],
  enabled: boolean
): boolean => {
  const [sender] = email.from;
  return (
    enabled &&
    sender !== undefined &&
    email.from.length === 1 &&
    email.accountId === null &&
    !email.draft &&
    own.length > 0 &&
    !own.some((address) => sameAddress(address, sender.email))
  );
};

/** Enough to see past a few loose matches of the text filters. */
const LIMIT = 10;
/** RFC 5321: 254 octets. Longer is never a real address (and isn't searched). */
const MAX_ADDRESS = 254;

/** The part of `firstContactKey` after `mailKeys.all`. */
export const FIRST_CONTACT = "first-contact";

export const firstContactKey = (address: string, before: string) =>
  [
    ...mailKeys.all,
    FIRST_CONTACT,
    address.trim().toLowerCase(),
    before,
  ] as const;

interface Hit {
  readonly id: EmailId;
  readonly from?: readonly { readonly email: string }[] | null;
  readonly to?: readonly { readonly email: string }[] | null;
  readonly cc?: readonly { readonly email: string }[] | null;
}

/** Whether one of the hits (other than this email) really has the address. */
export const anyExchange = (
  hits: readonly Hit[],
  address: string,
  self: EmailId
): boolean =>
  hits.some(
    (hit) =>
      hit.id !== self &&
      [...(hit.from ?? []), ...(hit.to ?? []), ...(hit.cc ?? [])].some(
        (person) => sameAddress(person.email, address)
      )
  );

/**
 * True when the account has no mail from, to or copying `address` that
 * arrived before `before` (this email's `receivedAt`). Null when it can't
 * tell (odd address, server error): nothing is shown then.
 */
export const isFirstContact = async (
  address: string,
  before: string,
  self: EmailId,
  client?: JmapClient
): Promise<boolean | null> => {
  const clean = address.trim();
  if (
    clean === "" ||
    clean.length > MAX_ADDRESS ||
    !clean.includes("@") ||
    /[\s"<>]/u.test(clean) ||
    !Number.isFinite(Date.parse(before))
  ) {
    return null;
  }
  const jmap = client ?? (await getClient());
  const accountId = jmap.primaryAccountId();
  try {
    const [, got] = await jmap.request((b) => {
      const query = b.call("Email/query", {
        accountId,
        filter: {
          conditions: [
            { before },
            {
              conditions: [{ from: clean }, { to: clean }, { cc: clean }],
              operator: "OR",
            },
          ],
          operator: "AND",
        },
        limit: LIMIT,
      });
      const get = b.call("Email/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["from", "to", "cc"],
      });
      return [query, get] as const;
    });
    return !anyExchange(got.list, clean, self);
  } catch {
    return null;
  }
};
