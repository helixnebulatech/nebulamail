// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the contacts sidebar, the top bar's search and the list share: the
// search text, All / Groups, and the dialogs either of them opens (new
// contact or group, address books).

import { createContext, use, useMemo, useRef, useState } from "react";
import type { ReactNode, RefObject } from "react";

export type ContactsFilter = "all" | "groups";

/** What the search box asks of the list (Arrow down, Enter). */
export interface ContactsListKeys {
  readonly focusFirst: () => void;
  readonly openFirst: () => void;
}

interface ContactsUi {
  readonly query: string;
  readonly setQuery: (query: string) => void;
  readonly filter: ContactsFilter;
  readonly setFilter: (filter: ContactsFilter) => void;
  readonly creating: "individual" | "group" | null;
  readonly setCreating: (kind: "individual" | "group" | null) => void;
  readonly books: boolean;
  readonly setBooks: (open: boolean) => void;
  readonly listKeys: RefObject<ContactsListKeys | null>;
  /** The list says what Arrow down and Enter in the search do. */
  readonly registerListKeys: (keys: ContactsListKeys | null) => void;
}

const Context = createContext<ContactsUi | null>(null);

export const ContactsUiProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ContactsFilter>("all");
  const [creating, setCreating] = useState<"individual" | "group" | null>(null);
  const [books, setBooks] = useState(false);
  const listKeys = useRef<ContactsListKeys | null>(null);
  const value = useMemo(
    () => ({
      registerListKeys: (keys: ContactsListKeys | null) => {
        listKeys.current = keys;
      },
      books,
      creating,
      filter,
      listKeys,
      query,
      setBooks,
      setCreating,
      setFilter,
      setQuery,
    }),
    [books, creating, filter, query]
  );
  return <Context value={value}>{children}</Context>;
};

export const useContactsUi = (): ContactsUi => {
  const value = use(Context);
  if (value === null) {
    throw new Error("useContactsUi outside ContactsUiProvider");
  }
  return value;
};
