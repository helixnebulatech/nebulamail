// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";

/** Breakpoints of the mail layout (responsive from the start). */
export const bp = stylex.defineConsts({
  /** Phones: one column, sidebar as a drawer, floating compose button. */
  mobile: "@media (max-width: 640px)",
  /**
   * Wide enough for list and reader, too tight for a full sidebar as well:
   * the sidebar shows as its icon rail (shell.tsx), whatever was chosen.
   */
  compact: "@media (min-width: 1001px) and (max-width: 1200px)",
  /** One panel at a time (list or reader); the app rail hides. */
  narrow: "@media (max-width: 1000px)",
  /**
   * Tablets: one panel next to the sidebar, with 44 px touch targets, so
   * toolbars have about as much room as on a phone.
   */
  narrowTouch: "@media (max-width: 1000px) and (pointer: coarse)",
});
