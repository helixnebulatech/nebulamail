// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the server refused when saving or sending a message, in words the
// composer can show. Stalwart 1.0 added limits per email (configurable):
// 100 mailboxes (`tooManyMailboxes`, `maxMailboxesPerEmail`), 100 keywords
// (`tooManyKeywords`), and headers (`tooLarge`).

import { JmapError } from "@nebulamail/jmap-client";
import type { SetError } from "@nebulamail/jmap-client";

import { SendFailedError } from "./send-failed";

/**
 * Why the server refused a message, when the user can do something about
 * it: too big (Email/set `tooLarge`, the request over a size limit), or
 * over Stalwart 1.0's limits of mailboxes or keywords per email (100 each
 * by default, `maxMailboxesPerEmail`).
 */
export type ServerRefusal = "tooLarge" | "tooManyKeywords" | "tooManyMailboxes";

/** The server refused to create the draft (`notCreated`). */
export class DraftRefusedError extends Error {
  readonly type: string;

  constructor(type: string, description: string | null | undefined) {
    super(description ?? type);
    this.name = "DraftRefusedError";
    this.type = type;
  }
}

const REFUSALS: Readonly<Record<string, ServerRefusal>> = {
  requestTooLarge: "tooLarge",
  tooLarge: "tooLarge",
  tooManyKeywords: "tooManyKeywords",
  tooManyMailboxes: "tooManyMailboxes",
};

/** What to tell the user about a failed save or send, if anything specific. */
export const serverRefusalOf = (error: unknown): ServerRefusal | null => {
  if (error instanceof SendFailedError) {
    return serverRefusalOf(error.cause);
  }
  if (error instanceof DraftRefusedError) {
    return REFUSALS[error.type] ?? null;
  }
  if (!(error instanceof JmapError)) {
    return null;
  }
  const { detail } = error;
  if (detail.kind === "method") {
    return REFUSALS[detail.type] ?? null;
  }
  const overLimit =
    (detail.kind === "limit" && detail.limit === "maxSizeRequest") ||
    (detail.kind === "request" &&
      detail.type === "urn:ietf:params:jmap:error:limit" &&
      // Too many at once says nothing about this email's size.
      detail.limit !== "maxConcurrentRequests");
  return overLimit ? "tooLarge" : null;
};

/** The refusal of the `draft` creation in a `notCreated`, if any. */
export const draftRefusal = (
  notCreated: Partial<Record<string, SetError>>
): DraftRefusedError | null => {
  const error = notCreated.draft;
  return error === undefined
    ? null
    : new DraftRefusedError(error.type, error.description);
};
