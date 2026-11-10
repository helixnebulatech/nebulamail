// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Button as BaseButton } from "@base-ui/react/button";
import * as stylex from "@stylexjs/stylex";
import type { LucideIcon } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";

import { Spark } from "./brand";
import { cls, shared } from "./internal";
import { Shortcut } from "./kbd";
import type { KbdTone } from "./kbd";
import { Spinner } from "./spinner";
import { colors, font, motion, radius, shadow, touch } from "./tokens.stylex";
import { Tooltip } from "./tooltip";

/** Icons are lucide components (`import { Send } from "lucide-react"`). */
export type Icon = LucideIcon;

// Focus: a thin accent ring, visible but without a glow.
const FOCUS_RING = `0 0 0 3px color-mix(in srgb, ${colors.acc} 35%, transparent)`;

const styles = stylex.create({
  base: {
    alignItems: "center",
    borderColor: "transparent",
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    cursor: "pointer",
    display: "inline-flex",
    flexShrink: 0,
    fontFamily: font.sans,
    fontWeight: font.weightButton,
    justifyContent: "center",
    position: "relative",
    textDecoration: "none",
    transform: { ":active": "scale(0.98)", default: null },
    transitionDuration: motion.fast,
    transitionProperty: "background-color, box-shadow, color, transform",
    transitionTimingFunction: motion.easing,
    userSelect: "none",
    whiteSpace: "nowrap",
  },
  disabled: {
    boxShadow: "none",
    cursor: "not-allowed",
    opacity: 0.45,
    transform: "none",
  },
  fullWidth: { width: "100%" },
  // Loading keeps full colour (it is busy, not unavailable).
  loading: { cursor: "progress" },
  icon: { flexShrink: 0 },
  label: { overflow: "hidden", textOverflow: "ellipsis" },
});

const sizes = stylex.create({
  lg: {
    borderRadius: "11px",
    columnGap: "8px",
    fontSize: "14.5px",
    height: { default: "42px", [touch.coarse]: touch.target },
    paddingInline: "18px",
  },
  md: {
    borderRadius: radius.control,
    columnGap: "7px",
    fontSize: font.sizeMd,
    height: { default: "34px", [touch.coarse]: touch.target },
    paddingInline: "14px",
  },
  sm: {
    borderRadius: radius.md,
    columnGap: "6px",
    fontSize: "12.5px",
    height: { default: "28px", [touch.coarse]: touch.target },
    paddingInline: "10px",
  },
});

const ICON_SIZE = { lg: 16, md: 15, sm: 14 } as const;

const variants = stylex.create({
  danger: {
    backgroundColor: {
      ":hover": `color-mix(in srgb, ${colors.danger} 88%, ${colors.fg})`,
      default: colors.danger,
    },
    boxShadow: { ":focus-visible": FOCUS_RING, default: null },
    color: colors.dangerFg,
  },
  dangerSoft: {
    backgroundColor: {
      ":hover": `color-mix(in srgb, ${colors.danger} 18%, ${colors.bg})`,
      default: colors.dangerSoft,
    },
    boxShadow: { ":focus-visible": FOCUS_RING, default: null },
    color: colors.danger,
  },
  ghost: {
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    boxShadow: { ":focus-visible": FOCUS_RING, default: null },
    color: { ":hover": colors.fg, default: colors.fg2 },
  },
  ink: {
    backgroundColor: {
      ":hover": `color-mix(in srgb, ${colors.ink} 86%, ${colors.fg3})`,
      default: colors.ink,
    },
    boxShadow: { ":focus-visible": FOCUS_RING, default: null },
    color: colors.inkFg,
  },
  primary: {
    backgroundColor: { ":hover": colors.accHi, default: colors.acc },
    boxShadow: { ":focus-visible": FOCUS_RING, default: shadow.primary },
    color: colors.accFg,
  },
  secondary: {
    backgroundColor: { ":hover": colors.hover, default: colors.bg },
    borderColor: colors.line2,
    boxShadow: { ":focus-visible": FOCUS_RING, default: shadow.sh1 },
    color: colors.fg,
  },
});

// Split buttons (`join`): the halves share one shape, a hairline between.
const joinStyles = stylex.create({
  end: {
    borderEndStartRadius: 0,
    // The divider: the label colour, faint, on any variant.
    borderInlineStartColor: "color-mix(in srgb, currentColor 32%, transparent)",
    borderStartStartRadius: 0,
    // Usually just a chevron: narrower than a labelled button.
    paddingInline: "9px",
  },
  start: { borderEndEndRadius: 0, borderStartEndRadius: 0 },
});

const pressedStyles = stylex.create({
  pressed: { backgroundColor: colors.hover, color: colors.fg },
});

export type ButtonVariant = keyof typeof variants;
export type ButtonSize = keyof typeof sizes;

