// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import type { EmailFilter } from "@nebulamail/jmap-client";
import {
  AccountId,
  EmailId,
  MailboxId,
  State,
  ThreadId,
} from "@nebulamail/jmap-client";
import {
  InfiniteQueryObserver,
  QueryClient,
  QueryObserver,
} from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import type { ConversationRow, EmailPage, ThreadData } from "./api";
import { PAGE_SIZE } from "./api";
import { mailKeys } from "./queries";
import { firstContactKey } from "./sender-new";
import type { Baseline, Delta, Lists, SyncServer } from "./sync";
import {
  applyQueryChanges,
  createMailSync,
  listQueryState,
  rowChanged,
  rowsToFetch,
  threadChanged,
  toPages,
  updatedList,
} from "./sync";

const INBOX = MailboxId.parse("inbox");
const FILTER: EmailFilter = { inMailbox: INBOX };
const PRIMARY = AccountId.parse("a");

const row = (
  id: string,
  members: readonly string[] = [id]
): ConversationRow => ({
  draft: false,
  flagged: false,
  from: [],
  hasAttachment: false,
  id: EmailId.parse(id),
  mailboxIds: [INBOX],
  members: members.map((member) => ({
    draft: false,
    flagged: false,
    from: [],
    hasAttachment: false,
    id: EmailId.parse(member),
    mailboxIds: [INBOX],
    receivedAt: "2026-10-30T10:00:00Z",
    unread: true,
  })),
  preview: "",
  receivedAt: "2026-10-30T10:00:00Z",
  subject: id,
  threadId: ThreadId.parse(`t-${id}`),
  to: [],
  unread: true,
});

/** For observers: the test reads the cache, not the updates. */
const ignore = (): void => {
  // Nothing.
};

const ids = (...list: string[]) => list.map((id) => EmailId.parse(id));

const nothing = { emails: new Set<string>(), threads: new Set<string>() };

describe(rowChanged, () => {
  it("is true when the row, one of its emails or its thread changed", () => {
    const conversation = row("e2", ["e1", "e2"]);

    expect(rowChanged(conversation, nothing)).toBeFalsy();
    expect(
      rowChanged(conversation, { ...nothing, emails: new Set(["e2"]) })
    ).toBeTruthy();
    expect(
      rowChanged(conversation, { ...nothing, emails: new Set(["e1"]) })
    ).toBeTruthy();
    expect(
      rowChanged(conversation, { ...nothing, threads: new Set(["t-e2"]) })
    ).toBeTruthy();
  });
});

describe(rowsToFetch, () => {
  it("asks for new rows and changed ones, not the rest", () => {
    const cached = new Map([
      [EmailId.parse("e1"), row("e1")],
      [EmailId.parse("e2"), row("e2")],
    ]);

    expect(
      rowsToFetch(ids("e3", "e1", "e2"), cached, {
        ...nothing,
        emails: new Set(["e2"]),
      })
    ).toStrictEqual(ids("e3", "e2"));
  });
});

describe(applyQueryChanges, () => {
  it("swaps a thread's row when a reply arrives, and back when it goes", () => {
    // collapseThreads: B, A; a reply R to A becomes A's row.
    const arrived = applyQueryChanges(ids("B", "A"), ids("A"), [
      { id: EmailId.parse("R"), index: 0 },
    ]);
    expect(arrived).toStrictEqual(ids("R", "B"));
    // R deleted: A is the row again, after B.
    expect(
      applyQueryChanges(arrived, ids("A", "R"), [
        { id: EmailId.parse("A"), index: 1 },
      ])
    ).toStrictEqual(ids("B", "A"));
  });

  it("removes first, then adds lowest index first (RFC 8620 §5.6)", () => {
    expect(
      applyQueryChanges(ids("a", "b", "c", "d"), ids("b", "d"), [
        { id: EmailId.parse("y"), index: 3 },
        { id: EmailId.parse("x"), index: 1 },
      ])
    ).toStrictEqual(ids("a", "x", "c", "y"));
  });

  it("adds at the end of what is known, not past it", () => {
    expect(
      applyQueryChanges(
        ids("a", "b"),
        [],
        [
          { id: EmailId.parse("x"), index: 2 },
          { id: EmailId.parse("y"), index: 9 },
        ]
      )
    ).toStrictEqual(ids("a", "b", "x"));
  });

  it("moves an added id that is already there instead of doubling it", () => {
    expect(
      applyQueryChanges(
        ids("a", "b", "c"),
        [],
        [{ id: EmailId.parse("c"), index: 0 }]
      )
    ).toStrictEqual(ids("c", "a", "b"));
  });
});

