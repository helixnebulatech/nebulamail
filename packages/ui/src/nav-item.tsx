// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { mergeProps } from "@base-ui/react/merge-props";
import { useRender } from "@base-ui/react/use-render";
import * as stylex from "@stylexjs/stylex";
import type { ReactElement, ReactNode } from "react";

import type { Icon } from "./button";
import { cls, shared } from "./internal";
import { colors, density, font, motion, radius, touch } from "./tokens.stylex";

const styles = stylex.create({
  // The current place: the selected fill, the same as the open row in the
  // message list and the active app in the rails, in both themes (a white
  // chip barely showed on the light sidebar).
  active: {
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    color: { ":hover": colors.selFg, default: colors.selFg },
    fontWeight: font.weightSemibold,
  },
  // Kept for the panel surface (settings): the same fill already works.
  activeOnPanel: {},
  base: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    boxSizing: "border-box",
    color: { ":hover": colors.fg, default: colors.fg2 },
    columnGap: "10px",
    cursor: "pointer",
    display: "flex",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    height: density.navItem,
    paddingInline: "10px",
    textDecoration: "none",
    transitionDuration: motion.fast,
    transitionProperty: "background-color, color",
    transitionTimingFunction: motion.easing,
    width: "100%",
  },
  count: {
    color: colors.accText,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    fontWeight: 650,
  },
  countMuted: { color: colors.fg3, fontWeight: font.weightMedium },
  // A row "⋯" laid over the count (`menuOverCount`): the count steps aside
  // while the row (marked with stylex.defaultMarker()) is hovered or holds
  // focus, and on touch screens, where the "⋯" always shows.
  countUnderMenu: {
    opacity: {
      default: 1,
      [stylex.when.ancestor(":hover")]: 0,
      [stylex.when.ancestor(":focus-within")]: 0,
      [touch.noHover]: 0,
    },
  },
  dot: (hue: number) => ({
    // Same hue family as the label chips (badge.tsx), as a mid-tone dot.
    backgroundColor: `oklch(0.65 0.15 ${hue})`,
  }),
  dotColor: (color: string) => ({ backgroundColor: color }),
  dotBase: {
    borderRadius: "4px",
    flexShrink: 0,
    height: "10px",
    marginInline: "3px",
    width: "10px",
  },
  icon: { flexShrink: 0 },
  iconActive: { color: colors.accText },
  label: {
    flexGrow: 1,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
});

export interface NavItemProps {
  readonly children: ReactNode;
  readonly icon?: Icon;
  /** Label colour dot instead of an icon (HSL hue). */
  readonly hue?: number;
  /** Colour dot of a CSS colour the app checked (a list's, a calendar's). */
  readonly color?: string;
  readonly count?: number;
  /** Grey count (totals, not unread). */
  readonly mutedCount?: boolean;
  /** Current page: highlighted and aria-current="page". */
  readonly active?: boolean;
  /**
   * "side" (default): on the sidebar's tinted background. "panel": on a
   * white panel, where the active row takes the selected fill.
   */
  readonly surface?: "side" | "panel";
  readonly href?: string;
  /** Router link element, e.g. `<Link to="/inbox" />`. */
  readonly render?: ReactElement;
  readonly onClick?: () => void;
  /** The full name where the label may be cut (a long list's name). */
  readonly title?: string;
  /**
   * The row lays a "⋯" over the count's place: hide the count while the
   * row (its ancestor with stylex.defaultMarker()) is hovered or focused.
   */
  readonly menuOverCount?: boolean;
}

const activeStyles = (
  active: boolean,
  surface: "side" | "panel"
): readonly stylex.StyleXStyles[] => {
  if (!active) {
    return [];
  }
  return surface === "panel"
    ? [styles.active, styles.activeOnPanel]
    : [styles.active];
};

const Count = ({
  count,
  menuOver,
  muted,
}: {
  readonly count: number | undefined;
  readonly menuOver: boolean;
  readonly muted: boolean;
}) =>
  count === undefined || count === 0 ? null : (
    <span
      {...stylex.props(
        styles.count,
        muted ? styles.countMuted : null,
        menuOver ? styles.countUnderMenu : null
      )}
    >
      {count}
    </span>
  );

/** Sidebar row: icon, label, count. A link (href/render) or a button. */
export const NavItem = ({
  children,
  icon: IconGlyph,
  hue,
  color,
  count,
  mutedCount = false,
  active = false,
  surface = "side",
  href,
  render,
  onClick,
  title,
  menuOverCount = false,
}: NavItemProps) => {
  const isLink = href !== undefined || render !== undefined;
  const content = (
    <>
      {hue === undefined ? null : (
        <span {...stylex.props(styles.dotBase, styles.dot(hue))} />
      )}
      {color === undefined ? null : (
        <span {...stylex.props(styles.dotBase, styles.dotColor(color))} />
      )}
      {IconGlyph === undefined ? null : (
        <IconGlyph
          aria-hidden
          size={16}
          {...stylex.props(styles.icon, active ? styles.iconActive : null)}
        />
      )}
      <span {...stylex.props(styles.label)}>{children}</span>
      <Count count={count} menuOver={menuOverCount} muted={mutedCount} />
    </>
  );
  return useRender({
    defaultTagName: isLink ? "a" : "button",
    props: mergeProps<"a">(
      {
        className: cls(
          ...(isLink ? [] : [shared.resetButton]),
          styles.base,
          ...activeStyles(active, surface),
          shared.focusRing,
          shared.focusRingInset
        ),
        ...(active ? { "aria-current": "page" as const } : {}),
        ...(href === undefined ? {} : { href }),
        ...(title === undefined ? {} : { title }),
        ...(onClick === undefined
          ? {}
          : {
              onClick: () => {
                onClick();
              },
            }),
        ...(isLink ? {} : { type: "button" }),
      },
      { children: content }
    ),
    ...(render === undefined ? {} : { render }),
  });
};
