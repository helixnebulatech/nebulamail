// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The reader's styles used by more than one of its files (reader.tsx,
// email-body.tsx, reader-cards.tsx, reader-conversation.tsx,
// reader-toolbar.tsx).

import { colors, font, radius, shadow } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import { bp } from "./breakpoints.stylex";

export const readerStyles = stylex.create({
  // An open message: its own card, so the header, the privacy line and
  // the body read as one message. On phones the screen is the card.
  card: {
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: { default: "1px", [bp.mobile]: 0 },
    boxShadow: { default: shadow.sh1, [bp.mobile]: "none" },
    paddingBottom: "14px",
    paddingInline: { default: "18px", [bp.mobile]: 0 },
    paddingTop: "16px",
  },
  center: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
  },
  notice: {
    backgroundColor: colors.warnSoft,
    borderRadius: radius.md,
    color: colors.fg,
    fontSize: font.sizeSm,
    margin: 0,
    marginTop: "12px",
    paddingBlock: "6px",
    paddingInline: "10px",
  },
  panel: { flexGrow: 1, minWidth: 0 },
  panelMobile: {
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderWidth: { default: "1px", [bp.mobile]: 0 },
  },
  toolbar: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "2px",
    display: "flex",
    flexShrink: 0,
    paddingBlock: "8px",
    paddingInline: "10px",
  },
});
