// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { ContactsPlaceholder } from "../components/contacts/contacts-page";

export const Route = createFileRoute("/contacts/")({
  component: ContactsPlaceholder,
});
