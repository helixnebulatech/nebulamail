// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Drafts and Sent mailboxes (RFC 8621 §2: `role` "drafts" and
// "sent"), looked up with one Mailbox/get and remembered per client and
// account: every autosave and send needs them, and they rarely change.
// Forgotten when the server says mailboxes changed (push.ts, RFC 8620
// §7.1 StateChange "Mailbox") and when an Email/set refuses the ids
// (`invalidProperties` on `mailboxIds`, RFC 8621 §4.6): the caller then
// asks again and retries once, only if the ids are really different.

import type {
  AccountId,
  JmapClient,
  MailboxId,
  SetError,
} from "@nebulamail/jmap-client";

export interface MailboxRoles {
  readonly drafts: MailboxId | null;
  readonly sent: MailboxId | null;
}

/** Replaced by `forgetMailboxRoles`, which drops every remembered lookup. */
let remembered = new WeakMap<
  JmapClient,
  Map<AccountId, Promise<MailboxRoles>>
>();

const lookUp = async (
  client: JmapClient,
  accountId: AccountId
): Promise<MailboxRoles> => {
  const [result] = await client.request((b) => [
    b.call("Mailbox/get", { accountId, properties: ["role"] }),
  ]);
  const byRole = (role: string): MailboxId | null =>
    result.list.find((item) => item.role === role)?.id ?? null;
  return { drafts: byRole("drafts"), sent: byRole("sent") };
};

/** The account's Drafts and Sent ids, one Mailbox/get the first time. */
export const mailboxRoles = async (
  client: JmapClient,
  accountId: AccountId
): Promise<MailboxRoles> => {
  let byAccount = remembered.get(client);
  if (byAccount === undefined) {
    byAccount = new Map();
    remembered.set(client, byAccount);
  }
  const known = byAccount.get(accountId);
  if (known !== undefined) {
    return await known;
  }
  const map = byAccount;
  const pending = lookUp(client, accountId);
  map.set(accountId, pending);
  try {
    return await pending;
  } catch (error) {
    // A failed lookup is not remembered: the next save asks again.
    if (map.get(accountId) === pending) {
      map.delete(accountId);
    }
    throw error;
  }
};

/** Forgets every remembered lookup (mailboxes changed). */
export const forgetMailboxRoles = (): void => {
  remembered = new WeakMap();
};

/** The id of a role, or an error saying the account lacks it. */
export const roleId = (
  roles: MailboxRoles,
  role: keyof MailboxRoles
): MailboxId => {
  const id = roles[role];
  if (id === null) {
    throw new Error(`The account has no ${role} mailbox`);
  }
  return id;
};

/**
 * Whether a SetError may be about stale mailbox ids (RFC 8621 §4.6:
 * `invalidProperties` naming `mailboxIds`, or naming nothing).
 */
export const mayBeStaleMailbox = (error: SetError | undefined): boolean =>
  error?.type === "invalidProperties" &&
  (error.properties === undefined ||
    error.properties === null ||
    error.properties.some((property) => property.startsWith("mailboxIds")));

/**
 * After a refusal that may be about stale ids: forgets them and looks
 * again. The fresh roles if any of `used` changed (worth one retry), else
 * null (the refusal was about something else).
 */
export const freshRolesIfChanged = async (
  client: JmapClient,
  accountId: AccountId,
  used: MailboxRoles
): Promise<MailboxRoles | null> => {
  forgetMailboxRoles();
  const fresh = await mailboxRoles(client, accountId);
  return fresh.drafts === used.drafts && fresh.sent === used.sent
    ? null
    : fresh;
};
