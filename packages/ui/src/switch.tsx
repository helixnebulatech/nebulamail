// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Switch as BaseSwitch } from "@base-ui/react/switch";
import * as stylex from "@stylexjs/stylex";
import { Lock } from "lucide-react";
import { useId } from "react";
import type { ComponentProps, ReactNode } from "react";

import { cls, shared } from "./internal";
import { colors, font, motion, radius } from "./tokens.stylex";

const styles = stylex.create({
  checked: {
    backgroundColor: colors.acc,
  },
  // Unavailable reads as "locked", not as "off": faded, with a lock in
  // the thumb (the hint under the label says why).
  disabled: { cursor: "not-allowed", opacity: 0.45 },
  // The thumb is always white, so a fixed grey (not a theme colour).
  lock: { color: "#5E6075", display: "block" },
  thumbDisabled: {
    alignItems: "center",
    display: "flex",
    justifyContent: "center",
  },
  label: {
    alignItems: "center",
    columnGap: "10px",
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
  },
  // fg2, not fg3: the label stays readable so the hint under it can say why.
  labelDisabled: { color: colors.fg2, cursor: "not-allowed" },
  thumb: {
    backgroundColor: "#FFFFFF",
    borderRadius: "50%",
    boxShadow: "0 1px 2px rgba(0, 0, 0, 0.25)",
    display: "block",
    height: "16px",
    marginInlineStart: "3px",
    transitionDuration: motion.normal,
    transitionProperty: "margin-inline-start",
    transitionTimingFunction: motion.easingSpring,
    width: "16px",
  },
  thumbChecked: { marginInlineStart: "17px" },
  // Settings rows: label and description on the start side, the switch on
  // the end side, aligned with the label's first line.
  row: {
    alignItems: "start",
    columnGap: "24px",
    display: "grid",
    gridTemplateAreas: '"text control" "desc control"',
    gridTemplateColumns: "minmax(0, 1fr) auto",
    rowGap: "4px",
  },
  // `contents`: the label keeps its click-to-toggle and names the switch,
  // while its two children sit in the row's grid.
  rowLabel: { display: "contents" },
  rowText: {
    color: colors.fg,
    cursor: "pointer",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    gridArea: "text",
    lineHeight: "22px",
  },
  rowTextDisabled: { color: colors.fg2, cursor: "not-allowed" },
  rowControl: { gridArea: "control" },
  rowDesc: {
    color: colors.fg3,
    fontFamily: font.sans,
    fontSize: font.sizeSm,
    gridArea: "desc",
    lineHeight: 1.45,
  },
  track: {
    alignItems: "center",
    backgroundColor: colors.control,
    borderRadius: radius.pill,
    boxSizing: "border-box",
    cursor: "pointer",
    display: "inline-flex",
    flexShrink: 0,
    height: "22px",
    padding: 0,
    transitionDuration: motion.fast,
    transitionProperty: "background-color",
    transitionTimingFunction: motion.easing,
    width: "36px",
  },
});

type BaseProps = Omit<
  ComponentProps<typeof BaseSwitch.Root>,
  "className" | "style" | "children"
>;

export interface SwitchProps extends BaseProps {
  /** Visible label. Without it, pass `aria-label`. */
  readonly label?: ReactNode;
  /**
   * Text under the label (it may hold links). With it, or with
   * `layout="row"`, the switch is a settings row: label and description
   * on the start side, the switch on the end side.
   */
  readonly description?: ReactNode;
  readonly layout?: "inline" | "row";
}

/** On/off switch. RTL-safe (the thumb moves with margin-inline-start). */
export const Switch = ({
  label,
  description,
  layout = "inline",
  disabled = false,
  ...props
}: SwitchProps) => {
  const descriptionId = useId();
  const row =
    label !== undefined && (layout === "row" || description !== undefined);
  const control = (
    <BaseSwitch.Root
      {...props}
      {...(row && description !== undefined
        ? { "aria-describedby": descriptionId }
        : {})}
      className={(state) =>
        cls(
          styles.track,
          state.checked ? styles.checked : null,
          shared.focusRing,
          state.disabled ? styles.disabled : null,
          row ? styles.rowControl : null
        )
      }
      disabled={disabled}
    >
      <BaseSwitch.Thumb
        className={(state) =>
          cls(
            styles.thumb,
            state.checked ? styles.thumbChecked : null,
            state.disabled ? styles.thumbDisabled : null
          )
        }
      >
        {disabled ? (
          <Lock
            aria-hidden
            size={10}
            strokeWidth={2.5}
            {...stylex.props(styles.lock)}
          />
        ) : null}
      </BaseSwitch.Thumb>
    </BaseSwitch.Root>
  );
  if (row) {
    return (
      <div {...stylex.props(styles.row)}>
        {/* oxlint-disable-next-line jsx-a11y/label-has-associated-control -- Base UI's switch is a role="switch" span; wrapping in <label> is its documented pattern. */}
        <label {...stylex.props(styles.rowLabel)}>
          <span
            {...stylex.props(
              styles.rowText,
              disabled ? styles.rowTextDisabled : null
            )}
          >
            {label}
          </span>
          {control}
        </label>
        {description === undefined ? null : (
          <div id={descriptionId} {...stylex.props(styles.rowDesc)}>
            {description}
          </div>
        )}
      </div>
    );
  }
  return label === undefined ? (
    control
  ) : (
    // oxlint-disable-next-line jsx-a11y/label-has-associated-control -- Base UI's switch is a role="switch" span; wrapping in <label> is its documented pattern.
    <label
      {...stylex.props(styles.label, disabled ? styles.labelDisabled : null)}
    >
      {control}
      {label}
    </label>
  );
};
