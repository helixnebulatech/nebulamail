// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Checkbox as BaseCheckbox } from "@base-ui/react/checkbox";
import * as stylex from "@stylexjs/stylex";
import { Check, Minus } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";

import { cls, shared } from "./internal";
import { colors, font, motion, touch } from "./tokens.stylex";

const styles = stylex.create({
  box: {
    // The box stays 16 px; on touch an invisible ring makes the hit area
    // `touch.target` (44 px) wide without moving the layout.
    "::after": {
      content: { default: null, [touch.coarse]: '""' },
      // From the padding box (inside the 1.5 px border): 13 + 2 × 15.5 = 44.
      inset: "-15.5px",
      position: "absolute",
    },
    position: "relative",
    alignItems: "center",
    // Transparent: the box sits on rows, menus and selected fills alike.
    backgroundColor: "transparent",
    borderColor: { ":hover": colors.fg2, default: colors.control },
    borderRadius: "5px",
    borderStyle: "solid",
    borderWidth: "1.5px",
    boxSizing: "border-box",
    color: "#FFFFFF",
    cursor: "pointer",
    display: "inline-grid",
    flexShrink: 0,
    height: "16px",
    justifyContent: "center",
    transitionDuration: motion.fast,
    transitionProperty: "background-color, border-color",
    transitionTimingFunction: motion.easing,
    width: "16px",
  },
  checked: {
    backgroundColor: colors.acc,
    borderColor: { ":hover": colors.acc, default: colors.acc },
  },
  // A square box in a colour of its own (a calendar's): the ring and the
  // fill take it.
  squareTone: {
    borderColor: {
      ":hover": "var(--checkbox-tone)",
      default: "var(--checkbox-tone)",
    },
  },
  squareToneChecked: {
    backgroundColor: "var(--checkbox-tone)",
    borderColor: {
      ":hover": "var(--checkbox-tone)",
      default: "var(--checkbox-tone)",
    },
  },
  // A task's tick: round, a bit bigger, the colour of its priority.
  round: {
    "::after": {
      content: { default: null, [touch.coarse]: '""' },
      inset: "-12.5px",
      position: "absolute",
    },
    borderColor: {
      ":hover": "var(--checkbox-tone, currentColor)",
      default: `var(--checkbox-tone, ${colors.control})`,
    },
    borderRadius: "50%",
    borderWidth: "1.5px",
    height: "18px",
    width: "18px",
  },
  roundChecked: {
    backgroundColor: `var(--checkbox-tone, ${colors.acc})`,
    borderColor: {
      ":hover": `var(--checkbox-tone, ${colors.acc})`,
      default: `var(--checkbox-tone, ${colors.acc})`,
    },
  },
  disabled: { cursor: "not-allowed", opacity: 0.45 },
  indicator: { display: "inline-grid" },
  label: {
    alignItems: "center",
    columnGap: "8px",
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    // A long label wraps inside its container instead of pushing it wider.
    maxWidth: "100%",
    minWidth: 0,
  },
  labelDisabled: { color: colors.fg2, cursor: "not-allowed" },
});

type BaseProps = Omit<
  ComponentProps<typeof BaseCheckbox.Root>,
  "className" | "style" | "children"
>;

export interface CheckboxProps extends BaseProps {
  /** Visible label. Without it, pass `aria-label`. */
  readonly label?: ReactNode;
  /** "round": a task's tick (to-do lists); "square" by default. */
  readonly shape?: "square" | "round";
  /** CSS colour of the ring and fill (a token or hex), e.g. a calendar's. */
  readonly tone?: string | undefined;
}

const toneVars = stylex.create({
  tone: (tone: string) => ({ "--checkbox-tone": tone }),
});

/** Checkbox with optional label; supports `indeterminate` (mixed). */
export const Checkbox = ({
  label,
  disabled = false,
  shape = "square",
  tone,
  ...props
}: CheckboxProps) => {
  const round = shape === "round";
  const tinted = tone === undefined ? null : toneVars.tone(tone);
  const squareTinted = !round && tinted !== null;
  let checkedStyle = round ? styles.roundChecked : styles.checked;
  if (squareTinted) {
    checkedStyle = styles.squareToneChecked;
  }
  const box = (
    <BaseCheckbox.Root
      {...props}
      className={(state) =>
        cls(
          styles.box,
          round ? styles.round : null,
          squareTinted ? styles.squareTone : null,
          state.checked || state.indeterminate ? checkedStyle : null,
          tinted,
          shared.focusRing,
          state.disabled ? styles.disabled : null
        )
      }
      disabled={disabled}
      {...(tinted === null ? {} : { style: stylex.props(tinted).style })}
    >
      <BaseCheckbox.Indicator
        className={cls(styles.indicator)}
        render={(indicatorProps, state) => (
          <span {...indicatorProps}>
            {state.indeterminate ? (
              <Minus aria-hidden size={12} strokeWidth={3.5} />
            ) : (
              <Check aria-hidden size={12} strokeWidth={3.5} />
            )}
          </span>
        )}
      />
    </BaseCheckbox.Root>
  );
  return label === undefined ? (
    box
  ) : (
    // oxlint-disable-next-line jsx-a11y/label-has-associated-control -- Base UI's checkbox is a role="checkbox" span; wrapping in <label> is its documented pattern.
    <label
      {...stylex.props(styles.label, disabled ? styles.labelDisabled : null)}
    >
      {box}
      {label}
    </label>
  );
};
