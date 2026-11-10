// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// React hooks over the mail domain functions. Changes are optimistic: the
// list and the open email update at once and roll back if the server says
// no. Push (push.ts) then brings the server's truth.

import type { EmailFilter, EmailId, MailboxId } from "@nebulamail/jmap-client";
import type { InfiniteData, QueryClient } from "@tanstack/react-query";
import {
  useInfiniteQuery,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useEffectEvent } from "react";

import type { MailAccount } from "./account-scope";
import { useMailAccount } from "./account-scope";
import type {
  ConversationRow,
  EmailDetail,
  EmailPage,
  MailboxSummary,
  ThreadData,
  ThreadMember,
} from "./api";
import { moveTo, setFlagged, setSeen } from "./api";
import type { DraftAttachment } from "./attachments";
import type { DeliveryStatuses, RecipientStatus } from "./delivery";
import { loadInlineImages } from "./images";
import { startPush } from "./push";
import {
  accountQuery,
  deliveryQuery,
  emailListQuery,
  emailQuery,
  filteredListQuery,
  identitiesQuery,
  mailKeys,
  mailboxesQuery,
  savedIdentitiesQuery,
  searchQuery,
  threadQuery,
} from "./queries";

/** Folders of an account: the user's own unless one is given. */
export const useMailboxes = (account: MailAccount = null) =>
  useQuery(mailboxesQuery(account));

/** Folders of the account shown in the URL (own or shared). */
export const useAccountMailboxes = () => useMailboxes(useMailAccount());

// The hooks below read the account from the URL (account-scope.ts), so the
// screens don't pass it around.

export const useEmailList = (mailboxId: MailboxId | undefined) =>
  useInfiniteQuery(emailListQuery(mailboxId, useMailAccount()));

export const useSearch = (query: string) =>
  useInfiniteQuery(searchQuery(query, useMailAccount()));

export const useEmail = (id: EmailId | undefined) =>
  useQuery(emailQuery(id, useMailAccount()));

export const useThread = (id: EmailId) =>
  useQuery(threadQuery(id, useMailAccount()));

export const useFilteredList = (filter: EmailFilter | undefined) =>
  useInfiniteQuery(filteredListQuery(filter, useMailAccount()));

export const useIdentities = () => useQuery(identitiesQuery());

/** The identities saved on this device (identity-cache.ts). */
export const useSavedIdentities = () => useQuery(savedIdentitiesQuery());

const NO_IMAGES: ReadonlyMap<string, string> = new Map();

/**
 * `data:` copies of a draft's inline (`cid:`) images for the editor, by
 * normalized Content-ID. Empty while loading, offline or on error: the
 * editor shows placeholders.
 */
export const useInlineImages = (
  attachments: readonly DraftAttachment[]
): ReadonlyMap<string, string> => {
  const parts = attachments.flatMap((attachment) =>
    attachment.cid === undefined ? [] : [{ ...attachment, cid: attachment.cid }]
  );
  const query = useQuery({
    enabled: parts.length > 0,
    queryFn: async () =>
      await loadInlineImages(
        parts,
        parts.map((part) => part.cid)
      ),
    queryKey: [...mailKeys.all, "inline", ...parts.map((part) => part.blobId)],
    staleTime: Infinity,
  });
  return query.data ?? NO_IMAGES;
};

export const useAccount = () => useQuery(accountQuery());

/**
 * Delivery status of these emails (delivery.ts). Only the user's own
 * account has submissions: in a shared folder, nothing is asked.
 */
export const useDeliveryStatuses = (ids: readonly EmailId[]) => {
  const own = useMailAccount() === null;
  return useQuery(deliveryQuery(own ? ids : []));
};

const NO_DELIVERY: DeliveryStatuses = new Map();

/** What the cached delivery queries already know about these emails. */
const knownDelivery = (
  queryClient: QueryClient,
  ids: readonly EmailId[]
): DeliveryStatuses | undefined => {
  const wanted = new Set(ids);
  const found = new Map<EmailId, readonly RecipientStatus[]>();
  for (const [, data] of queryClient.getQueriesData<DeliveryStatuses>({
    queryKey: mailKeys.delivery(),
  })) {
    for (const [id, statuses] of data ?? NO_DELIVERY) {
      if (wanted.has(id) && !found.has(id)) {
        found.set(id, statuses);
      }
    }
  }
  return found.size === 0 ? undefined : found;
};

