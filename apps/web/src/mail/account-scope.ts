// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which JMAP account the mail screens are showing (shared
// folders). The user's own account is `null` ("the primary one"); folders
// other users share appear in the session as other accounts
// (`urn:ietf:params:jmap:mail:share`), and every call for them must use
// their accountId, and every cached copy must be keyed by it.
//
// The account travels in the folder segment of the URL, so links to an
// email keep it without any other change: `/mail/inbox` is our inbox,
// `/mail/c.k/<email id>` is mailbox "k" of account "c". "." can't appear in
// a JMAP id (RFC 8620 §1.2: A-Z a-z 0-9 - _), so the split is unambiguous.

import { AccountId } from "@nebulamail/jmap-client";
import { useParams } from "@tanstack/react-router";

const SEPARATOR = ".";

/** `null`: the signed-in user's own (primary) account. */
export type MailAccount = AccountId | null;

export interface MailboxRef {
  readonly accountId: MailAccount;
  /** Role alias, "starred" or mailbox id, inside that account. */
  readonly mailbox: string;
}

export const parseMailboxParam = (param: string): MailboxRef => {
  const index = param.indexOf(SEPARATOR);
  if (index === -1) {
    return { accountId: null, mailbox: param };
  }
  const account = AccountId.safeParse(param.slice(0, index));
  // Not an account id: it resolves to no mailbox (the list says so).
  return account.success
    ? { accountId: account.data, mailbox: param.slice(index + 1) }
    : { accountId: null, mailbox: param };
};

export const mailboxParamFor = (
  accountId: MailAccount,
  mailbox: string
): string =>
  accountId === null ? mailbox : `${accountId}${SEPARATOR}${mailbox}`;

/** Cache key segment for an account ("" = own). */
export const accountKeyPart = (accountId: MailAccount): string =>
  accountId ?? "";

/** The account of the folder in the URL (own outside /mail). */
export const useMailAccount = (): MailAccount => {
  const { mailbox } = useParams({ strict: false });
  return mailbox === undefined ? null : parseMailboxParam(mailbox).accountId;
};
