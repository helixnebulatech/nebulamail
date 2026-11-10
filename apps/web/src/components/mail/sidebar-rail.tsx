// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The collapsed sidebar (an icon rail) shared by mail, contacts and
// settings: the mark, the module's primary button as an icon, one 40px
// square per place with a tooltip, and the expand button at the bottom.
// One component, so the rail looks the same in every module.

import { IconButton, Mark, Menu, Tooltip } from "@nebulamail/ui";
import type { Icon } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Keyboard, PanelLeftOpen } from "lucide-react";
import type { ReactElement, ReactNode } from "react";

import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useMailUi } from "./mail-ui";
import { AppsMenu } from "./sidebar-top";
import { shared } from "./styles";

export const railStyles = stylex.create({
  // Full height, so the spacer pushes the expand button to the bottom.
  rail: {
    alignItems: "center",
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    height: "100%",
    minHeight: 0,
    overflowY: "auto",
    paddingBlock: "14px",
    rowGap: "4px",
  },
  badge: {
    alignItems: "center",
    backgroundColor: colors.acc,
    borderColor: colors.side,
    borderRadius: "9px",
    borderStyle: "solid",
    borderWidth: "2px",
    color: colors.accFg,
    display: "grid",
    fontSize: "10px",
    fontWeight: font.weightBold,
    height: "16px",
    // On the chip's corner, half outside it: the active chip's fill stays
    // whole, and the side-coloured border cuts it out cleanly.
    insetInlineEnd: "-4px",
    justifyContent: "center",
    minWidth: "16px",
    paddingInline: "3px",
    position: "absolute",
    top: "-4px",
    zIndex: 1,
    boxSizing: "border-box",
  },
  brand: { marginBottom: "6px" },
  primary: { marginBottom: "10px" },
  item: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    color: { ":hover": colors.fg, default: colors.fg2 },
    display: "grid",
    height: "40px",
    justifyContent: "center",
    position: "relative",
    textDecoration: "none",
    width: "40px",
  },
  itemActive: {
    // Same as the sidebar's current item (NavItem): the selected fill.
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    color: { ":hover": colors.accText, default: colors.accText },
  },
  spacer: { flexGrow: 1 },
  // Compact windows show this rail instead of the right-hand app rail
  // (shell.tsx): the other modules and the shortcuts move to its foot.
  apps: {
    alignItems: "center",
    display: { default: "none", [bp.compact]: "flex" },
    flexDirection: "column",
    rowGap: "4px",
  },
});

/** What a rail link receives: its name, state, look and icon. */
export interface RailLinkProps {
  readonly "aria-label": string;
  readonly "aria-current"?: "page";
  readonly className?: string;
  readonly style?: Readonly<Record<string, string | number>>;
  readonly children: ReactNode;
}

/**
 * One place in the rail. `render` draws the link (a router `<Link>` with
 * these props spread on it); without it the item is a button that runs
 * `onClick`.
 */
export const RailItem = ({
  active = false,
  badge,
  icon: ItemIcon,
  label,
  onClick,
  render,
  shortcut,
}: {
  readonly label: string;
  readonly icon: Icon;
  readonly active?: boolean;
  /** An unread count, shown as a small badge (99+ above 99). */
  readonly badge?: number | undefined;
  readonly shortcut?: readonly string[] | undefined;
  readonly render?: (props: RailLinkProps) => ReactElement;
  readonly onClick?: () => void;
}) => {
  const content = (
    <>
      <ItemIcon aria-hidden size={18} />
      {badge !== undefined && badge > 0 ? (
        <span aria-hidden {...stylex.props(railStyles.badge)}>
          {badge > 99 ? "99+" : badge}
        </span>
      ) : null}
    </>
  );
  const name =
    badge === undefined || badge === 0 ? label : `${label} (${badge})`;
  return (
    <Tooltip
      content={label}
      side="right"
      {...(shortcut === undefined ? {} : { shortcut })}
    >
      {render === undefined ? (
        <button
          aria-label={name}
          aria-pressed={active}
          onClick={onClick}
          type="button"
          {...stylex.props(
            shared.resetButton,
            railStyles.item,
            active ? railStyles.itemActive : null,
            shared.focusRing
          )}
        >
          {content}
        </button>
      ) : (
        render({
          "aria-label": name,
          ...(active ? { "aria-current": "page" as const } : {}),
          ...stylex.props(
            railStyles.item,
            active ? railStyles.itemActive : null,
            shared.focusRing
          ),
          children: content,
        })
      )}
    </Tooltip>
  );
};

/**
 * A place in the rail that opens a menu of more places (the folders the
 * full sidebar lists under "More" and "Folders"). Active while one of them
 * is open, so the rail always shows where you are.
 */
export const RailMenu = ({
  active = false,
  children,
  icon: ItemIcon,
  label,
}: {
  readonly label: string;
  readonly icon: Icon;
  readonly active?: boolean;
  /** The menu's items (MenuItem). */
  readonly children: ReactNode;
}) => (
  <Menu
    side="right"
    tooltip={label}
    trigger={
      <button
        aria-label={label}
        type="button"
        {...stylex.props(
          shared.resetButton,
          railStyles.item,
          active ? railStyles.itemActive : null,
          shared.focusRing
        )}
      >
        <ItemIcon aria-hidden size={18} />
      </button>
    }
  >
    {children}
  </Menu>
);

const HelpButton = () => {
  const ui = useMailUi();
  return (
    <IconButton
      icon={Keyboard}
      label={m.help_shortcuts()}
      onClick={() => {
        ui.setHelpOpen(true);
      }}
      shortcut={["?"]}
      size="sm"
      tooltipSide="right"
    />
  );
};

/** The rail itself: mark, primary button, the places, expand. */
export const SidebarRail = ({
  children,
  label,
  onExpand,
  primary,
}: {
  /** The nav landmark's name ("Mail folders", "Settings sections"…). */
  readonly label: string;
  /** The module's primary button, `collapsed`. */
  readonly primary?: ReactNode;
  /** The places (RailItem). */
  readonly children: ReactNode;
  /** Omitted where the rail is forced by the window's width. */
  readonly onExpand?: (() => void) | undefined;
}) => (
  <nav aria-label={label} {...stylex.props(railStyles.rail)}>
    <span {...stylex.props(railStyles.brand)}>
      <Mark label={m.app_name()} size={26} />
    </span>
    {primary === undefined ? null : (
      <span {...stylex.props(railStyles.primary)}>{primary}</span>
    )}
    {children}
    <span {...stylex.props(railStyles.spacer)} />
    <span {...stylex.props(railStyles.apps)}>
      <AppsMenu side="right" />
      <HelpButton />
    </span>
    {onExpand === undefined ? null : (
      <IconButton
        icon={PanelLeftOpen}
        label={m.sidebar_expand()}
        onClick={onExpand}
        tooltipSide="right"
      />
    )}
  </nav>
);