const mergeDelivery = (
  results: readonly { readonly data: DeliveryStatuses | undefined }[]
): DeliveryStatuses => {
  if (results.length === 1) {
    return results[0]?.data ?? NO_DELIVERY;
  }
  const merged = new Map<EmailId, readonly RecipientStatus[]>();
  for (const { data } of results) {
    for (const [id, statuses] of data ?? NO_DELIVERY) {
      merged.set(id, statuses);
    }
  }
  return merged;
};

/**
 * Delivery status of a list's rows, one query per loaded page: loading
 * more asks only for the new page, and a page whose emails changed shows
 * what was known while it loads again.
 */
export const useDeliveryStatusPages = (
  pages: readonly (readonly { readonly id: EmailId }[])[]
): DeliveryStatuses => {
  const own = useMailAccount() === null;
  const queryClient = useQueryClient();
  return useQueries({
    combine: mergeDelivery,
    queries: own
      ? pages.map((page) => {
          const ids = page.map((row) => row.id);
          return deliveryQuery(ids, () => knownDelivery(queryClient, ids));
        })
      : [],
  });
};

/** The user's own addresses (identities and login), shown as "me". */
export const useOwnAddresses = (): readonly string[] => {
  const identities = useIdentities();
  const account = useAccount();
  return [
    ...(identities.data ?? []).map((identity) => identity.email),
    ...(account.data === undefined ? [] : [account.data.username]),
  ];
};

/** The mailbox with a role ("inbox", "trash"…), once mailboxes are loaded. */
export const findRole = (
  mailboxes: readonly MailboxSummary[] | undefined,
  role: MailboxSummary["role"]
): MailboxSummary | undefined => mailboxes?.find((box) => box.role === role);

type Lists = InfiniteData<EmailPage, number>;

type Saved<T> = readonly (readonly [readonly unknown[], T | undefined])[];

interface Snapshot {
  readonly lists: Saved<Lists>;
  readonly threads: Saved<ThreadData | null>;
  readonly emails: Saved<EmailDetail | null>;
}

const snapshot = (client: QueryClient, account: MailAccount): Snapshot => ({
  emails: client.getQueriesData<EmailDetail | null>({
    queryKey: mailKeys.emailsOf(account),
  }),
  lists: client.getQueriesData<Lists>({ queryKey: mailKeys.listsOf(account) }),
  threads: client.getQueriesData<ThreadData | null>({
    queryKey: mailKeys.threadsOf(account),
  }),
});

const restore = (client: QueryClient, saved: Snapshot): void => {
  for (const [key, data] of [
    ...saved.lists,
    ...saved.threads,
    ...saved.emails,
  ]) {
    client.setQueryData(key, data);
  }
};

/** What an optimistic change can touch on any cached copy of an email. */
type Change = <T extends ThreadMember>(email: T) => T;

/**
 * Applies `change` to every cached copy of these emails: list rows and the
 * thread members inside them, open conversations and email details.
 * `removeRows`: rows whose own email changed leave the lists (moves).
 */
const patchEverywhere = (
  client: QueryClient,
  account: MailAccount,
  ids: ReadonlySet<EmailId>,
  change: Change,
  removeRows: boolean
): void => {
  const patch = <T extends ThreadMember>(email: T): T =>
    ids.has(email.id) ? change(email) : email;
  const patchRow = (row: ConversationRow): ConversationRow[] => {
    if (removeRows && ids.has(row.id)) {
      return [];
    }
    return [{ ...patch(row), members: row.members.map(patch) }];
  };
  client.setQueriesData<Lists>(
    { queryKey: mailKeys.listsOf(account) },
    (lists) =>
      lists === undefined
        ? lists
        : {
            ...lists,
            pages: lists.pages.map((page) => ({
              ...page,
              emails: page.emails.flatMap(patchRow),
              // Rows taken out here are not what the server's query state
              // holds: the next sync asks for the ids (sync.ts).
              queryState: removeRows ? null : page.queryState,
            })),
          }
  );
  client.setQueriesData<ThreadData | null>(
    { queryKey: mailKeys.threadsOf(account) },
    (thread) =>
      thread === undefined || thread === null
        ? thread
        : { ...thread, emails: thread.emails.map(patch) }
  );
  client.setQueriesData<EmailDetail | null>(
    { queryKey: mailKeys.emailsOf(account) },
    (email) => (email === undefined || email === null ? email : patch(email))
  );
};

