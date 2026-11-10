// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Any string, written so that `"known" | OtherString` still offers the
 * known literals in editor autocompletion (plain `string` would absorb them).
 */
export type OtherString = string & Record<never, never>;
