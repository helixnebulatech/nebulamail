// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TanStack Query options for the mail data. Mail is only fetched in the
// browser (the access token lives in memory), never during SSR.
//
// Every key of account data has the account in it (account-scope.ts): ids
// like "k" repeat across accounts. The short forms (`lists()`, `emails()`…)
// are prefixes over all accounts, for invalidation.

import type { EmailFilter, EmailId, MailboxId } from "@nebulamail/jmap-client";
import { isMethodError } from "@nebulamail/jmap-client";
import {
  infiniteQueryOptions,
  queryOptions,
  skipToken,
} from "@tanstack/react-query";

import { connectedAccountKey } from "./account-key";
import type { MailAccount } from "./account-scope";
import { accountKeyPart } from "./account-scope";
import type { EmailPage, MailboxSummary } from "./api";
import {
  getAccount,
  getEmail,
  getThread,
  listEmails,
  listMailboxes,
  queryEmails,
} from "./api";
import type { Identity } from "./compose";
import { listIdentities } from "./compose";
import type { DeliveryStatuses } from "./delivery";
import { getDeliveryStatuses } from "./delivery";
import { loadIdentities, saveIdentities } from "./identity-cache";
import { parseSearch } from "./search";

export const mailKeys = {
  account: () => [...mailKeys.all, "account"] as const,
  all: ["mail"] as const,
  /** Delivery status of sent emails (delivery.ts), own account only. */
  delivery: () => [...mailKeys.all, "delivery"] as const,
  email: (id: EmailId, account: MailAccount = null) =>
    [...mailKeys.emailsOf(account), id] as const,
  emails: () => [...mailKeys.all, "email"] as const,
  emailsOf: (account: MailAccount) =>
    [...mailKeys.emails(), accountKeyPart(account)] as const,
  filtered: (filter: EmailFilter, account: MailAccount = null) =>
    [...mailKeys.listsOf(account), "filter", JSON.stringify(filter)] as const,
  identities: () => [...mailKeys.all, "identities"] as const,
  list: (mailboxId: MailboxId, account: MailAccount = null) =>
    [...mailKeys.listsOf(account), mailboxId] as const,
  lists: () => [...mailKeys.all, "list"] as const,
  listsOf: (account: MailAccount) =>
    [...mailKeys.lists(), accountKeyPart(account)] as const,
  /** Folders of every account (prefix). */
  mailboxes: () => [...mailKeys.all, "mailboxes"] as const,
  mailboxesOf: (account: MailAccount) =>
    [...mailKeys.mailboxes(), accountKeyPart(account)] as const,
  search: (query: string, account: MailAccount = null) =>
    [...mailKeys.listsOf(account), "search", query] as const,
  /** Keyed by the opened email: its thread id isn't known before. */
  thread: (id: EmailId, account: MailAccount = null) =>
    [...mailKeys.threadsOf(account), id] as const,
  threads: () => [...mailKeys.all, "thread"] as const,
  threadsOf: (account: MailAccount) =>
    [...mailKeys.threads(), accountKeyPart(account)] as const,
};

/** How long an early folders' request waits for its query to take it. */
const EARLY_MAILBOXES_MS = 10_000;

let earlyMailboxes:
  | {
      readonly account: MailAccount;
      readonly at: number;
      readonly result: Promise<MailboxSummary[]>;
    }
  | undefined;

/**
 * The page's first load (router.tsx `hydrate`): the folders' request
 * starts right after the JMAP session, not when the screen that shows them
 * has hydrated. Never into React Query's cache from here: data there
 * before hydration would make the first render differ from the server's
 * (React error #418). The folders' query takes the answer when it first
 * runs; a failure is its failure (it retries with a new request).
 */
export const startMailboxesEarly = (account: MailAccount = null): void => {
  const result = listMailboxes(account);
  result.catch(() => null);
  earlyMailboxes = { account, at: Date.now(), result };
};

const takeEarlyMailboxes = (
  account: MailAccount
): Promise<MailboxSummary[]> | undefined => {
  const early = earlyMailboxes;
  if (
    early === undefined ||
    early.account !== account ||
    Date.now() - early.at > EARLY_MAILBOXES_MS
  ) {
    return undefined;
  }
  earlyMailboxes = undefined;
  return early.result;
};

export const mailboxesQuery = (account: MailAccount = null) =>
  queryOptions({
    queryFn: async () =>
      await (takeEarlyMailboxes(account) ?? listMailboxes(account)),
    queryKey: mailKeys.mailboxesOf(account),
  });

