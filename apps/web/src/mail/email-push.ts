// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Delivered mail inside the push (Stalwart 1.0 `emailPush`,
// decided 2026-11-01). Asked for with the "EmailDelivery" subscription
// (lib/web-push.ts) when the server can: the server then pushes an
// `EmailPush` with these few properties of each new unread inbox email
// instead of a StateChange, and the worker (pwa/sw.ts) shows it with no
// token and no request (also in the static build, which has no token
// there). Only what the notification needs: never the body or the
// preview. Without the capability (the bridge, other servers) nothing
// changes: StateChange and `Email/changes`, as before.

import type { EmailPushConfig, JmapClient } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  EMAIL_PUSH,
  hasAccountCapability,
  hasCapability,
} from "@nebulamail/jmap-client";

import { pushAccountKey, setEmailPushTarget } from "./push-store";

/** What the worker reads of each email (sw.ts `worthAnnouncing`). */
export const EMAIL_PUSH_PROPERTIES = [
  "id",
  "from",
  "subject",
  "receivedAt",
  "keywords",
  "mailboxIds",
] as const;

/** The `emailPush` of a subscription for the primary mail account. */
export const emailPushConfig = (
  accountId: string,
  inboxId: string
): Readonly<Record<string, EmailPushConfig>> => ({
  [accountId]: {
    // The server leaves out what isn't new unread inbox mail; the worker
    // checks again (and junk, and age), the push being untrusted.
    filter: { inMailbox: inboxId, notKeyword: "$seen" },
    properties: [...EMAIL_PUSH_PROPERTIES],
    urgency: "normal",
  },
});

/** Whether the server can put mail of the primary account in the push. */
export const canEmailPush = (client: JmapClient): boolean => {
  try {
    const accountId = client.primaryAccountId(CAPABILITIES.mail);
    // Stalwart announces it in the session and on the user's own account.
    return (
      hasCapability(client.session, EMAIL_PUSH) &&
      hasAccountCapability(client.session, EMAIL_PUSH, accountId)
    );
  } catch {
    return false;
  }
};

const inboxIdOf = async (client: JmapClient): Promise<string | null> => {
  const accountId = client.primaryAccountId(CAPABILITIES.mail);
  const [found] = await client.request((b) => [
    b.call("Mailbox/get", { accountId, ids: null, properties: ["role"] }),
  ]);
  return found.list.find((box) => box.role === "inbox")?.id ?? null;
};

/**
 * The `emailPush` to ask for, or null (the server can't, or no inbox).
 * Tells the worker whose mail it will get and where the inbox is.
 */
export const prepareEmailPush = async (
  client: JmapClient
): Promise<Readonly<Record<string, EmailPushConfig>> | null> => {
  if (!canEmailPush(client)) {
    await setEmailPushTarget(null);
    return null;
  }
  const inboxId = await inboxIdOf(client);
  const accountId = client.primaryAccountId(CAPABILITIES.mail);
  if (inboxId === null) {
    await setEmailPushTarget(null);
    return null;
  }
  await setEmailPushTarget({
    // Same key as mail-push.ts `mailPushAccount`.
    account: pushAccountKey(
      client.session.apiUrl.origin,
      client.session.username,
      accountId
    ),
    accountId,
    inboxId,
  });
  return emailPushConfig(accountId, inboxId);
};
