// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Live updates of the contacts page (JMAP push over SSE, like
// calendar/push.ts): a change only says "cards changed", so the list and
// the open card are fetched again. Contacts added on a phone (or another
// CardDAV client behind the bridge) show up without reloading.

import { getClient } from "../mail/client";

const TYPES = ["AddressBook", "ContactCard"] as const;

/** Calls `onChange` for each contacts change (and after a reconnect). */
export const watchContacts = async (
  onChange: () => void,
  signal: AbortSignal
): Promise<void> => {
  const client = await getClient();
  const changes = client.push({ onConnect: onChange, signal, types: TYPES });
  for await (const change of changes) {
    const types = new Set(
      Object.values(change.changed).flatMap((perAccount) =>
        Object.keys(perAccount ?? {})
      )
    );
    if (TYPES.some((type) => types.has(type))) {
      onChange();
    }
  }
};
