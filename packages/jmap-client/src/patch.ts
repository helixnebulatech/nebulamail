// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Helpers to build `Email/set` patches. They escape `~` and `/` in keywords
// and ids (RFC 6901), which hand-written `"keywords/" + k` strings forget.

import type { MailboxId } from "./ids";
import type { EmailPatch } from "./mail";
import { escapePointerSegment } from "./url";

/**
 * `{ $seen: true, $flagged: false }` → `{ "keywords/$seen": true,
 * "keywords/$flagged": null }`.
 */
export const keywordPatch = (
  keywords: Readonly<Record<string, boolean>>
): EmailPatch =>
  Object.fromEntries(
    Object.entries(keywords).map(([keyword, on]) => [
      `keywords/${escapePointerSegment(keyword)}`,
      on ? (true as const) : null,
    ])
  );

/** Adds/removes mailboxes without replacing the whole `mailboxIds` map. */
export const mailboxPatch = (change: {
  readonly add?: readonly MailboxId[];
  readonly remove?: readonly MailboxId[];
}): EmailPatch =>
  Object.fromEntries([
    ...(change.remove ?? []).map(
      (id) => [`mailboxIds/${escapePointerSegment(id)}`, null] as const
    ),
    ...(change.add ?? []).map(
      (id) => [`mailboxIds/${escapePointerSegment(id)}`, true] as const
    ),
  ]);
