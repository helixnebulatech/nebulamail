// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { cls, shared } from "./internal";
import { colors, font, motion, radius, shadow } from "./tokens.stylex";

const styles = stylex.create({
  checked: {
    backgroundColor: colors.activeFill,
    boxShadow: shadow.active,
    color: { ":hover": colors.fg, default: colors.fg },
    fontWeight: font.weightSemibold,
  },
  group: {
    // As wide as its options, also in a column that stretches its children
    // (a width keeps it from stretching; alignSelf would move it in a row).
    width: "fit-content",
    backgroundColor: colors.canvas,
    borderColor: colors.line,
    borderRadius: radius.pill,
    borderStyle: "solid",
    borderWidth: "1px",
    display: "inline-flex",
    flexShrink: 0,
    padding: "2px",
  },
  item: {
    alignItems: "center",
    borderRadius: radius.pill,
    color: { ":hover": colors.fg, default: colors.fg2 },
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: font.sans,
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
    height: "26px",
    paddingInline: "11px",
    transitionDuration: motion.fast,
    transitionProperty: "background-color, color, box-shadow",
    transitionTimingFunction: motion.easing,
    whiteSpace: "nowrap",
  },
  disabled: { cursor: "not-allowed", opacity: 0.45 },
  // 28px in all, the height of a small Button next to it (calendar bar).
  itemSm: { height: "22px", paddingInline: "10px" },
});

export interface SegmentedOption<Value extends string> {
  readonly value: Value;
  readonly label: ReactNode;
  readonly disabled?: boolean;
}

export interface SegmentedControlProps<Value extends string> {
  readonly options: readonly SegmentedOption<Value>[];
  readonly value?: Value;
  readonly defaultValue?: Value;
  readonly onValueChange?: (value: Value) => void;
  /** Accessible name of the group (e.g. "Filtro rápido"). */
  readonly label: string;
  /** "sm": as tall as a small Button (28px). */
  readonly size?: "sm" | "md";
}

/** Pill filter (Todos / No leídos / Leídos / Adjuntos). A radio group: arrows move. */
export const SegmentedControl = <Value extends string>({
  options,
  value,
  defaultValue,
  onValueChange,
  label,
  size = "md",
}: SegmentedControlProps<Value>) => (
  <RadioGroup<Value>
    aria-label={label}
    className={cls(styles.group)}
    {...(value === undefined ? {} : { value })}
    {...(defaultValue === undefined ? {} : { defaultValue })}
    {...(onValueChange === undefined
      ? {}
      : {
          onValueChange: (next: Value) => {
            onValueChange(next);
          },
        })}
  >
    {options.map((option) => (
      <Radio.Root
        className={(state) =>
          cls(
            shared.resetButton,
            styles.item,
            size === "sm" ? styles.itemSm : null,
            state.checked ? styles.checked : null,
            shared.focusRing,
            state.disabled ? styles.disabled : null
          )
        }
        disabled={option.disabled ?? false}
        key={option.value}
        value={option.value}
      >
        {option.label}
      </Radio.Root>
    ))}
  </RadioGroup>
);