const KBD_TONE: Record<ButtonVariant, KbdTone> = {
  danger: "onAccent",
  dangerSoft: "default",
  ghost: "default",
  ink: "onInk",
  primary: "onAccent",
  secondary: "default",
};

type BaseButtonProps = Omit<
  ComponentProps<typeof BaseButton>,
  "className" | "style" | "children"
>;

export interface ButtonProps extends BaseButtonProps {
  readonly children?: ReactNode;
  readonly variant?: ButtonVariant;
  readonly size?: ButtonSize;
  /** Leading icon. */
  readonly icon?: Icon;
  /** Trailing icon (e.g. a chevron). */
  readonly iconEnd?: Icon;
  /** Shortcut keys shown inside the button, e.g. `["⌘", "↵"]`. */
  readonly shortcut?: readonly string[];
  /** Shows the loader, sets aria-busy and blocks clicks (keeps focus). */
  readonly loading?: boolean;
  /** Toggle buttons: sets aria-pressed and the "on" look. */
  readonly pressed?: boolean;
  readonly fullWidth?: boolean;
  /**
   * Half of a split button: "start" squares its trailing corners, "end"
   * its leading ones and draws the divider. Put both in a gapless row.
   */
  readonly join?: "start" | "end";
}

export const Button = ({
  children,
  variant = "secondary",
  size = "md",
  icon: IconStart,
  iconEnd: IconEnd,
  shortcut,
  loading = false,
  pressed,
  fullWidth = false,
  disabled = false,
  join,
  ...props
}: ButtonProps) => (
  <BaseButton
    {...props}
    {...(pressed === undefined ? {} : { "aria-pressed": pressed })}
    aria-busy={loading || undefined}
    className={(state) =>
      cls(
        shared.resetButton,
        styles.base,
        sizes[size],
        variants[variant],
        pressed === true ? pressedStyles.pressed : null,
        fullWidth ? styles.fullWidth : null,
        join === undefined ? null : joinStyles[join],
        shared.focusRing,
        loading ? styles.loading : null,
        state.disabled && !loading ? styles.disabled : null
      )
    }
    disabled={disabled || loading}
    focusableWhenDisabled={loading}
  >
    {loading ? <Spinner size="sm" tone="current" /> : null}
    {!loading && IconStart !== undefined ? (
      <IconStart
        aria-hidden
        size={ICON_SIZE[size]}
        {...stylex.props(styles.icon)}
      />
    ) : null}
    {children === undefined ? null : (
      <span {...stylex.props(styles.label)}>{children}</span>
    )}
    {IconEnd === undefined ? null : (
      <IconEnd
        aria-hidden
        size={ICON_SIZE[size]}
        {...stylex.props(styles.icon)}
      />
    )}
    {shortcut === undefined ? null : (
      <Shortcut keys={shortcut} tone={KBD_TONE[variant]} />
    )}
  </BaseButton>
);

const iconStyles = stylex.create({
  base: {
    alignItems: "center",
    borderRadius: radius.md,
    color: { ":hover": colors.fg, default: colors.fg2 },
    cursor: "pointer",
    display: "inline-grid",
    flexShrink: 0,
    justifyContent: "center",
    transform: { ":active": "scale(0.94)", default: null },
    transitionDuration: motion.fast,
    transitionProperty: "background-color, color, transform",
    transitionTimingFunction: motion.easing,
  },
  ghost: {
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
  },
  md: {
    height: { default: "32px", [touch.coarse]: touch.target },
    width: { default: "32px", [touch.coarse]: touch.target },
  },
  pressed: { backgroundColor: colors.hover, color: colors.fg },
  secondary: {
    backgroundColor: { ":hover": colors.hover, default: colors.bg },
    borderColor: colors.line,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh1,
  },
  sm: {
    height: { default: "28px", [touch.coarse]: touch.target },
    width: { default: "28px", [touch.coarse]: touch.target },
  },
  // Chip buttons stay small; on touch an invisible ring makes the hit
  // area `touch.target` wide without moving the layout.
  xs: {
    "::after": {
      content: { default: null, [touch.coarse]: '""' },
      inset: "-11px",
      position: "absolute",
    },
    borderRadius: radius.sm,
    height: "22px",
    position: "relative",
    width: "22px",
  },
});

const ICON_ONLY_SIZE = { md: 16, sm: 16, xs: 14 } as const;

export interface IconButtonProps extends Omit<BaseButtonProps, "aria-label"> {
  readonly icon: Icon;
  /** Accessible name; also the tooltip text. Required. */
  readonly label: string;
  readonly size?: "xs" | "sm" | "md";
  readonly variant?: "ghost" | "secondary";
  /** Shortcut shown in the tooltip. */
  readonly shortcut?: readonly string[];
  /** Show the label as a tooltip on hover/focus (default true). */
  readonly tooltip?: boolean;
  /** Where the tooltip opens (default above). */
  readonly tooltipSide?: "top" | "bottom" | "left" | "right";
  readonly pressed?: boolean;
  /** Draws attention: brand-coloured icon (e.g. the active privacy shield). */
  readonly accent?: boolean;
}

