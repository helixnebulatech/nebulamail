// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Tabs as BaseTabs } from "@base-ui/react/tabs";
import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { NebulaLine } from "./brand";
import type { Icon } from "./button";
import { cls, shared } from "./internal";
import { colors, density, font, motion, radius } from "./tokens.stylex";

/**
 * A narrower tab list (the list pane on a ~1024 px window): tighter tabs,
 * so the three labels still fit.
 */
const TIGHT = "@container tabs (max-width: 360px)";
/**
 * Too narrow even for that: the tabs that aren't selected keep their icon
 * and count, and their label only for screen readers, so nothing gets cut
 * off.
 */
const NARROW = "@container tabs (max-width: 290px)";

const styles = stylex.create({
  activeAccent: { color: colors.accText },
  count: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    fontWeight: font.weightMedium,
  },
  countActive: { color: colors.accText, fontWeight: 650 },
  icon: { flexShrink: 0 },
  /** Inactive tab label: hidden visually in a narrow list (NARROW). */
  labelCollapsible: {
    clipPath: { default: null, [NARROW]: "inset(50%)" },
    height: { default: null, [NARROW]: "1px" },
    overflow: { default: null, [NARROW]: "hidden" },
    position: { default: null, [NARROW]: "absolute" },
    whiteSpace: { default: null, [NARROW]: "nowrap" },
    width: { default: null, [NARROW]: "1px" },
  },
  list: {
    alignItems: "center",
    containerName: "tabs",
    containerType: "inline-size",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "2px",
    display: "flex",
    paddingInline: "10px",
    paddingTop: "10px",
  },
  // Inside a row that draws the line itself (tabs and tools on one row).
  listFlush: {
    borderBottomWidth: 0,
    flexGrow: 1,
    minWidth: 0,
    paddingInlineEnd: 0,
  },
  // The panel fills the space under the tab list (so its own content can
  // scroll), and the hidden ones stay out of the layout.
  panel: {
    display: { ":is([hidden])": "none", default: "flex" },
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    outline: "none",
  },
  root: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
  },
  tab: {
    alignItems: "center",
    borderEndEndRadius: 0,
    borderEndStartRadius: 0,
    borderStartEndRadius: radius.md,
    borderStartStartRadius: radius.md,
    color: { ":hover": colors.fg, default: colors.fg3 },
    columnGap: { default: "7px", [TIGHT]: "5px" },
    cursor: "pointer",
    display: "flex",
    flexShrink: 0,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    height: density.tab,
    paddingInline: { default: "10px", [TIGHT]: "6px" },
    position: "relative",
    transitionDuration: motion.fast,
    transitionProperty: "color",
    transitionTimingFunction: motion.easing,
    whiteSpace: "nowrap",
  },
  tabActive: { color: colors.fg, fontWeight: 650 },
  // The motif: the nebula line under the active tab.
  underline: {
    bottom: "-1px",
    insetInlineEnd: "8px",
    insetInlineStart: "8px",
    pointerEvents: "none",
    position: "absolute",
  },
});

export interface TabsProps<Value extends string> {
  readonly value?: Value;
  readonly defaultValue?: Value;
  readonly onValueChange?: (value: Value) => void;
  readonly children: ReactNode;
}

/** Split-inbox tabs. Compose with TabList, Tab and TabPanel. */
export const Tabs = <Value extends string>({
  value,
  defaultValue,
  onValueChange,
  children,
}: TabsProps<Value>) => (
  <BaseTabs.Root
    className={cls(styles.root)}
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
    {children}
  </BaseTabs.Root>
);

export interface TabListProps {
  /** Accessible name, e.g. "Bandeja dividida". */
  readonly label: string;
  readonly children: ReactNode;
  /**
   * Inside a row that draws the bottom line and holds other controls
   * after the tabs: no line of its own, and it takes the room left.
   */
  readonly flush?: boolean;
}

export const TabList = ({ label, children, flush = false }: TabListProps) => (
  <BaseTabs.List
    aria-label={label}
    className={cls(styles.list, flush ? styles.listFlush : null)}
  >
    {children}
  </BaseTabs.List>
);

export interface TabProps {
  readonly value: string;
  readonly children: ReactNode;
  readonly icon?: Icon;
  /** Unread count shown after the label. */
  readonly count?: number;
  readonly disabled?: boolean;
}

export const Tab = ({
  value,
  children,
  icon: IconGlyph,
  count,
  disabled = false,
}: TabProps) => (
  <BaseTabs.Tab
    className={(state) =>
      cls(
        shared.resetButton,
        styles.tab,
        state.active ? styles.tabActive : null,
        shared.focusRing
      )
    }
    disabled={disabled}
    render={(props, state) => (
      <button {...props} type="button">
        {IconGlyph === undefined ? null : (
          <IconGlyph
            aria-hidden
            size={15}
            {...stylex.props(
              styles.icon,
              state.active ? styles.activeAccent : null
            )}
          />
        )}
        <span {...stylex.props(state.active ? null : styles.labelCollapsible)}>
          {children}
        </span>
        {count === undefined ? null : (
          <span
            {...stylex.props(
              styles.count,
              state.active ? styles.countActive : null
            )}
          >
            {count}
          </span>
        )}
        {state.active ? (
          <span {...stylex.props(styles.underline)}>
            <NebulaLine />
          </span>
        ) : null}
      </button>
    )}
    value={value}
  />
);

export interface TabPanelProps {
  readonly value: string;
  readonly children: ReactNode;
}

export const TabPanel = ({ value, children }: TabPanelProps) => (
  <BaseTabs.Panel className={cls(styles.panel)} value={value}>
    {children}
  </BaseTabs.Panel>
);
