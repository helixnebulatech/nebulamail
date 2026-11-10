// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The current list (folder, split, filter or search) and what the list and
// the reader share about it: the emails in order, the focused row, the
// selection, and how to open, close and step through messages.

import { EmailId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { createContext, use, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { parseMailboxParam } from "../../mail/account-scope";
import type { ConversationRow, MailboxRole } from "../../mail/api";
import {
  findRole,
  useAccountMailboxes,
  useFilteredList,
  useSearch as useMailSearch,
} from "../../mail/hooks";
import { listStatusOf } from "../../mail/list-status";
import type { ListStatus } from "../../mail/list-status";
import type { ThreadContext } from "../../mail/threads";
import type { MailboxView, ReadFilter, Split } from "../../mail/views";
import {
  DEFAULT_SPLITS,
  findSplit,
  listFilter,
  resolveView,
} from "../../mail/views";
import { m } from "../../paraglide/messages.js";
import { useComposer } from "./compose-context";

export interface ListSearch {
  readonly q?: string;
  readonly split?: string;
  readonly filter?: ReadFilter;
}

/**
 * The kind of folder, for what its rows show and do. "drafts": the user's
 * own Drafts (opening one edits it); a shared account's open as emails.
 */
export type FolderKind = "drafts" | "sent" | "other";

/** What the list shows. */
export interface ListData {
  /** The URL segment ("inbox", "starred", a mailbox id). */
  readonly mailbox: string;
  readonly view: MailboxView | null;
  readonly folderKind: FolderKind;
  readonly mailboxesLoaded: boolean;
  readonly search: ListSearch;
  readonly query: string;
  readonly split: Split;
  readonly splits: readonly Split[];
  readonly readFilter: ReadFilter;
  /** One row per conversation, newest first. */
  readonly emails: readonly ConversationRow[];
  /** The same rows as loaded, page by page (per-page queries). */
  readonly pages: readonly (readonly ConversationRow[])[];
  /** What conversations show and act on in this view (threads.ts). */
  readonly context: ThreadContext;
  readonly total: number;
  readonly status: ListStatus;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  /** The email in the URL (any email of a conversation). */
  readonly openId: EmailId | undefined;
  /** The row of the open conversation, if it is in the loaded list. */
  readonly openRowId: EmailId | undefined;
  /** The keyboard cursor's row (the first row until it moves). */
  readonly focusedId: EmailId | undefined;
  /**
   * Whether to draw the cursor: once it was moved (J/K, Tab, a click) or
   * an email is open. A list just opened shows no row as current.
   */
  readonly cursorShown: boolean;
  readonly selected: ReadonlySet<EmailId>;
  /** Whether any row is selected (`selected` not empty). */
  readonly hasSelection: boolean;
}

/** The cursor and the selection: they change with every J/K and click. */
type CursorFields = "cursorShown" | "focusedId" | "selected";

/** What changes it (the URL, the cursor, the selection, the query). */
export interface ListActions {
  readonly refetch: () => void;
  readonly loadMore: () => void;
  readonly setFocusedId: (id: EmailId | undefined) => void;
  readonly setSelected: (ids: ReadonlySet<EmailId>) => void;
  readonly open: (id: EmailId) => void;
  readonly close: () => void;
  readonly setSearch: (patch: ListSearch) => void;
}

/** Lookups on the rows that are neither data nor changes. */
export interface ListMeta {
  /** The row after (+1) or before (-1) this email's row in the list. */
  readonly neighbor: (id: EmailId, step: 1 | -1) => EmailId | undefined;
}

/**
 * The current list, split as state, actions and meta (composition rule
 * "state-context-interface"): the rows and panels only read this contract,
 * and ListProvider alone knows it comes from the URL and the JMAP queries.
 */
export interface ListState {
  readonly state: ListData;
  readonly actions: ListActions;
  readonly meta: ListMeta;
}

/**
 * The list without the cursor and the selection, for what shows or acts on
 * the open email (the reader and its toolbar): moving the cursor or
 * selecting rows doesn't render those again.
 */
export interface ListContent {
  readonly state: Omit<ListData, CursorFields>;
  readonly actions: ListActions;
  readonly meta: ListMeta;
}

const ListContext = createContext<ListState | null>(null);
const ListContentContext = createContext<ListContent | null>(null);

/** The row showing this email: its own, or the conversation it is in. */
export const rowIndex = (
  rows: readonly ConversationRow[],
  id: EmailId
): number => {
  const own = rows.findIndex((row) => row.id === id);
  return own === -1
    ? rows.findIndex((row) => row.members.some((member) => member.id === id))
    : own;
};

const FILTER_OPERATORS: Readonly<Record<ReadFilter, string>> = {
  all: "",
  attachment: "has:attachment",
  read: "is:read",
  unread: "is:unread",
};

let lastContext: ThreadContext | null = null;

/**
 * The previous context object while its ids are the same. Every row gets
 * it and a new object would redraw them all (EmailRow is memo) on each
 * cursor move; the compiler can't keep it (it goes into hooks, which may
 * change their arguments), so it is kept here. Any equal object is as
 * good as another, so sharing the last one is safe.
 */
const sameContext = (next: ThreadContext): ThreadContext => {
  const last = lastContext;
  if (
    last !== null &&
    last.draftsId === next.draftsId &&
    last.junkId === next.junkId &&
    last.mailboxId === next.mailboxId &&
    last.sentId === next.sentId &&
    last.trashId === next.trashId
  ) {
    return last;
  }
  lastContext = next;
  return next;
};

/** The emails of the current list: a folder view or a search. */
const useListData = (mailbox: string, search: ListSearch) => {
  const mailboxes = useAccountMailboxes();
  const view = resolveView(mailbox, mailboxes.data);
  const query = search.q?.trim() ?? "";
  const split = findSplit(DEFAULT_SPLITS, search.split);
  const readFilter = search.filter ?? "all";
  const roleId = (role: MailboxRole) =>
    findRole(mailboxes.data, role)?.id ?? null;
  const draftsId = roleId("drafts");
  const junkId = roleId("junk");
  const sentId = roleId("sent");
  const trashId = roleId("trash");
  const mailboxId =
    query === "" && view?.kind === "mailbox" ? view.mailbox.id : null;
  const excluded = [trashId, junkId].filter((id) => id !== null);
  const filter =
    query !== "" || view === null || view.kind === "missing"
      ? undefined
      : listFilter(view, { excluded, readFilter, split });
  const folderList = useFilteredList(filter);
  // The read filter also narrows a search (as operators search.ts knows).
  const searchList = useMailSearch(
    query === "" ? "" : `${query} ${FILTER_OPERATORS[readFilter]}`.trim()
  );
  const active = query === "" ? folderList : searchList;
  const context = sameContext({
    draftsId,
    junkId,
    mailboxId,
    sentId,
    trashId,
  });
  return {
    active,
    context,
    emails: active.data?.pages.flatMap((page) => page.emails) ?? [],
    pages: active.data?.pages.map((page) => page.emails) ?? [],
    mailboxesLoaded: mailboxes.isSuccess,
    query,
    readFilter,
    split,
    retry: () => {
      // The folder list waits for the folders: ask for those first.
      if (mailboxes.isError) {
        void mailboxes.refetch();
      }
      if (filter !== undefined || query !== "") {
        void active.refetch();
      }
    },
    status: listStatusOf({
      error: active.error,
      mailboxesFailed: mailboxes.isError,
      mailboxesLoaded: mailboxes.isSuccess,
      query,
      queryStatus: active.status,
      view,
    }),
    total: active.data?.pages[0]?.total ?? 0,
    view,
  };
};

export const ListProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const navigate = useNavigate();
  // Read through a ref: the composer changes with every window opened,
  // moved or closed, and the list's value (so every row panel) must not.
  const composer = useComposer();
  const composerRef = useRef(composer);
  useLayoutEffect(() => {
    composerRef.current = composer;
  });
  const toast = useToast();
  const { mailbox } = useParams({ from: "/mail/$mailbox" });
  const params = useParams({ strict: false });
  const search: ListSearch = useSearch({ from: "/mail/$mailbox" });
  const data = useListData(mailbox, search);
  const { active, context, emails, query, readFilter, split, view } = data;

  const openId =
    params.emailId === undefined
      ? undefined
      : EmailId.safeParse(params.emailId).data;
  const [focused, setFocused] = useState<EmailId | undefined>(undefined);
  const [selected, setSelected] = useState<ReadonlySet<EmailId>>(new Set());
  const [selectionKey, setSelectionKey] = useState(
    `${mailbox}|${query}|${split.id}|${readFilter}`
  );
  // A different list starts with nothing selected (state adjusted in render).
  const listKey = `${mailbox}|${query}|${split.id}|${readFilter}`;
  if (listKey !== selectionKey) {
    setSelectionKey(listKey);
    setSelected(new Set());
    setFocused(undefined);
  }
  const openRowId =
    openId === undefined ? undefined : emails[rowIndex(emails, openId)]?.id;
  const movedTo =
    focused !== undefined && emails.some((email) => email.id === focused)
      ? focused
      : undefined;
  const focusedId = movedTo ?? openRowId ?? emails[0]?.id;
  const cursorShown = movedTo !== undefined || openRowId !== undefined;

  const role = view?.kind === "mailbox" ? view.mailbox.role : null;
  // Drafts of a shared account open as emails: our composer saves to ours.
  const isDrafts =
    role === "drafts" && parseMailboxParam(mailbox).accountId === null;
  let folderKind: FolderKind = "other";
  if (isDrafts) {
    folderKind = "drafts";
  } else if (role === "sent") {
    folderKind = "sent";
  }

  const open = (id: EmailId): void => {
    setFocused(id);
    const read = (): void => {
      void navigate({
        params: { emailId: id, mailbox },
        search: (previous) => previous,
        to: "/mail/$mailbox/$emailId",
      });
    };
    if (isDrafts && query === "") {
      void (async () => {
        try {
          // Too long to edit: shown in the reader, which says it's cut.
          if ((await composerRef.current.openDraft(id)) === "too-large") {
            read();
          }
        } catch {
          toast.show({
            priority: "high",
            title: m.reader_error(),
            tone: "danger",
          });
        }
      })();
      return;
    }
    read();
  };
  const close = (): void => {
    void navigate({
      params: { mailbox },
      search: (previous) => previous,
      to: "/mail/$mailbox",
    });
  };

  const neighbor = (id: EmailId, step: 1 | -1): EmailId | undefined => {
    const index = rowIndex(emails, id);
    return index === -1 ? undefined : emails[index + step]?.id;
  };

  const setSearch = (patch: ListSearch): void => {
    void navigate({
      params: { mailbox },
      search: (previous) => {
        const next: Record<string, string | undefined> = {
          ...previous,
          ...patch,
        };
        // Drop defaults so URLs stay short.
        return Object.fromEntries(
          Object.entries(next).filter(
            ([key, value]) =>
              value !== undefined &&
              value !== "" &&
              !(key === "filter" && value === "all") &&
              !(key === "split" && value === "important")
          )
        );
      },
      to: "/mail/$mailbox",
    });
  };

  // Without the cursor and the selection (ListContent): the React
  // Compiler keeps this object while only those change.
  const content: ListContent = {
    actions: {
      close,
      loadMore: () => {
        if (!active.isFetchingNextPage) {
          void active.fetchNextPage();
        }
      },
      open,
      refetch: data.retry,
      setFocusedId: setFocused,
      setSearch,
      setSelected,
    },
    meta: { neighbor },
    state: {
      context,
      emails,
      folderKind,
      hasMore: active.hasNextPage,
      hasSelection: selected.size > 0,
      loadingMore: active.isFetchingNextPage,
      mailbox,
      mailboxesLoaded: data.mailboxesLoaded,
      openId,
      openRowId,
      pages: data.pages,
      query,
      readFilter,
      search,
      split,
      splits: DEFAULT_SPLITS,
      status: data.status,
      total: data.total,
      view,
    },
  };

  return (
    // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes `content` (see above).
    <ListContentContext value={content}>
      <ListContext
        // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object.
        value={{
          ...content,
          state: { ...content.state, cursorShown, focusedId, selected },
        }}
      >
        {children}
      </ListContext>
    </ListContentContext>
  );
};

export const useList = (): ListState => {
  const value = use(ListContext);
  if (value === null) {
    throw new Error("useList must be used inside ListProvider");
  }
  return value;
};

/** The list without the cursor and the selection (ListContent). */
export const useListContent = (): ListContent => {
  const value = use(ListContentContext);
  if (value === null) {
    throw new Error("useListContent must be used inside ListProvider");
  }
  return value;
};