const accentStyles = stylex.create({
  accent: { color: colors.accText },
});

export const IconButton = ({
  icon: IconGlyph,
  label,
  size = "md",
  variant = "ghost",
  shortcut,
  tooltip = true,
  tooltipSide,
  pressed,
  accent = false,
  ...props
}: IconButtonProps) => {
  const button = (
    <BaseButton
      {...props}
      {...(pressed === undefined ? {} : { "aria-pressed": pressed })}
      aria-label={label}
      className={(state) =>
        cls(
          shared.resetButton,
          iconStyles.base,
          iconStyles[size],
          iconStyles[variant],
          pressed === true ? iconStyles.pressed : null,
          accent ? accentStyles.accent : null,
          shared.focusRing,
          state.disabled ? styles.disabled : null
        )
      }
    >
      <IconGlyph aria-hidden size={ICON_ONLY_SIZE[size]} />
    </BaseButton>
  );
  return tooltip ? (
    <Tooltip
      content={label}
      {...(shortcut === undefined ? {} : { shortcut })}
      {...(tooltipSide === undefined ? {} : { side: tooltipSide })}
    >
      {button}
    </Tooltip>
  ) : (
    button
  );
};

const composeStyles = stylex.create({
  base: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.accHi, default: colors.acc },
    borderRadius: radius.lg,
    boxShadow: { ":focus-visible": FOCUS_RING, default: shadow.primary },
    color: colors.accFg,
    columnGap: "10px",
    cursor: "pointer",
    display: "flex",
    fontFamily: font.sans,
    fontSize: "14px",
    fontWeight: font.weightSemibold,
    height: "44px",
    isolation: "isolate",
    letterSpacing: font.trackingTight,
    overflow: "hidden",
    paddingInlineEnd: "12px",
    paddingInlineStart: "14px",
    position: "relative",
    transform: { ":active": "scale(0.98)", default: null },
    transitionDuration: motion.fast,
    transitionProperty: "filter, transform",
    transitionTimingFunction: motion.easing,
    width: "100%",
  },
  collapsed: {
    borderRadius: radius.lg,
    height: "40px",
    justifyContent: "center",
    paddingInline: 0,
    width: "40px",
  },
  // A phone's floating button while the list scrolls down: the icon alone,
  // big enough to tap.
  collapsedFab: { borderRadius: radius.xl, height: "52px", width: "52px" },
  label: { flexGrow: 1, textAlign: "start" },
  spark: { color: colors.accFg, display: "inline-flex" },
});

export interface ComposeButtonProps extends BaseButtonProps {
  /** The visible label, e.g. "Nuevo mensaje". Also the accessible name. */
  readonly children: string;
  readonly shortcut?: readonly string[];
  /**
   * Icon-only (the label becomes aria-label): `true` in the collapsed rail
   * (with a tooltip), "fab" for a phone's floating button (bigger, no
   * tooltip: touch screens have no hover).
   */
  readonly collapsed?: boolean | "fab";
  /**
   * What the button does (a pencil for a message, calendar-plus for an
   * event…), the same icon as its command in the palette. Without one,
   * the brand's star.
   */
  readonly icon?: LucideIcon;
}

/** The big primary button of each module ("New message", "New event"…). */
export const ComposeButton = ({
  children,
  shortcut,
  collapsed = false,
  icon: ActionIcon,
  ...props
}: ComposeButtonProps) => {
  const iconOnly = collapsed !== false;
  const button = (
    <BaseButton
      {...props}
      {...(iconOnly ? { "aria-label": children } : {})}
      className={cls(
        shared.resetButton,
        composeStyles.base,
        iconOnly ? composeStyles.collapsed : null,
        collapsed === "fab" ? composeStyles.collapsedFab : null,
        shared.focusRing
      )}
    >
      <span {...stylex.props(composeStyles.spark)}>
        {ActionIcon === undefined ? (
          <Spark double size={18} />
        ) : (
          <ActionIcon aria-hidden size={17} strokeWidth={2.1} />
        )}
      </span>
      {iconOnly ? null : (
        <>
          <span {...stylex.props(composeStyles.label)}>{children}</span>
          {shortcut === undefined ? null : (
            <Shortcut keys={shortcut} tone="onAccent" />
          )}
        </>
      )}
    </BaseButton>
  );
  return collapsed === true ? (
    <Tooltip
      content={children}
      side="right"
      {...(shortcut === undefined ? {} : { shortcut })}
    >
      {button}
    </Tooltip>
  ) : (
    button
  );
};
