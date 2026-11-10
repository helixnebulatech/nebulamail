// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { ContextMenu as BaseContextMenu } from "@base-ui/react/context-menu";
import { Menu as BaseMenu } from "@base-ui/react/menu";
import * as stylex from "@stylexjs/stylex";
import type { CompiledStyles } from "@stylexjs/stylex";
import { Check, ChevronRight } from "lucide-react";
import type { ReactElement, ReactNode } from "react";

import type { Icon } from "./button";
import { cls, usePortalContainer } from "./internal";
import { Shortcut } from "./kbd";
import {
  colors,
  density,
  font,
  motion,
  radius,
  shadow,
  zIndex,
} from "./tokens.stylex";
import { Tooltip } from "./tooltip";

/** Shared with Select (form-controls.tsx), so both look the same. */
export const menuStyles = stylex.create({
  danger: { color: colors.danger },
  disabled: { cursor: "default", opacity: 0.45 },
  groupLabel: {
    color: colors.fg3,
    fontSize: "11.5px",
    fontWeight: font.weightSemibold,
    paddingBlock: "6px 2px",
    paddingInline: "10px",
  },
  hidden: { opacity: 0, transform: "scale(0.96) translateY(-4px)" },
  highlighted: { backgroundColor: colors.hover },
  icon: { color: colors.fg3, flexShrink: 0 },
  iconDanger: { color: colors.danger },
  item: {
    alignItems: "center",
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "10px",
    cursor: "pointer",
    display: "flex",
    fontSize: font.sizeMd,
    height: density.menuItem,
    outline: "none",
    paddingInline: "10px",
    userSelect: "none",
  },
  label: { flexGrow: 1, whiteSpace: "nowrap" },
  popup: {
    backgroundColor: colors.raised,
    borderColor: colors.line2,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    fontFamily: font.sans,
    maxHeight: "var(--available-height)",
    minWidth: "220px",
    outline: "none",
    overflowY: "auto",
    padding: "5px",
    transformOrigin: "var(--transform-origin)",
    transitionDuration: motion.fast,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
  },
  positioner: { outline: "none", zIndex: zIndex.popover },
  // A colour sample in place of the icon (text colour, highlight).
  swatch: {
    borderColor: colors.line2,
    borderRadius: radius.pill,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    flexShrink: 0,
    height: "16px",
    width: "16px",
  },
  swatchColor: (color: string) => ({ backgroundColor: color }),
  check: { color: colors.accText, flexShrink: 0 },
  separator: {
    backgroundColor: colors.line,
    height: "1px",
    marginBlock: "5px",
    marginInline: "6px",
  },
});

/** `extra`: styles over the menu's popup (one stylex call, so they win). */
export const popupClass = (
  state: { transitionStatus: string | undefined },
  extra: CompiledStyles | null = null
) =>
  cls(
    menuStyles.popup,
    extra,
    state.transitionStatus === "starting" || state.transitionStatus === "ending"
      ? menuStyles.hidden
      : null
  );

export interface MenuProps {
  /** The trigger element, e.g. an IconButton or Button. */
  readonly trigger: ReactElement;
  readonly children: ReactNode;
  readonly side?: "top" | "bottom" | "left" | "right";
  readonly align?: "start" | "center" | "end";
  readonly open?: boolean;
  readonly onOpenChange?: (open: boolean) => void;
  /** Tooltip on the trigger (its name and shortcut). */
  readonly tooltip?: ReactNode;
  readonly tooltipShortcut?: readonly string[];
}

/** Dropdown menu. Items: MenuItem, MenuSeparator, MenuGroup. */
export const Menu = ({
  trigger,
  children,
  side = "bottom",
  align = "start",
  open,
  onOpenChange,
  tooltip,
  tooltipShortcut,
}: MenuProps) => {
  const container = usePortalContainer();
  const menuTrigger = <BaseMenu.Trigger render={trigger} />;
  return (
    <BaseMenu.Root
      {...(open === undefined ? {} : { open })}
      {...(onOpenChange === undefined
        ? {}
        : {
            onOpenChange: (next: boolean) => {
              onOpenChange(next);
            },
          })}
    >
      {tooltip === undefined ? (
        menuTrigger
      ) : (
        <Tooltip
          content={tooltip}
          {...(tooltipShortcut === undefined
            ? {}
            : { shortcut: tooltipShortcut })}
        >
          {menuTrigger}
        </Tooltip>
      )}
      <BaseMenu.Portal container={container}>
        <BaseMenu.Positioner
          align={align}
          className={cls(menuStyles.positioner)}
          side={side}
          sideOffset={6}
        >
          <BaseMenu.Popup className={popupClass}>{children}</BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.Root>
  );
};

