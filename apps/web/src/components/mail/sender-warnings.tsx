// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// More about the sender of the open email, next to
// the look-alike domain label (reader-header.tsx):
//   - a display name that shows another address than the real one
//     (mail/sender-name.ts): red, shown even with «Verificado»;
//   - «Primera vez que te escribe» (mail/sender-new.ts): quiet, only for
//     mail to the user's own account from someone they never exchanged
//     mail with and who isn't in their contacts.
// Labels only: nothing is ever blocked.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Badge, Tooltip } from "@nebulamail/ui";
import { useQuery } from "@tanstack/react-query";
import { TriangleAlert, UserRound } from "lucide-react";

import { useContactsSupported } from "../../contacts/hooks";
import type { EmailDetail, Person } from "../../mail/api";
import { contactsKey, listContacts } from "../../mail/contacts";
import { sameAddress } from "../../mail/recipients";
import { senderNameWarning } from "../../mail/sender-name";
import {
  firstContactEligible,
  firstContactKey,
  isFirstContact,
} from "../../mail/sender-new";
import { useNewSenderHint } from "../../mail/use-new-sender-hint";
import { m } from "../../paraglide/messages.js";

/** The display name shows another address (sender-name.ts). */
export const SenderNameBadge = ({ person }: { readonly person: Person }) => {
  const warning = senderNameWarning(person.name, person.email);
  if (warning === null) {
    return null;
  }
  const real = cleanDisplayText(person.email);
  const tooltip = m.sender_name_address_tooltip({
    real,
    shown: cleanDisplayText(warning.shown),
  });
  return (
    <Tooltip content={tooltip}>
      <span data-sender-warning={warning.kind}>
        <Badge icon={TriangleAlert} tone="danger">
          {m.sender_name_address()}
        </Badge>
      </span>
    </Tooltip>
  );
};

/** Same cache as «Añadir a contactos» and the composer's suggestions. */
const useContacts = (enabled: boolean) =>
  useQuery({
    enabled,
    queryFn: listContacts,
    queryKey: contactsKey,
    staleTime: 5 * 60 * 1000,
  });

/** «Primera vez que te escribe» (sender-new.ts). */
export const NewSenderBadge = ({
  email,
  own,
}: {
  readonly email: EmailDetail;
  readonly own: readonly string[];
}) => {
  const [sender] = email.from;
  const [hintOn] = useNewSenderHint();
  // Off in Settings: no contacts load and no search (no request at all).
  const eligible = firstContactEligible(email, own, hintOn);
  const supported = useContactsSupported();
  const contacts = useContacts(eligible && supported.data === true);
  const contactsKnown =
    supported.data === false ||
    supported.isError ||
    contacts.data !== undefined ||
    contacts.isError;
  const inContacts =
    sender !== undefined &&
    (contacts.data ?? []).some((contact) =>
      sameAddress(contact.email, sender.email)
    );
  const address = sender?.email ?? "";
  const first = useQuery({
    enabled: eligible && contactsKnown && !inContacts,
    queryFn: async () =>
      await isFirstContact(address, email.receivedAt, email.id),
    queryKey: firstContactKey(address, email.receivedAt),
    // Earlier mail doesn't change (only if deleted): once is enough.
    staleTime: Number.POSITIVE_INFINITY,
  });
  if (!eligible || inContacts || first.data !== true) {
    return null;
  }
  return (
    <Tooltip content={m.sender_new_tooltip()}>
      <span data-sender-new>
        <Badge icon={UserRound} tone="neutral">
          {m.sender_new()}
        </Badge>
      </span>
    </Tooltip>
  );
};
