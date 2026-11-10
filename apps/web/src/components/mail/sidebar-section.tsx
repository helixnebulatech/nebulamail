// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The sidebar section heading of every module ("Folders", "Shared with
// me", "Address books", "My calendars", "My lists"), on the nav items'
// grid: the chevron (foldable sections) in their 16px icon column
// (x = 22), the text in their label column (x = 48), and an optional
// action ("+", "⋯") at the end of the row. Secondary text under a heading
// (a sharer's address) uses `sidebarText`, on the same label column.

import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { ReactNode } from "react";

import { shared } from "./styles";

/** NavItem's inline padding, icon box and gap (packages/ui nav-item.tsx). */
const ICON_START = "10px";
const LABEL_START = "36px";

export const sidebarText = stylex.create({
  text: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    margin: 0,
    paddingBlock: "4px",
    paddingInlineEnd: "10px",
    paddingInlineStart: LABEL_START,
  },
});

const styles = stylex.create({
  row: {
    alignItems: "center",
    columnGap: "2px",
    display: "flex",
    marginTop: "12px",
  },
  // In sidebars laid out with a 16px row gap (calendar, tasks): the gap
  // already spaces it, and 2px to the rows under it, as mail's row gap.
  flush: { marginBottom: "2px", marginTop: 0 },
  action: {
    alignItems: "center",
    columnGap: "2px",
    display: "flex",
    flexShrink: 0,
  },
  head: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    color: { ":hover": colors.fg2, default: colors.fg3 },
    columnGap: "10px",
    display: "flex",
    flexGrow: 1,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    height: "26px",
    minWidth: 0,
    paddingInline: ICON_START,
    textAlign: "start",
  },
  static: {
    backgroundColor: { ":hover": "transparent", default: "transparent" },
    color: { ":hover": colors.fg3, default: colors.fg3 },
    margin: 0,
  },
  icon: {
    display: "grid",
    flexShrink: 0,
    placeItems: "center",
    width: "16px",
  },
});

export const SidebarSection = ({
  action,
  children,
  flush = false,
  onToggle,
  open,
}: {
  readonly open: boolean;
  /** null: a plain heading that doesn't fold. */
  readonly onToggle: (() => void) | null;
  /** The heading's text. */
  readonly children: ReactNode;
  /** Buttons at the end of the row ("+" to add, "⋯" for more). */
  readonly action?: ReactNode;
  /** In a sidebar that spaces its blocks with a row gap (calendar, tasks). */
  readonly flush?: boolean;
}) => (
  <div {...stylex.props(styles.row, flush ? styles.flush : null)}>
    {onToggle === null ? (
      <h2 {...stylex.props(styles.head, styles.static)}>
        <span aria-hidden {...stylex.props(styles.icon)} />
        <span {...stylex.props(shared.ellipsis)}>{children}</span>
      </h2>
    ) : (
      <button
        aria-expanded={open}
        onClick={onToggle}
        type="button"
        {...stylex.props(shared.resetButton, styles.head, shared.focusRing)}
      >
        <span aria-hidden {...stylex.props(styles.icon)}>
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </span>
        <span {...stylex.props(shared.ellipsis)}>{children}</span>
      </button>
    )}
    {action === undefined ? null : (
      <span {...stylex.props(styles.action)}>{action}</span>
    )}
  </div>
);
