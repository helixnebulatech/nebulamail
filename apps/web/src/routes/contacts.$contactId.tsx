// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { ContactDetail } from "../components/contacts/contact-detail";
import { ContactsPlaceholder } from "../components/contacts/contacts-page";
import { parseContactKey } from "../contacts/ref";

const ContactRoute = () => {
  const { contactId } = Route.useParams();
  // Ours: the card id; shared with us: `<account>.<card>`.
  const ref = parseContactKey(contactId);
  // A new detail per contact: edit and delete dialogs don't carry over.
  return ref === null ? (
    <ContactsPlaceholder />
  ) : (
    <ContactDetail key={contactId} reference={ref} />
  );
};

export const Route = createFileRoute("/contacts/$contactId")({
  component: ContactRoute,
});
