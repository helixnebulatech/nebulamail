// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Keeps the cached mail up to date when push says Email or Thread changed.
// Instead of fetching every
// list page, open email and conversation again, one request asks:
//
//   - `Email/changes` and `Thread/changes` since the last sync: which
//     emails and threads changed or went away;
//   - `Email/queryChanges` of each list on screen (RFC 8620 §5.6), from the
//     `queryState` its pages were read at: which rows left it and which
//     came in, where. They are applied to the rows it shows
//     (`applyQueryChanges`).
//
// A list without one query state (its pages read at different states, or
// rows taken out here before the server said so), or whose queryChanges
// the server refuses (`cannotCalculateChanges`, `tooManyChanges`), is asked
// again with `Email/query`, ids only, as many as it shows, in a second
// request.
//
// Then only the rows that are new or changed are fetched (`getRows`); the
// others are reused from the cache. Open emails and conversations are only
// fetched again if they changed.
//
// When the changes can't be worked out (`Email/changes` or `Thread/changes`
// refused, too many changes, an error), everything of that account is
// fetched again, as before.

import type {
  AccountId,
  EmailFilter,
  EmailId,
  JmapClient,
  State,
} from "@nebulamail/jmap-client";
import type {
  InfiniteData,
  InvalidateQueryFilters,
  QueryClient,
} from "@tanstack/react-query";

import type { MailAccount } from "./account-scope";
import type { ConversationRow, EmailPage, ThreadData } from "./api";
import { LIST_SORT, PAGE_SIZE, getRows } from "./api";
import { mailKeys } from "./queries";
import { FIRST_CONTACT } from "./sender-new";

/** A cached list: the pages TanStack Query keeps. */
export type Lists = InfiniteData<EmailPage, number>;

/** The states the cache of an account was last brought up to. */
export interface Baseline {
  readonly email: State;
  readonly thread: State;
}

/** Emails and threads that changed or went away since the baseline. */
export interface Changed {
  readonly emails: ReadonlySet<string>;
  readonly threads: ReadonlySet<string>;
}

/** A list to ask about: its filter, the rows it shows, its query state. */
export interface ListWindow {
  readonly filter: EmailFilter;
  /** How many rows to ask for when asking the ids again. */
  readonly limit: number;
  /** The rows it shows, in order (the start of the list). */
  readonly ids: readonly EmailId[];
  /** The state those rows are from; null: ask the ids again. */
  readonly queryState: State | null;
}

/** What a list holds now: its first rows, its size and query state. */
export interface WindowIds {
  readonly ids: readonly EmailId[];
  readonly total: number;
  readonly queryState: State | null;
}

export interface Delta {
  readonly baseline: Baseline;
  readonly changed: Changed;
  /** One per window asked for; null: that query failed. */
  readonly windows: readonly (WindowIds | null)[];
}

/** The server side (`jmapSyncServer`; a fake in the tests). */
export interface SyncServer {
  readonly primary: AccountId;
  /** The current states, to sync from later. */
  baseline: (accountId: AccountId) => Promise<Baseline>;
  /** null: the changes can't be worked out; start over. */
  delta: (
    accountId: AccountId,
    since: Baseline,
    windows: readonly ListWindow[]
  ) => Promise<Delta | null>;
  rows: (
    accountId: AccountId,
    ids: readonly EmailId[]
  ) => Promise<ReadonlyMap<EmailId, ConversationRow>>;
}

// --- Pure parts ---------------------------------------------------------------

/** The row has to be fetched again: it or its thread changed. */
export const rowChanged = (row: ConversationRow, changed: Changed): boolean =>
  changed.threads.has(row.threadId) ||
  changed.emails.has(row.id) ||
  row.members.some((member) => changed.emails.has(member.id));

/** Rows of the new list that are not in the cache or changed. */
export const rowsToFetch = (
  ids: readonly EmailId[],
  cached: ReadonlyMap<EmailId, ConversationRow>,
  changed: Changed
): EmailId[] =>
  ids.filter((id) => {
    const row = cached.get(id);
    return row === undefined || rowChanged(row, changed);
  });

/** One row added to a query's results, where it is now (RFC 8620 §5.6). */
export interface AddedItem {
  readonly id: EmailId;
  readonly index: number;
}

