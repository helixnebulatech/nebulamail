// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What a list row shows of a message's preview.

/** Separators left at the start once the subject is cut off the preview. */
const LEADING_SEPARATORS = /^[\s:;,.·|\-–—]+/u;
/** The subject must end at a word boundary ("Hello" isn't cut off "Hellos"). */
const BOUNDARY = /^[\s:;,.·|\-–—]/u;

/**
 * The preview without the subject when it starts with it (invitations and
 * many notifications repeat it as their first line): the row shows it
 * once.
 */
export const previewWithoutSubject = (
  preview: string,
  subject: string
): string => {
  const head = subject.trim();
  if (
    head === "" ||
    preview.length <= head.length ||
    preview.slice(0, head.length).toLocaleLowerCase() !==
      head.toLocaleLowerCase() ||
    !BOUNDARY.test(preview.slice(head.length))
  ) {
    return preview;
  }
  return preview.slice(head.length).replace(LEADING_SEPARATORS, "");
};