const useOptimistic = <Variables extends { readonly ids: readonly EmailId[] }>(
  run: (variables: Variables, account: MailAccount) => Promise<void>,
  change: (variables: Variables) => Change,
  removeRows = false
) => {
  const client = useQueryClient();
  // Actions apply to the account shown in the URL (own or shared).
  const account = useMailAccount();
  return useMutation({
    mutationFn: async (variables: Variables) => {
      await run(variables, account);
    },
    onError: async (_error, _variables, saved: Snapshot | undefined) => {
      if (saved !== undefined) {
        restore(client, saved);
      }
      // A big change goes in several `Email/set` calls (maxObjectsInSet,
      // RFC 8620 §5.3) and some may have been applied before one failed:
      // the snapshot is then wrong too, so ask the server for the truth.
      await Promise.all(
        [
          mailKeys.listsOf(account),
          mailKeys.threadsOf(account),
          mailKeys.emailsOf(account),
        ].map(async (queryKey) => {
          await client.invalidateQueries({ queryKey });
        })
      );
    },
    onMutate: async (variables): Promise<Snapshot> => {
      await client.cancelQueries({ queryKey: mailKeys.all });
      const saved = snapshot(client, account);
      patchEverywhere(
        client,
        account,
        new Set(variables.ids),
        change(variables),
        removeRows
      );
      return saved;
    },
    onSettled: async () => {
      await client.invalidateQueries({ queryKey: mailKeys.mailboxes() });
    },
  });
};

export const useSetSeen = () =>
  useOptimistic(
    async (
      {
        ids,
        seen,
      }: {
        readonly ids: readonly EmailId[];
        readonly seen: boolean;
      },
      account
    ): Promise<void> => {
      await setSeen(ids, seen, account);
    },
    ({ seen }) =>
      (email) => ({ ...email, unread: !seen })
  );

export const useSetFlagged = () =>
  useOptimistic(
    async (
      {
        flagged,
        ids,
      }: {
        readonly ids: readonly EmailId[];
        readonly flagged: boolean;
      },
      account
    ): Promise<void> => {
      await setFlagged(ids, flagged, account);
    },
    ({ flagged }) =>
      (email) => ({ ...email, flagged })
  );

/**
 * Archive, trash, junk, move: rows of these emails leave the lists they
 * were in, and open conversations see them in their new folder.
 */
export const useMoveTo = () =>
  useOptimistic(
    async (
      {
        ids,
        keywords,
        mailboxId,
      }: {
        readonly ids: readonly EmailId[];
        readonly mailboxId: MailboxId;
        /** Set (true) or cleared (false) in the same update, e.g. `$junk`. */
        readonly keywords?: Readonly<Record<string, boolean>>;
      },
      account
    ): Promise<void> => {
      await moveTo(ids, mailboxId, keywords, account);
    },
    ({ mailboxId }) =>
      (email) => ({ ...email, mailboxIds: [mailboxId] }),
    true
  );

/**
 * Keeps the cache in sync with the server while the mail UI is open, and
 * shows new-mail notifications if enabled (notify.ts); clicking one calls
 * `onOpenEmail`.
 */
export const usePush = (onOpenEmail?: (id: EmailId) => void): void => {
  const client = useQueryClient();
  const openEmail = useEffectEvent((id: EmailId) => {
    onOpenEmail?.(id);
  });
  useEffect(() => {
    const controller = new AbortController();
    startPush(client, controller.signal, {
      onOpenEmail: (id) => {
        openEmail(id);
      },
    }).catch(() => {
      // Push is best effort: the app still works by refetching.
    });
    return () => {
      controller.abort();
    };
  }, [client]);
};
