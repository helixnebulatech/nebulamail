// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The message list while its first page loads: placeholder rows with the
// real rows' height and gutters (email-row.tsx), so nothing jumps when the
// mail arrives. Screen readers hear one "Loading messages…" status.

import { colors } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import { m } from "../../paraglide/messages.js";
import { shared } from "./styles";

const REDUCED = "@media (prefers-reduced-motion: reduce)";

const pulse = stylex.keyframes({
  "0%": { opacity: 1 },
  "50%": { opacity: 0.55 },
  "100%": { opacity: 1 },
});

/** Text bar widths, varied so the rows don't read as a grid. */
const WIDTHS = [
  [38, 72],
  [30, 84],
  [44, 64],
  [34, 78],
  [40, 70],
  [28, 88],
] as const;

const ROWS = 12;

const styles = stylex.create({
  root: {
    animationDuration: "1.6s",
    animationIterationCount: "infinite",
    animationName: { default: pulse, [REDUCED]: "none" },
    animationTimingFunction: "ease-in-out",
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    overflow: "hidden",
    // As the real list (list-panel.tsx `rows`/`virtual`).
    paddingBlock: "4px",
    paddingInline: "6px",
    rowGap: "2px",
  },
  row: {
    alignItems: "center",
    columnGap: "12px",
    display: "flex",
    flexShrink: 0,
    // As email-row.tsx's `link` (GUTTER at the start).
    paddingInlineEnd: "16px",
    paddingInlineStart: "20px",
  },
  compact: { height: "56px" },
  comfortable: { alignItems: "flex-start", height: "82px", paddingTop: "12px" },
  avatar: {
    backgroundColor: colors.hover,
    // As Avatar's (size × 0.34).
    borderRadius: "11px",
    flexShrink: 0,
    height: "32px",
    width: "32px",
  },
  avatarCompact: { borderRadius: "10px", height: "28px", width: "28px" },
  lines: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
    rowGap: "8px",
  },
  bar: (width: number) => ({
    backgroundColor: colors.hover,
    borderRadius: "5px",
    height: "10px",
    width: `${width}%`,
  }),
});

export const ListSkeleton = ({ compact }: { readonly compact: boolean }) => (
  <output {...stylex.props(styles.root)}>
    <span {...stylex.props(shared.srOnly)}>{m.list_loading()}</span>
    {Array.from({ length: ROWS }, (_, index) => {
      const [first, second] = WIDTHS[index % WIDTHS.length] ?? WIDTHS[0];
      return (
        <div
          aria-hidden
          key={index}
          {...stylex.props(
            styles.row,
            compact ? styles.compact : styles.comfortable
          )}
        >
          <span
            {...stylex.props(
              styles.avatar,
              compact ? styles.avatarCompact : null
            )}
          />
          <span {...stylex.props(styles.lines)}>
            <span {...stylex.props(styles.bar(first))} />
            <span {...stylex.props(styles.bar(second))} />
            {compact ? null : (
              <span {...stylex.props(styles.bar(second - 14))} />
            )}
          </span>
        </div>
      );
    })}
  </output>
);