/** `undefined` (mailboxes not loaded yet) → the query waits. */
export const emailListQuery = (
  mailboxId: MailboxId | undefined,
  account: MailAccount = null
) =>
  infiniteQueryOptions({
    getNextPageParam: (page: EmailPage) => page.next ?? undefined,
    initialPageParam: 0,
    queryFn:
      mailboxId === undefined
        ? skipToken
        : async ({ pageParam }) =>
            await listEmails(mailboxId, pageParam, account),
    queryKey: [...mailKeys.listsOf(account), mailboxId ?? ""],
  });

export const emailQuery = (
  id: EmailId | undefined,
  account: MailAccount = null
) =>
  queryOptions({
    queryFn:
      id === undefined ? skipToken : async () => await getEmail(id, account),
    queryKey: [...mailKeys.emailsOf(account), id ?? ""],
  });

/** The conversation of an email (all its thread, without bodies). */
export const threadQuery = (id: EmailId, account: MailAccount = null) =>
  queryOptions({
    queryFn: async () => await getThread(id, account),
    queryKey: mailKeys.thread(id, account),
  });

/** `null` filter (empty search box) → the query stays disabled. */
export const searchQuery = (query: string, account: MailAccount = null) => {
  const filter: EmailFilter | null = parseSearch(query);
  return infiniteQueryOptions({
    getNextPageParam: (page: EmailPage) => page.next ?? undefined,
    initialPageParam: 0,
    queryFn:
      filter === null
        ? skipToken
        : async ({ pageParam }) =>
            await queryEmails(filter, pageParam, account),
    queryKey: mailKeys.search(query.trim(), account),
    // The general `retry: 1` (router.tsx), except when the server refused
    // the search: asking again gets the same answer.
    retry: (failures: number, error: unknown) =>
      failures < 1 && !isMethodError(error, "unsupportedFilter"),
  });
};

/** Any list (folder + split + read filter); `undefined` waits. */
export const filteredListQuery = (
  filter: EmailFilter | undefined,
  account: MailAccount = null
) =>
  infiniteQueryOptions({
    getNextPageParam: (page: EmailPage) => page.next ?? undefined,
    initialPageParam: 0,
    queryFn:
      filter === undefined
        ? skipToken
        : async ({ pageParam }) =>
            await queryEmails(filter, pageParam, account),
    queryKey:
      filter === undefined
        ? [...mailKeys.listsOf(account), "filter", ""]
        : mailKeys.filtered(filter, account),
  });

const rememberIdentities = async (
  identities: readonly Identity[]
): Promise<void> => {
  await saveIdentities(await connectedAccountKey(), identities);
};

export const identitiesQuery = () =>
  queryOptions({
    queryFn: async () => {
      const identities = await listIdentities();
      // For writing offline next time (identity-cache.ts).
      rememberIdentities(identities).catch(() => null);
      return identities;
    },
    queryKey: mailKeys.identities(),
    staleTime: Infinity,
  });

/**
 * The identities saved on this device, while the server's aren't there
 * (opened offline). Read from IndexedDB, so it runs without network. If
 * the session is up and belongs to someone else, none.
 */
export const savedIdentitiesQuery = () =>
  queryOptions({
    networkMode: "always",
    queryFn: async (): Promise<readonly Identity[]> => {
      const saved = await loadIdentities().catch(() => null);
      if (saved === null) {
        return [];
      }
      const current = await connectedAccountKey().catch(() => null);
      return current === null || current === saved.account
        ? saved.identities
        : [];
    },
    queryKey: [...mailKeys.identities(), "saved"],
    retry: false,
    staleTime: Infinity,
  });

export const accountQuery = () =>
  queryOptions({
    queryFn: getAccount,
    queryKey: mailKeys.account(),
    staleTime: Infinity,
  });

/**
 * Delivery status of sent emails (delivery.ts). Kept until push says an
 * EmailSubmission changed (push.ts); an empty list waits.
 */
export const deliveryQuery = (
  ids: readonly EmailId[],
  placeholder?: () => DeliveryStatuses | undefined
) => {
  const sorted = [...new Set(ids)].toSorted();
  return queryOptions({
    // What other delivery queries already know about these emails, shown
    // while this one loads (a page whose emails changed): no flicker.
    ...(placeholder === undefined ? {} : { placeholderData: placeholder }),
    queryFn:
      sorted.length === 0
        ? skipToken
        : async () => await getDeliveryStatuses(sorted),
    queryKey: [...mailKeys.delivery(), ...sorted],
    retry: false,
    staleTime: 60_000,
  });
};