/**
 * The start of a list after `Email/queryChanges` (RFC 8620 §5.6): every
 * removed id goes, then the added ones go in at their index, lowest first.
 * Those past the end of what is known are left out (the rows between are
 * not known here); an added id still in the list is moved, not doubled.
 */
export const applyQueryChanges = (
  ids: readonly EmailId[],
  removed: readonly EmailId[],
  added: readonly AddedItem[]
): EmailId[] => {
  const gone = new Set<EmailId>([...removed, ...added.map((item) => item.id)]);
  const result = ids.filter((id) => !gone.has(id));
  const inOrder = added.toSorted((a, b) => a.index - b.index);
  for (const item of inOrder) {
    if (item.index > result.length) {
      break;
    }
    result.splice(item.index, 0, item.id);
  }
  return result;
};

/** Rows cut into pages as TanStack Query expects (`next`: position). */
export const toPages = (
  rows: readonly ConversationRow[],
  total: number,
  filter: EmailFilter,
  queryState: State | null
): Lists => {
  const pages: EmailPage[] = [];
  const pageParams: number[] = [];
  for (let start = 0; start === 0 || start < rows.length; start += PAGE_SIZE) {
    const end = Math.min(start + PAGE_SIZE, rows.length);
    pages.push({
      emails: rows.slice(start, end),
      filter,
      next: end < rows.length || end < total ? end : null,
      queryState,
      total,
    });
    pageParams.push(start);
  }
  return { pageParams, pages };
};

/**
 * The list in the server's order: fetched rows where they were fetched,
 * cached ones elsewhere. A fetched email that is gone is left out.
 */
export const updatedList = (
  window: WindowIds,
  cached: ReadonlyMap<EmailId, ConversationRow>,
  fetched: ReadonlyMap<EmailId, ConversationRow>,
  refetched: ReadonlySet<EmailId>,
  filter: EmailFilter
): Lists =>
  toPages(
    window.ids.flatMap((id) => {
      const row = refetched.has(id) ? fetched.get(id) : cached.get(id);
      return row === undefined ? [] : [row];
    }),
    window.total,
    filter,
    window.queryState
  );

/** An open conversation (keyed by one of its emails) changed. */
export const threadChanged = (
  keyEmail: unknown,
  data: ThreadData | null | undefined,
  changed: Changed
): boolean =>
  (typeof keyEmail === "string" && changed.emails.has(keyEmail)) ||
  (data !== undefined &&
    data !== null &&
    (changed.threads.has(data.threadId) ||
      data.emails.some((email) => changed.emails.has(email.id))));

// --- Server -------------------------------------------------------------------

/** More changes than this: fetching everything again is cheaper. */
const MAX_CHANGES = 500;

export const jmapSyncServer = (client: JmapClient): SyncServer => ({
  baseline: async (accountId) => {
    const [emails, threads] = await client.request((b) => [
      b.call("Email/get", { accountId, ids: [] }),
      b.call("Thread/get", { accountId, ids: [] }),
    ]);
    return { email: emails.state, thread: threads.state };
  },
  delta: async (accountId, since, windows) => {
    const stated = windows.flatMap((window, index) =>
      window.queryState === null
        ? []
        : [{ index, queryState: window.queryState, window }]
    );
    const [emails, threads, ...changes] = await client.requestSettled(
      (b) =>
        [
          b.call("Email/changes", {
            accountId,
            maxChanges: MAX_CHANGES,
            sinceState: since.email,
          }),
          b.call("Thread/changes", {
            accountId,
            maxChanges: MAX_CHANGES,
            sinceState: since.thread,
          }),
          // Lists without a state are asked again below.
          ...stated.map(({ queryState, window }) =>
            b.call("Email/queryChanges", {
              accountId,
              calculateTotal: true,
              collapseThreads: true,
              filter: window.filter,
              maxChanges: MAX_CHANGES,
              sinceQueryState: queryState,
              sort: LIST_SORT,
            })
          ),
        ] as const
    );
    if (
      !emails.ok ||
      !threads.ok ||
      emails.value.hasMoreChanges ||
      threads.value.hasMoreChanges
    ) {
      return null;
    }
    const applied: (WindowIds | null)[] = windows.map(() => null);
    for (const [at, { index, queryState, window }] of stated.entries()) {
      const result = changes[at];
      // Refused (cannotCalculateChanges, tooManyChanges…), or not an
      // answer from that state, or without the size: asked again below.
      if (
        result?.ok === true &&
        result.value.oldQueryState === queryState &&
        result.value.total !== undefined
      ) {
        applied[index] = {
          ids: applyQueryChanges(
            window.ids,
            result.value.removed,
            result.value.added
          ),
          queryState: result.value.newQueryState,
          total: result.value.total,
        };
      }
    }
    const again = windows.flatMap((window, index) =>
      applied[index] === null ? [{ index, window }] : []
    );
    const asked =
      again.length === 0
        ? []
        : await client
            .requestSettled((b) =>
              again.map(({ window }) =>
                b.call("Email/query", {
                  accountId,
                  calculateTotal: true,
                  collapseThreads: true,
                  filter: window.filter,
                  limit: window.limit,
                  position: 0,
                  sort: LIST_SORT,
                })
              )
            )
            .catch(() => []);
    const queried = new Map(
      again.map(({ index }, at): [number, WindowIds | null] => {
        const query = asked[at];
        return [
          index,
          query?.ok === true
            ? {
                ids: query.value.ids,
                queryState: query.value.queryState,
                total: query.value.total ?? query.value.ids.length,
              }
            : null,
        ];
      })
    );
    return {
      baseline: {
        email: emails.value.newState,
        thread: threads.value.newState,
      },
      changed: {
        emails: new Set([...emails.value.updated, ...emails.value.destroyed]),
        threads: new Set([
          ...threads.value.updated,
          ...threads.value.destroyed,
        ]),
      },
      windows: applied.map(
        (window, index) => window ?? queried.get(index) ?? null
      ),
    };
  },
  primary: client.primaryAccountId(),
  rows: async (accountId, ids) => await getRows(client, accountId, ids),
});