export interface ContextMenuProps {
  /** The area that opens the menu on right click / long press. */
  readonly children: ReactElement;
  /** Menu content: MenuItem, MenuSeparator, MenuGroup. */
  readonly menu: ReactNode;
}

/** Right-click menu over its child element. */
export const ContextMenu = ({ children, menu }: ContextMenuProps) => {
  const container = usePortalContainer();
  return (
    <BaseContextMenu.Root>
      <BaseContextMenu.Trigger render={children} />
      <BaseContextMenu.Portal container={container}>
        <BaseContextMenu.Positioner className={cls(menuStyles.positioner)}>
          <BaseContextMenu.Popup className={popupClass}>
            {menu}
          </BaseContextMenu.Popup>
        </BaseContextMenu.Positioner>
      </BaseContextMenu.Portal>
    </BaseContextMenu.Root>
  );
};

export interface MenuItemProps {
  readonly children: ReactNode;
  readonly icon?: Icon;
  readonly shortcut?: readonly string[];
  /** Destructive action (orange-red). */
  readonly danger?: boolean;
  readonly disabled?: boolean;
  readonly onClick?: () => void;
  /** Keep the menu open after clicking. */
  readonly keepOpen?: boolean;
}

export const MenuItem = ({
  children,
  icon: IconGlyph,
  shortcut,
  danger = false,
  disabled = false,
  onClick,
  keepOpen = false,
}: MenuItemProps) => (
  <BaseMenu.Item
    className={(state) =>
      cls(
        menuStyles.item,
        danger ? menuStyles.danger : null,
        state.highlighted ? menuStyles.highlighted : null,
        state.disabled ? menuStyles.disabled : null
      )
    }
    closeOnClick={!keepOpen}
    disabled={disabled}
    {...(onClick === undefined
      ? {}
      : {
          onClick: () => {
            onClick();
          },
        })}
  >
    {IconGlyph === undefined ? null : (
      <IconGlyph
        aria-hidden
        size={16}
        {...stylex.props(
          menuStyles.icon,
          danger ? menuStyles.iconDanger : null
        )}
      />
    )}
    <span {...stylex.props(menuStyles.label)}>{children}</span>
    {shortcut === undefined ? null : <Shortcut keys={shortcut} />}
  </BaseMenu.Item>
);

export interface MenuSubmenuProps {
  /** The item that opens it (hover, → or Enter). */
  readonly label: ReactNode;
  readonly icon?: Icon;
  readonly children: ReactNode;
  readonly disabled?: boolean;
}

/** A nested menu (e.g. "Move to…" with the folders), inside Menu or ContextMenu. */
export const MenuSubmenu = ({
  label,
  icon: IconGlyph,
  children,
  disabled = false,
}: MenuSubmenuProps) => {
  const container = usePortalContainer();
  return (
    <BaseMenu.SubmenuRoot>
      <BaseMenu.SubmenuTrigger
        className={(state) =>
          cls(
            menuStyles.item,
            state.highlighted || state.open ? menuStyles.highlighted : null,
            state.disabled ? menuStyles.disabled : null
          )
        }
        disabled={disabled}
      >
        {IconGlyph === undefined ? null : (
          <IconGlyph aria-hidden size={16} {...stylex.props(menuStyles.icon)} />
        )}
        <span {...stylex.props(menuStyles.label)}>{label}</span>
        <ChevronRight
          aria-hidden
          size={14}
          {...stylex.props(menuStyles.icon)}
        />
      </BaseMenu.SubmenuTrigger>
      <BaseMenu.Portal container={container}>
        <BaseMenu.Positioner
          alignOffset={-4}
          className={cls(menuStyles.positioner)}
          sideOffset={4}
        >
          <BaseMenu.Popup className={popupClass}>{children}</BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.SubmenuRoot>
  );
};

