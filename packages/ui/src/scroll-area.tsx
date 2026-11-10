// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { ScrollArea as BaseScrollArea } from "@base-ui/react/scroll-area";
import * as stylex from "@stylexjs/stylex";
import type { StyleXStyles } from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { cls } from "./internal";
import { colors, motion, radius } from "./tokens.stylex";

const styles = stylex.create({
  root: { minHeight: 0, position: "relative" },
  scrollbar: {
    display: "flex",
    justifyContent: "center",
    margin: "3px",
    opacity: 0,
    transitionDuration: motion.normal,
    transitionProperty: "opacity",
    width: "8px",
  },
  scrollbarVisible: { opacity: 1 },
  thumb: {
    backgroundColor: { ":hover": colors.fg3, default: colors.line2 },
    borderRadius: radius.pill,
    width: "100%",
  },
  viewport: {
    // Native scrollbar hidden here (UIProvider disables Base UI's inline
    // <style> that would do it, to keep the CSP strict).
    "::-webkit-scrollbar": { display: "none" },
    height: "100%",
    outline: "none",
    overscrollBehavior: "contain",
    scrollbarWidth: "none",
  },
});

export interface ScrollAreaProps {
  readonly children: ReactNode;
  /** Sizing escape hatch (height, max-height, flex). */
  readonly xstyle?: StyleXStyles;
}

/** Scroll container with a thin overlay scrollbar that shows on hover/scroll. */
export const ScrollArea = ({ children, xstyle }: ScrollAreaProps) => (
  <BaseScrollArea.Root className={cls(styles.root, xstyle)}>
    <BaseScrollArea.Viewport className={cls(styles.viewport)}>
      {children}
    </BaseScrollArea.Viewport>
    <BaseScrollArea.Scrollbar
      className={(state) =>
        cls(
          styles.scrollbar,
          state.hovering || state.scrolling ? styles.scrollbarVisible : null
        )
      }
    >
      <BaseScrollArea.Thumb className={cls(styles.thumb)} />
    </BaseScrollArea.Scrollbar>
  </BaseScrollArea.Root>
);
