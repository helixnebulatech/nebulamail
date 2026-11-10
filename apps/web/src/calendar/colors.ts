// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Calendar colours. A calendar's colour comes from the server (and from
// whoever shares it), so only plain hex colours are used; anything else
// gets a colour from the palette, chosen from the id so it stays the same.

/** Helix-friendly colours for calendars (readable on light and dark). */
export const CALENDAR_PALETTE = [
  "#4F46E5",
  "#2353C9",
  "#0E7A4E",
  "#D97706",
  "#7C3AED",
  "#0F8C8C",
  "#DB2777",
  "#62637A",
] as const;

const HEX = /^#(?:[\da-f]{3}|[\da-f]{6})$/iu;

export const isSafeColor = (
  value: string | null | undefined
): value is string => typeof value === "string" && HEX.test(value);

const hash = (text: string): number => {
  let value = 0;
  for (const char of text) {
    value = (value * 31 + (char.codePointAt(0) ?? 0)) % 2_147_483_647;
  }
  return value;
};

export const calendarColor = (
  color: string | null | undefined,
  id: string
): string =>
  isSafeColor(color)
    ? color
    : (CALENDAR_PALETTE[hash(id) % CALENDAR_PALETTE.length] ?? "#4F46E5");
