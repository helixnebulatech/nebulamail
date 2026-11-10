// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Route loaders that only warm React Query's cache: the
// data starts loading with the route's code (and on hover/focus of a link,
// `defaultPreload: "intent"`), not after the screen mounts. The screens
// still read it through their own hooks, with the same query options.

import type { QueryClient } from "@tanstack/react-query";

import { calendarsQuery, meQuery, ownAddressesQuery } from "../calendar/hooks";
import { browserTimeZone } from "../calendar/time";
import { addressBooksQuery, contactListQuery } from "../contacts/hooks";
import type { MailAccount } from "../mail/account-scope";
import { mailboxesQuery } from "../mail/queries";
import { tasksQuery, todayIn } from "../tasks/hooks";
import { readSavedPreferences } from "./preferences";

/**
 * Browser only (the session lives there; the server never fetches mail)
 * and never awaited: navigation doesn't wait, and a failure is left to the
 * screen's query (its own retry, error state and SessionWatch).
 */
export const inBrowser = (): boolean => typeof document !== "undefined";

/** The folders of the sidebar (own account) and of `account` if shared. */
export const prefetchMailboxes = (
  queryClient: QueryClient,
  account: MailAccount = null
): void => {
  if (!inBrowser()) {
    return;
  }
  queryClient.query(mailboxesQuery()).catch(() => null);
  if (account !== null) {
    queryClient.query(mailboxesQuery(account)).catch(() => null);
  }
};

/** Every task (tasks/hooks.ts `useTasks`, same zone and day). */
export const prefetchTasks = (queryClient: QueryClient): void => {
  if (!inBrowser()) {
    return;
  }
  const zone = browserTimeZone();
  queryClient.query(tasksQuery(zone, todayIn(zone))).catch(() => null);
};

/**
 * The calendar's first screen: its calendars, the user's addresses and
 * principal, and the tasks if it shows them (preferences cookie). The week's occurrences wait for these (calendar/hooks.ts
 * `useOccurrences`), so starting them with the route takes most of that
 * chain off the first paint; the occurrences' key depends on the view and
 * date the screen picks, so they start from there.
 */
export const prefetchCalendar = (queryClient: QueryClient): void => {
  if (!inBrowser()) {
    return;
  }
  queryClient.query(calendarsQuery()).catch(() => null);
  queryClient.query(ownAddressesQuery()).catch(() => null);
  queryClient.query(meQuery()).catch(() => null);
  if (readSavedPreferences().calendarTasks === "shown") {
    prefetchTasks(queryClient);
  }
};

/** The contact list and its address books. */
export const prefetchContacts = (queryClient: QueryClient): void => {
  if (!inBrowser()) {
    return;
  }
  queryClient.query(contactListQuery()).catch(() => null);
  queryClient.query(addressBooksQuery()).catch(() => null);
};