/** A list with one page per state. */
const listAt = (...states: (string | null)[]): Lists => ({
  pageParams: states.map((_, index) => index * PAGE_SIZE),
  pages: states.map((state) => ({
    emails: [],
    filter: FILTER,
    next: null,
    queryState: state === null ? null : State.parse(state),
    total: 0,
  })),
});

describe(listQueryState, () => {
  it("is the state of all its pages, or null when they differ", () => {
    expect(listQueryState(listAt("q1", "q1"))).toBe("q1");
    expect(listQueryState(listAt("q1", "q2"))).toBeNull();
    expect(listQueryState(listAt("q1", null))).toBeNull();
    expect(listQueryState(listAt(null))).toBeNull();
  });
});

describe(toPages, () => {
  it("cuts the rows into pages with their positions", () => {
    const rows = Array.from({ length: PAGE_SIZE + 3 }, (_, index) =>
      row(`e${index}`)
    );
    const lists = toPages(rows, 200, FILTER, null);

    expect(lists.pageParams).toStrictEqual([0, PAGE_SIZE]);
    expect(lists.pages.map((page) => page.emails.length)).toStrictEqual([
      PAGE_SIZE,
      3,
    ]);
    expect(lists.pages.map((page) => page.next)).toStrictEqual([
      PAGE_SIZE,
      PAGE_SIZE + 3,
    ]);
  });

  it("ends the list when everything is there, and keeps an empty one", () => {
    expect(toPages([row("e1")], 1, FILTER, null).pages[0]?.next).toBeNull();
    expect(toPages([], 0, FILTER, State.parse("q1"))).toStrictEqual({
      pageParams: [0],
      pages: [
        {
          emails: [],
          filter: FILTER,
          next: null,
          queryState: State.parse("q1"),
          total: 0,
        },
      ],
    });
  });
});

describe(updatedList, () => {
  it("follows the server's order, reusing what didn't change", () => {
    const old1 = row("e1");
    const old2 = row("e2");
    const new2 = { ...row("e2"), unread: false };
    const lists = updatedList(
      { ids: ids("e3", "e2", "e1", "gone"), queryState: null, total: 4 },
      new Map([
        [old1.id, old1],
        [old2.id, old2],
      ]),
      new Map([
        [EmailId.parse("e3"), row("e3")],
        [new2.id, new2],
      ]),
      new Set(ids("e3", "e2", "gone")),
      FILTER
    );
    const emails = lists.pages[0]?.emails ?? [];

    expect(emails.map((email) => email.id)).toStrictEqual(
      ids("e3", "e2", "e1")
    );
    expect(emails[1]).toBe(new2);
    expect(emails[2]).toBe(old1);
  });
});

describe(threadChanged, () => {
  const thread: ThreadData = {
    emails: [row("e1"), row("e2")],
    threadId: ThreadId.parse("t1"),
    total: 2,
  };

  it("looks at the email it was opened from, its thread and its emails", () => {
    expect(threadChanged("e9", thread, nothing)).toBeFalsy();
    expect(
      threadChanged("e9", thread, { ...nothing, emails: new Set(["e9"]) })
    ).toBeTruthy();
    expect(
      threadChanged("e9", thread, { ...nothing, emails: new Set(["e2"]) })
    ).toBeTruthy();
    expect(
      threadChanged("e9", thread, { ...nothing, threads: new Set(["t1"]) })
    ).toBeTruthy();
    expect(threadChanged("e9", null, nothing)).toBeFalsy();
  });
});

// --- createMailSync -------------------------------------------------------------

const baseline = (n: number): Baseline => ({
  email: State.parse(`e${n}`),
  thread: State.parse(`t${n}`),
});

const page = (rows: readonly ConversationRow[], total: number): EmailPage => ({
  emails: rows,
  filter: FILTER,
  next: rows.length < total ? rows.length : null,
  queryState: null,
  total,
});

interface Setup {
  readonly queryClient: QueryClient;
  readonly listFn: () => Promise<EmailPage>;
  readonly listKey: readonly unknown[];
  readonly server: SyncServer & {
    readonly baseline: ReturnType<typeof vi.fn<SyncServer["baseline"]>>;
    readonly delta: ReturnType<typeof vi.fn<SyncServer["delta"]>>;
    readonly rows: ReturnType<typeof vi.fn<SyncServer["rows"]>>;
  };
  readonly stop: () => void;
}

