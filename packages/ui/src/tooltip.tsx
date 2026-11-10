// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";
import * as stylex from "@stylexjs/stylex";
import type { ReactElement, ReactNode } from "react";

import { cls, usePortalContainer } from "./internal";
import { Shortcut } from "./kbd";
import { colors, font, motion, radius, zIndex } from "./tokens.stylex";

const styles = stylex.create({
  arrow: {
    backgroundColor: colors.ink,
    borderRadius: "1px",
    height: "8px",
    transform: "rotate(45deg)",
    width: "8px",
  },
  arrowBottom: { top: "-4px" },
  arrowLeft: { insetInlineEnd: "-4px" },
  arrowRight: { insetInlineStart: "-4px" },
  arrowTop: { bottom: "-4px" },
  hidden: { opacity: 0, transform: "scale(0.96)" },
  popup: {
    alignItems: "center",
    backgroundColor: colors.ink,
    borderRadius: radius.md,
    color: colors.inkFg,
    columnGap: "6px",
    display: "flex",
    fontFamily: font.sans,
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
    lineHeight: 1.3,
    maxWidth: "280px",
    paddingBlock: "5px",
    paddingInline: "9px",
    transformOrigin: "var(--transform-origin)",
    transitionDuration: motion.fast,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
  },
  positioner: { zIndex: zIndex.tooltip },
});

const arrowSide = {
  bottom: styles.arrowBottom,
  "inline-end": styles.arrowLeft,
  "inline-start": styles.arrowRight,
  left: styles.arrowLeft,
  right: styles.arrowRight,
  top: styles.arrowTop,
} as const;

export interface TooltipProps {
  /** The trigger: one element that accepts props and a ref (e.g. a Button). */
  readonly children: ReactElement;
  readonly content: ReactNode;
  /** Keys shown after the text, e.g. `["E"]`. */
  readonly shortcut?: readonly string[];
  readonly side?: "top" | "bottom" | "left" | "right";
  /** Force open (catalog, onboarding). */
  readonly open?: boolean;
}

/** Ink tooltip with an optional shortcut. Shown on hover and keyboard focus. */
export const Tooltip = ({
  children,
  content,
  shortcut,
  side = "top",
  open,
}: TooltipProps) => {
  const container = usePortalContainer();
  return (
    <BaseTooltip.Root {...(open === undefined ? {} : { open })}>
      <BaseTooltip.Trigger delay={400} render={children} />
      <BaseTooltip.Portal container={container}>
        <BaseTooltip.Positioner
          className={cls(styles.positioner)}
          side={side}
          sideOffset={8}
        >
          <BaseTooltip.Popup
            className={(state) =>
              cls(
                styles.popup,
                state.transitionStatus === "starting" ||
                  state.transitionStatus === "ending"
                  ? styles.hidden
                  : null
              )
            }
          >
            {content}
            {shortcut === undefined ? null : (
              <Shortcut keys={shortcut} tone="onInk" />
            )}
            <BaseTooltip.Arrow
              className={(state) => cls(styles.arrow, arrowSide[state.side])}
            />
          </BaseTooltip.Popup>
        </BaseTooltip.Positioner>
      </BaseTooltip.Portal>
    </BaseTooltip.Root>
  );
};