export const MenuSeparator = () => (
  <BaseMenu.Separator className={cls(menuStyles.separator)} />
);

export interface MenuGroupProps {
  readonly label: ReactNode;
  readonly children: ReactNode;
}

export const MenuGroup = ({ label, children }: MenuGroupProps) => (
  <BaseMenu.Group>
    <BaseMenu.GroupLabel className={cls(menuStyles.groupLabel)}>
      {label}
    </BaseMenu.GroupLabel>
    {children}
  </BaseMenu.Group>
);

export interface MenuRadioGroupProps {
  /** The value of the checked item (none checked if no item has it). */
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  readonly children: ReactNode;
}

/** Items of which one is the current choice (MenuRadioItem). */
export const MenuRadioGroup = ({
  value,
  onValueChange,
  children,
}: MenuRadioGroupProps) => (
  <BaseMenu.RadioGroup
    onValueChange={(next: unknown) => {
      if (typeof next === "string") {
        onValueChange(next);
      }
    }}
    value={value}
  >
    {children}
  </BaseMenu.RadioGroup>
);

export interface MenuRadioItemProps {
  readonly value: string;
  readonly children: ReactNode;
  readonly icon?: Icon;
  /** A colour sample instead of the icon (a CSS colour; null: none). */
  readonly swatch?: string | null;
  readonly shortcut?: readonly string[];
  /** Close the menu once chosen (a toolbar menu); off: it stays open. */
  readonly closeOnClick?: boolean;
}

/** One choice of a MenuRadioGroup; the current one has a check mark. */
export const MenuRadioItem = ({
  value,
  children,
  closeOnClick = false,
  icon: IconGlyph,
  swatch,
  shortcut,
}: MenuRadioItemProps) => (
  <BaseMenu.RadioItem
    closeOnClick={closeOnClick}
    className={(state) =>
      cls(menuStyles.item, state.highlighted ? menuStyles.highlighted : null)
    }
    value={value}
  >
    {IconGlyph === undefined ? null : (
      <IconGlyph aria-hidden size={16} {...stylex.props(menuStyles.icon)} />
    )}
    {swatch === undefined ? null : (
      <span
        aria-hidden
        {...stylex.props(
          menuStyles.swatch,
          swatch === null ? null : menuStyles.swatchColor(swatch)
        )}
      />
    )}
    <span {...stylex.props(menuStyles.label)}>{children}</span>
    {shortcut === undefined ? null : <Shortcut keys={shortcut} />}
    <BaseMenu.RadioItemIndicator
      keepMounted
      render={(props, state) => (
        <span {...props}>
          <Check
            aria-hidden
            size={15}
            {...stylex.props(menuStyles.check)}
            // Hidden, not removed: the labels stay aligned.
            opacity={state.checked ? 1 : 0}
          />
        </span>
      )}
    />
  </BaseMenu.RadioItem>
);

export interface MenuCheckboxItemProps {
  readonly checked: boolean;
  readonly onCheckedChange: (checked: boolean) => void;
  readonly children: ReactNode;
  readonly icon?: Icon;
}

/** An on/off option inside a menu; when on, it has a check mark. */
export const MenuCheckboxItem = ({
  checked,
  onCheckedChange,
  children,
  icon: IconGlyph,
}: MenuCheckboxItemProps) => (
  <BaseMenu.CheckboxItem
    checked={checked}
    className={(state) =>
      cls(menuStyles.item, state.highlighted ? menuStyles.highlighted : null)
    }
    onCheckedChange={(next: boolean) => {
      onCheckedChange(next);
    }}
  >
    {IconGlyph === undefined ? null : (
      <IconGlyph aria-hidden size={16} {...stylex.props(menuStyles.icon)} />
    )}
    <span {...stylex.props(menuStyles.label)}>{children}</span>
    <BaseMenu.CheckboxItemIndicator
      keepMounted
      render={(props, state) => (
        <span {...props}>
          <Check
            aria-hidden
            size={15}
            {...stylex.props(menuStyles.check)}
            // Hidden, not removed: the labels stay aligned.
            opacity={state.checked ? 1 : 0}
          />
        </span>
      )}
    />
  </BaseMenu.CheckboxItem>
);