/** A query client with the inbox list on screen (an observer). */
const setup = (rows: readonly ConversationRow[], total: number): Setup => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const listKey = mailKeys.list(INBOX);
  const listFn = vi.fn<() => Promise<EmailPage>>(
    async () => await Promise.resolve(page(rows, total))
  );
  queryClient.setQueryData<Lists>(listKey, {
    pageParams: [0],
    pages: [page(rows, total)],
  });
  const observer = new InfiniteQueryObserver(queryClient, {
    getNextPageParam: (last: EmailPage) => last.next ?? undefined,
    initialPageParam: 0,
    queryFn: listFn,
    queryKey: listKey,
  });
  const unsubscribe = observer.subscribe(ignore);
  const server = {
    baseline: vi.fn<SyncServer["baseline"]>(
      async () => await Promise.resolve(baseline(1))
    ),
    delta: vi.fn<SyncServer["delta"]>(),
    primary: PRIMARY,
    rows: vi.fn<SyncServer["rows"]>(),
  };
  return { listFn, listKey, queryClient, server, stop: unsubscribe };
};

/** Starts the sync and waits for what it fetches again. */
const ready = async (
  sync: ReturnType<typeof createMailSync>,
  queryClient: QueryClient
): Promise<void> => {
  await sync.reset();
  await vi.waitFor(() => {
    expect(queryClient.isFetching()).toBe(0);
  });
};

const delta = (
  windowIds: readonly string[],
  total: number,
  changed: Partial<Delta["changed"]> = {}
): Delta => ({
  baseline: baseline(2),
  changed: { ...nothing, ...changed },
  windows: [{ ids: ids(...windowIds), queryState: null, total }],
});

