// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Style fragments shared by the mail screens (Helix tokens only).

import { colors } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

export const shared = stylex.create({
  ellipsis: {
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  focusRing: {
    outlineColor: { ":focus-visible": colors.focus, default: "transparent" },
    outlineOffset: "2px",
    outlineStyle: { ":focus-visible": "solid", default: "none" },
    outlineWidth: "2px",
  },
  resetButton: {
    appearance: "none",
    backgroundColor: "transparent",
    borderStyle: "none",
    borderWidth: 0,
    color: "inherit",
    cursor: "pointer",
    fontFamily: "inherit",
    fontSize: "inherit",
    lineHeight: "inherit",
    margin: 0,
    padding: 0,
    textAlign: "start",
  },
  resetList: { listStyle: "none", margin: 0, padding: 0 },
  srOnly: {
    borderWidth: 0,
    clip: "rect(0 0 0 0)",
    height: "1px",
    margin: "-1px",
    overflow: "hidden",
    padding: 0,
    position: "absolute",
    whiteSpace: "nowrap",
    width: "1px",
  },
});
