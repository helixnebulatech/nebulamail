// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the email list shows: loading, an error, "no preview" for a search
// the server can't run, or the rows.

import { isMethodError } from "@nebulamail/jmap-client";

import type { MailboxView } from "./views";

export type ListStatus = "pending" | "error" | "unsupported" | "success";

export interface ListStatusInput {
  readonly view: MailboxView | null;
  readonly mailboxesLoaded: boolean;
  /**
   * The folders query gave up (its retries failed). A folder list waits
   * for the folders, so without this it would stay "pending" for good.
   */
  readonly mailboxesFailed: boolean;
  /** The search text ("" for a folder). */
  readonly query: string;
  readonly queryStatus: "pending" | "error" | "success";
  readonly error: unknown;
}

/**
 * `unsupportedFilter` on a search (too long a text, too many terms: the
 * server's limits, not copied here) is "unsupported": retrying won't help.
 * On a folder list the filter is ours, so it stays a plain error (a bug).
 * Also when a later page fails: the status is the query's.
 */
export const listStatusOf = ({
  error,
  mailboxesFailed,
  mailboxesLoaded,
  query,
  queryStatus,
  view,
}: ListStatusInput): ListStatus => {
  if (query === "") {
    if (view === null && mailboxesLoaded) {
      return "error";
    }
    // 2026-11-08: the first request
    // failed and «Loading messages…» stayed, with nothing more asked.
    return mailboxesFailed && queryStatus === "pending" ? "error" : queryStatus;
  }
  return queryStatus === "error" && isMethodError(error, "unsupportedFilter")
    ? "unsupported"
    : queryStatus;
};