describe(createMailSync, () => {
  it("leaves a list that didn't move as it was", async () => {
    const { listKey, queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    const before = queryClient.getQueryData<Lists>(listKey);
    server.delta.mockResolvedValue({
      ...delta(["e1"], 1),
      baseline: baseline(1),
    });
    server.rows.mockResolvedValue(new Map());

    await sync.changed(PRIMARY);

    expect(queryClient.getQueryData<Lists>(listKey)).toBe(before);
    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBeFalsy();
    stop();
  });

  it("asks from the list's rows and state, and keeps the new state", async () => {
    const { listKey, queryClient, server, stop } = setup(
      [row("e2"), row("e1")],
      2
    );
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    const shown = queryClient.getQueryData<Lists>(listKey);
    queryClient.setQueryData<Lists>(listKey, {
      pageParams: [0],
      pages: (shown?.pages ?? []).map((item) => ({
        ...item,
        queryState: State.parse("q1"),
      })),
    });
    const before = queryClient.getQueryData<Lists>(listKey);
    server.delta.mockResolvedValue({
      ...delta(["e2", "e1"], 2),
      windows: [
        { ids: ids("e2", "e1"), queryState: State.parse("q2"), total: 2 },
      ],
    });
    server.rows.mockResolvedValue(new Map());

    await sync.changed(PRIMARY);

    expect(server.delta.mock.calls[0]?.[2]).toStrictEqual([
      {
        filter: FILTER,
        ids: ids("e2", "e1"),
        limit: PAGE_SIZE,
        queryState: State.parse("q1"),
      },
    ]);
    // Same rows (the same objects), new state.
    const after = queryClient.getQueryData<Lists>(listKey);
    expect(after?.pages[0]?.queryState).toBe("q2");
    expect(after?.pages[0]?.emails).toBe(before?.pages[0]?.emails);
    stop();
  });

  it("fetches only the new row and keeps the others", async () => {
    const e1 = row("e1");
    const e2 = row("e2");
    const { listKey, queryClient, server, stop } = setup([e2, e1], 2);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    server.delta.mockResolvedValue(delta(["e3", "e2", "e1"], 3));
    server.rows.mockResolvedValue(new Map([[EmailId.parse("e3"), row("e3")]]));

    await sync.changed(PRIMARY);

    expect(server.delta).toHaveBeenCalledWith(PRIMARY, baseline(1), [
      {
        filter: FILTER,
        ids: ids("e2", "e1"),
        limit: PAGE_SIZE,
        queryState: null,
      },
    ]);
    expect(server.rows).toHaveBeenCalledWith(PRIMARY, ids("e3"));
    const [first] = queryClient.getQueryData<Lists>(listKey)?.pages ?? [];
    expect(first?.emails.map((email) => email.id)).toStrictEqual(
      ids("e3", "e2", "e1")
    );
    expect(first?.emails[1]).toStrictEqual(e2);
    expect(first?.total).toBe(3);
    stop();
  });

  it("doesn't pile up resets when reconnects come fast", async () => {
    const { queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);

    const resets = Array.from({ length: 5 }, sync.reset);
    await Promise.all(resets);

    expect(server.baseline.mock.calls.length).toBeLessThanOrEqual(2);
    stop();
  });

  it("doesn't search again for «first time writing to you» on a reconnect", async () => {
    const { listFn, queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    const search = vi.fn<() => Promise<boolean>>(
      async () => await Promise.resolve(true)
    );
    const observer = new QueryObserver(queryClient, {
      queryFn: search,
      queryKey: firstContactKey("ana@example.com", "2026-11-03T09:00:00Z"),
    });
    const unsubscribe = observer.subscribe(ignore);
    await vi.waitFor(() => {
      expect(search).toHaveBeenCalledOnce();
    });

    await ready(sync, queryClient);

    // The list is fetched again; the earlier-mail search isn't.
    expect(listFn).toHaveBeenCalledOnce();
    expect(search).toHaveBeenCalledOnce();
    unsubscribe();
    stop();
  });

  it("syncs from the states it reached last time", async () => {
    const { queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    server.delta.mockResolvedValue(delta(["e1"], 1));
    server.rows.mockResolvedValue(new Map());

    await sync.changed(PRIMARY);
    await sync.changed(PRIMARY);

    expect(server.delta.mock.calls.map(([, since]) => since)).toStrictEqual([
      baseline(1),
      baseline(2),
    ]);
    expect(server.rows).toHaveBeenLastCalledWith(PRIMARY, []);
    stop();
  });

  it("only fetches an open email again if it changed", async () => {
    const { queryClient, server, stop } = setup([row("e1")], 1);
    const fetchEmail = vi.fn<() => Promise<null>>(
      async () => await Promise.resolve(null)
    );
    const observers = ["e1", "e2"].map(
      (id) =>
        new QueryObserver(queryClient, {
          queryFn: fetchEmail,
          queryKey: mailKeys.email(EmailId.parse(id)),
          staleTime: Infinity,
        })
    );
    const stops = observers.map((observer) => observer.subscribe(ignore));
    await vi.waitFor(() => {
      expect(fetchEmail).toHaveBeenCalledTimes(2);
    });
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    await vi.waitFor(() => {
      expect(fetchEmail).toHaveBeenCalledTimes(4);
    });
    server.delta.mockResolvedValue(
      delta(["e1"], 1, { emails: new Set(["e2"]) })
    );
    server.rows.mockResolvedValue(new Map());

    await sync.changed(PRIMARY);

    await vi.waitFor(() => {
      expect(fetchEmail).toHaveBeenCalledTimes(5);
    });
    expect(
      queryClient.getQueryState(mailKeys.email(EmailId.parse("e1")))
        ?.isInvalidated
    ).toBeFalsy();
    for (const unsubscribe of stops) {
      unsubscribe();
    }
    stop();
  });

  it("starts over when the changes can't be worked out", async () => {
    const { queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    server.delta.mockResolvedValue(null);
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");

    await sync.changed(PRIMARY);

    expect(server.rows).not.toHaveBeenCalled();
    expect(server.baseline).toHaveBeenCalledTimes(2);
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: mailKeys.listsOf(null),
    });
    stop();
  });

  it("starts over when fetching the rows fails", async () => {
    const { queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    server.delta.mockResolvedValue(delta(["e2", "e1"], 2));
    server.rows.mockRejectedValue(new Error("offline"));
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");

    await sync.changed(PRIMARY);

    expect(invalidate).toHaveBeenCalledWith({
      queryKey: mailKeys.listsOf(null),
    });
    stop();
  });

  it("fetches a list again if it changed while syncing", async () => {
    const { listFn, listKey, queryClient, server, stop } = setup(
      [row("e1")],
      1
    );
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    expect(listFn).toHaveBeenCalledOnce();
    server.delta.mockResolvedValue(delta(["e2", "e1"], 2));
    server.rows.mockImplementation(async () => {
      // e.g. an optimistic "mark as read" while the rows were loading.
      queryClient.setQueryData<Lists>(listKey, {
        pageParams: [0],
        pages: [page([{ ...row("e1"), unread: false }], 1)],
      });
      return await Promise.resolve(new Map([[EmailId.parse("e2"), row("e2")]]));
    });

    await sync.changed(PRIMARY);

    // Not written over: fetched again from the server.
    await vi.waitFor(() => {
      expect(listFn).toHaveBeenCalledTimes(2);
    });
    const [first] = queryClient.getQueryData<Lists>(listKey)?.pages ?? [];
    expect(first?.emails.map((email) => email.id)).toStrictEqual(ids("e1"));
    stop();
  });

  it("doesn't touch lists of other accounts, and starts a new account over", async () => {
    const { listKey, queryClient, server, stop } = setup([row("e1")], 1);
    const sync = createMailSync(queryClient, server);
    await ready(sync, queryClient);
    const shared = AccountId.parse("b");

    await sync.changed(shared);

    expect(server.delta).not.toHaveBeenCalled();
    expect(server.baseline).toHaveBeenLastCalledWith(shared);
    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBeFalsy();
    stop();
  });
});