// --- Cache --------------------------------------------------------------------

export interface MailSync {
  /** (Re)connected: changes may have been missed. Fetch everything again. */
  readonly reset: () => Promise<void>;
  /** Push said Email or Thread changed in this account. */
  readonly changed: (accountId: AccountId) => Promise<void>;
}

/** The list holds the same rows, in the same order, and nothing to fetch. */
const unchanged = (
  rows: readonly ConversationRow[],
  window: WindowIds,
  total: number,
  refetched: ReadonlySet<EmailId>
): boolean =>
  refetched.size === 0 &&
  window.total === total &&
  window.ids.length === rows.length &&
  window.ids.every((id, index) => rows[index]?.id === id);

/**
 * The state a list's rows are from: the one of all its pages, or null
 * when they differ or one is unknown (then its ids are asked again).
 */
export const listQueryState = (data: Lists): State | null => {
  const [first, ...rest] = data.pages.map((page) => page.queryState);
  return first !== undefined &&
    first !== null &&
    rest.every((state) => state === first)
    ? first
    : null;
};

/** The same pages with another query state (the rows didn't move). */
const withQueryState = (data: Lists, queryState: State | null): Lists => ({
  ...data,
  pages: data.pages.map((page) => ({ ...page, queryState })),
});

export const createMailSync = (
  queryClient: QueryClient,
  server: SyncServer
): MailSync => {
  const baselines = new Map<AccountId, Baseline>();
  // One sync at a time, in order: each starts from where the last ended.
  let queue: Promise<void> = Promise.resolve();
  let resetWaiting = false;
  const serial = async (job: () => Promise<void>): Promise<void> => {
    const next = queue.then(job).catch(() => {
      // Push is best effort; the next change or reconnect catches up.
    });
    queue = next;
    await next;
  };

  const mailAccount = (accountId: AccountId): MailAccount =>
    accountId === server.primary ? null : accountId;

  /**
   * Marks these queries stale and fetches again those on screen, without
   * waiting: a slow or failing email must not hold up the next sync.
   */
  const refetch = (filters: InvalidateQueryFilters): void => {
    void queryClient.invalidateQueries(filters);
  };

  /**
   * Starts over: the states first, then everything again, so what comes
   * back is at least as new as the baseline.
   */
  const reload = async (accountId: AccountId): Promise<void> => {
    baselines.delete(accountId);
    const baseline = await server.baseline(accountId).catch(() => null);
    if (baseline !== null) {
      baselines.set(accountId, baseline);
    }
    const account = mailAccount(accountId);
    for (const queryKey of [
      mailKeys.listsOf(account),
      mailKeys.threadsOf(account),
      mailKeys.emailsOf(account),
    ]) {
      refetch({ queryKey });
    }
  };

  const update = async (accountId: AccountId): Promise<void> => {
    const since = baselines.get(accountId);
    if (since === undefined) {
      await reload(accountId);
      return;
    }
    const account = mailAccount(accountId);
    // Lists on screen and at rest are updated in place; the others
    // (hidden, loading) are just marked stale or fetched again below.
    const lists = queryClient
      .getQueryCache()
      .findAll({ queryKey: mailKeys.listsOf(account) })
      .flatMap((query) => {
        const data = queryClient.getQueryData<Lists>(query.queryKey);
        const filter = data?.pages[0]?.filter;
        if (
          !query.isActive() ||
          query.state.fetchStatus !== "idle" ||
          data === undefined ||
          filter === undefined
        ) {
          return [];
        }
        const rows = data.pages.flatMap((page) => page.emails);
        return [
          {
            cached: new Map(rows.map((row) => [row.id, row])),
            data,
            filter,
            hash: query.queryHash,
            key: query.queryKey,
            limit: Math.max(rows.length, PAGE_SIZE),
            queryState: listQueryState(data),
            rows,
            total: data.pages.at(-1)?.total ?? rows.length,
          },
        ];
      });
    const delta = await server.delta(
      accountId,
      since,
      lists.map(({ filter, limit, queryState, rows }) => ({
        filter,
        ids: rows.map((row) => row.id),
        limit,
        queryState,
      }))
    );
    if (delta === null) {
      await reload(accountId);
      return;
    }
    const { changed } = delta;
    const plans = lists.flatMap((list, index) => {
      const window = delta.windows[index] ?? null;
      return window === null
        ? []
        : [
            {
              ...list,
              refetch: new Set(rowsToFetch(window.ids, list.cached, changed)),
              window,
            },
          ];
    });
    const fetched = await server.rows(accountId, [
      ...new Set(plans.flatMap((plan) => [...plan.refetch])),
    ]);
    baselines.set(accountId, delta.baseline);
    const updated = new Set<string>();
    for (const plan of plans) {
      // A page loaded or an optimistic change landed meanwhile: that list
      // is fetched again instead.
      if (queryClient.getQueryData(plan.key) !== plan.data) {
        continue;
      }
      updated.add(plan.hash);
      if (unchanged(plan.rows, plan.window, plan.total, plan.refetch)) {
        // Nothing moved: no new render, but the next queryChanges starts
        // from the new state.
        if (plan.window.queryState !== plan.queryState) {
          queryClient.setQueryData<Lists>(
            plan.key,
            withQueryState(plan.data, plan.window.queryState)
          );
        }
      } else {
        queryClient.setQueryData<Lists>(
          plan.key,
          updatedList(
            plan.window,
            plan.cached,
            fetched,
            plan.refetch,
            plan.filter
          )
        );
      }
    }
    refetch({
      predicate: (query) => !updated.has(query.queryHash),
      queryKey: mailKeys.listsOf(account),
    });
    refetch({
      predicate: (query) => {
        // [..."mail", "email", account, id]
        const id = query.queryKey.at(3);
        return typeof id === "string" && changed.emails.has(id);
      },
      queryKey: mailKeys.emailsOf(account),
    });
    refetch({
      predicate: (query) =>
        threadChanged(
          query.queryKey.at(3),
          queryClient.getQueryData<ThreadData | null>(query.queryKey),
          changed
        ),
      queryKey: mailKeys.threadsOf(account),
    });
  };

  return {
    changed: async (accountId) => {
      await serial(async () => {
        await update(accountId).catch(async () => {
          await reload(accountId);
        });
      });
    },
    reset: async () => {
      // Reconnects can come every second (a flaky network): one waiting
      // reset is enough.
      if (resetWaiting) {
        await queue;
        return;
      }
      resetWaiting = true;
      await serial(async () => {
        resetWaiting = false;
        baselines.clear();
        const baseline = await server
          .baseline(server.primary)
          .catch(() => null);
        if (baseline !== null) {
          baselines.set(server.primary, baseline);
        }
        // Not «first time writing to you» (sender-new.ts): it only looks
        // at earlier mail, which a missed change doesn't make different,
        // and every reconnect would search again.
        refetch({
          predicate: (query) =>
            query.queryKey[mailKeys.all.length] !== FIRST_CONTACT,
          queryKey: mailKeys.all,
        });
      });
    },
  };
};
