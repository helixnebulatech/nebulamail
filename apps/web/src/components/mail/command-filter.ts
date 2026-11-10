// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The ⌘K palette's filter: every typed word must appear in the command's
// label or keywords, ignoring case and accents ("arch" finds "Archivar",
// "leido" finds "Marcar como leído").

const normalize = (text: string): string =>
  text
    .normalize("NFD")
    .replaceAll(/\p{Diacritic}/gu, "")
    .toLowerCase();

export const matchesQuery = (
  command: { readonly label: string; readonly keywords?: string },
  query: string
): boolean => {
  const haystack = normalize(`${command.label} ${command.keywords ?? ""}`);
  return normalize(query)
    .split(/\s+/u)
    .filter((word) => word !== "")
    .every((word) => haystack.includes(word));
};
