// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// UI state shared by the mail shell: the command palette, the shortcut help,
// the mobile drawer, the search box and the session's privacy counter.
// Per React tree (not module state): the server renders many users.

import { createContext, use, useState } from "react";
import type { ReactNode } from "react";

export interface MailUi {
  readonly paletteOpen: boolean;
  readonly setPaletteOpen: (open: boolean) => void;
  readonly helpOpen: boolean;
  readonly setHelpOpen: (open: boolean) => void;
  readonly drawerOpen: boolean;
  readonly setDrawerOpen: (open: boolean) => void;
  /** Trackers blocked in the emails opened this session. */
  readonly blocked: number;
  /** Counts an email's blocked trackers once, however often it's opened. */
  readonly reportBlocked: (emailId: string, count: number) => void;
  /**
   * Callback ref for a search input. "/" focuses the newest one still on
   * the page (the contacts page has its own over the mail search).
   */
  readonly registerSearch: (input: HTMLInputElement | null) => void;
  readonly focusSearch: () => void;
  readonly blurSearch: () => void;
}

const MailUiContext = createContext<MailUi | null>(null);

export const MailUiProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [counted, setCounted] = useState<ReadonlyMap<string, number>>(
    new Map()
  );
  // State, not a ref: it is handed out as a callback ref through context.
  const [searchInputs, setSearchInputs] = useState<readonly HTMLInputElement[]>(
    []
  );
  const registerSearch = (input: HTMLInputElement | null): void => {
    if (input !== null) {
      setSearchInputs((current) =>
        current.includes(input)
          ? current
          : [...current.filter((other) => other.isConnected), input]
      );
    }
  };
  /** Read in event handlers only (`isConnected` is DOM state). */
  const activeSearch = (): HTMLInputElement | null =>
    searchInputs.findLast((input) => input.isConnected) ?? null;

  const reportBlocked = (emailId: string, count: number): void => {
    setCounted((current) =>
      current.get(emailId) === count || count === 0
        ? current
        : new Map([...current, [emailId, count]])
    );
  };

  const blurSearch = (): void => {
    activeSearch()?.blur();
  };

  const focusSearch = (): void => {
    setDrawerOpen(false);
    const input = activeSearch();
    input?.focus();
    input?.select();
  };

  let blocked = 0;
  for (const count of counted.values()) {
    blocked += count;
  }

  return (
    <MailUiContext
      // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object and its functions.
      value={{
        blocked,
        blurSearch,
        drawerOpen,
        focusSearch,
        helpOpen,
        paletteOpen,
        registerSearch,
        reportBlocked,
        setDrawerOpen,
        setHelpOpen,
        setPaletteOpen,
      }}
    >
      {children}
    </MailUiContext>
  );
};

export const useMailUi = (): MailUi => {
  const value = use(MailUiContext);
  if (value === null) {
    throw new Error("useMailUi must be used inside MailUiProvider");
  }
  return value;
};

/**
 * Shortcuts must not act on the mail behind an open dialog, menu or the
 * composer (focus is there). Used as a guard inside handlers.
 */
export const isInOverlay = (target: EventTarget | null): boolean =>
  target instanceof Element &&
  target.closest(
    '[role="dialog"], [role="alertdialog"], [role="menu"], [data-composer]'
  ) !== null;
