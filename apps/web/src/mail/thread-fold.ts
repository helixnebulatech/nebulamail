// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Long runs of folded messages in the reader fold their middle into one
// "N more messages" row (components/mail/reader-conversation.tsx).

import type { EmailSummary } from "./api";

/** One row of the conversation: an email, or a fold of several. */
export type Entry<T extends { readonly id: string } = EmailSummary> =
  | { readonly kind: "email"; readonly email: T }
  | { readonly kind: "more"; readonly count: number; readonly from: T["id"] };

/** Runs of folded emails at least this long fold their middle. */
const FOLD_RUN = 4;

/**
 * The rows to show: each run of {@link FOLD_RUN} or more folded emails
 * keeps its first and last, with one "N more messages" row between them
 * (Gmail, Superhuman). Unless `all`.
 */
export const foldLongRuns = <T extends { readonly id: string }>(
  emails: readonly T[],
  foldable: (email: T) => boolean,
  all: boolean
): Entry<T>[] => {
  const entries: Entry<T>[] = [];
  let run: T[] = [];
  const flush = (): void => {
    const [head] = run;
    const tail = run.at(-1);
    if (!all && run.length >= FOLD_RUN && head !== undefined && tail) {
      entries.push(
        { email: head, kind: "email" },
        { count: run.length - 2, from: head.id, kind: "more" },
        { email: tail, kind: "email" }
      );
    } else {
      for (const email of run) {
        entries.push({ email, kind: "email" });
      }
    }
    run = [];
  };
  for (const email of emails) {
    if (foldable(email)) {
      run.push(email);
    } else {
      flush();
      entries.push({ email, kind: "email" });
    }
  }
  flush();
  return entries;
};
