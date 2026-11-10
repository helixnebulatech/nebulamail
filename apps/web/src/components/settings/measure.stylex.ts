// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";

/**
 * One reading width for every settings section, so headers, groups,
 * dividers, filter rows and their buttons all end on the same edge.
 */
export const settingsMeasure = stylex.defineConsts({
  width: "720px",
});
