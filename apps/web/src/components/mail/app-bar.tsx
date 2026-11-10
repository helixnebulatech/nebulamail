// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The one top bar of every module (mail, contacts, settings, calendar,
// tasks): the same height, edges and right-hand controls everywhere, so
// switching modules moves nothing but what belongs to the module.
//
//   [menu] [title ……………] [ search (centred, ≤520px) ] [……… extras | ? ⌘ ⚙ account]
//
// The start always names where you are (`AppBarTitle`: the folder, the
// task view, "Contacts", "Settings"); the calendar puts its date controls
// and range there instead.
//
// The search sits in the centre column of a three-column grid, so it lands
// in the same place and width in every module that has one; a module
// without search (calendar) gets two columns. On phones it is a single row.

import { IconButton } from "@nebulamail/ui";
import { colors, font, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Keyboard, Menu as MenuIcon, Zap } from "lucide-react";
import type { ReactNode } from "react";

import { m } from "../../paraglide/messages.js";
import { AccountMenu } from "./account-menu";
import { bp } from "./breakpoints.stylex";
import { useMailUi } from "./mail-ui";
import { SettingsPopover } from "./settings-popover";

const styles = stylex.create({
  bar: {
    alignItems: "center",
    columnGap: { default: "12px", [bp.mobile]: "6px" },
    display: { default: "grid", [bp.mobile]: "flex" },
    height: "60px",
    minWidth: 0,
    // The panels below start 2px after the sidebar and end 10px before
    // the rail (shell.tsx): the bar's content uses the same edges.
    paddingInlineEnd: { default: "10px", [bp.mobile]: "8px" },
    paddingInlineStart: { default: "2px", [bp.mobile]: "8px" },
  },
  // Left and right share what the search leaves equally, whatever each
  // module puts there, so the search lands on the same pixels in every
  // module. The right-hand controls are kept narrow enough to fit (the
  // account shows a short name, nothing on compact windows).
  // Below 1000px there's no room for that symmetry: the controls take
  // what they need, the title keeps at least a few words, the search
  // the rest.
  withSearch: {
    gridTemplateColumns: {
      default: "minmax(0, 1fr) minmax(0, 520px) minmax(0, 1fr)",
      [bp.narrow]: "minmax(72px, max-content) minmax(120px, 1fr) max-content",
    },
  },
  withoutSearch: { gridTemplateColumns: "minmax(0, 1fr) max-content" },
  start: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    flexGrow: { default: null, [bp.mobile]: 0 },
    flexShrink: 1,
    minWidth: 0,
  },
  startAlone: { flexGrow: { default: null, [bp.mobile]: 1 } },
  search: {
    display: "flex",
    flexBasis: { default: null, [bp.mobile]: 0 },
    flexGrow: { default: null, [bp.mobile]: 1 },
    minWidth: 0,
  },
  end: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    justifyContent: "flex-end",
    minWidth: 0,
  },
  divider: {
    backgroundColor: colors.line,
    display: {
      default: "block",
      [bp.compact]: "none",
      [bp.narrow]: "none",
    },
    height: "20px",
    marginInline: "2px",
    width: "1px",
  },
  mobileOnly: { display: { default: "none", [bp.mobile]: "inline-flex" } },
  // The shortcut help lives in the app rail; where the rail hides (narrow
  // windows) it comes here. Phones have no keyboard to show it for.
  helpButton: {
    display: {
      default: "none",
      [bp.narrow]: "inline-flex",
      [bp.narrowTouch]: "none",
      [bp.mobile]: "none",
    },
  },
  touchOnly: { display: { default: "none", [touch.coarse]: "inline-flex" } },
  hideOnMobile: { display: { default: "inline-flex", [bp.mobile]: "none" } },
  title: {
    alignItems: "center",
    columnGap: "10px",
    display: "flex",
    flexShrink: 1,
    fontSize: { default: font.sizeXl, [bp.mobile]: font.sizeLg },
    fontWeight: font.weightBold,
    letterSpacing: font.trackingTight,
    margin: 0,
    maxWidth: { default: "100%", [bp.mobile]: "45vw" },
    minWidth: 0,
    // On the panels' left edge, like the calendar's first control: one
    // edge for everything at the start of the bar.
    paddingInlineStart: 0,
  },
  // Where the panel below names the page itself on phones (mail, contacts).
  titleNoMobile: { display: { default: "flex", [bp.mobile]: "none" } },
  titleText: {
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
});

/**
 * The page's name at the start of the bar, the same in every module. Not a
 * heading: each panel keeps its own <h1> (visually hidden where this names
 * it), so the outline doesn't change with the window's width.
 */
export const AppBarTitle = ({
  before,
  children,
  heading = false,
  hideOnMobile = false,
}: {
  readonly children: ReactNode;
  /** A mark before the text (a list's colour). */
  readonly before?: ReactNode;
  /** The bar's title is the page's only <h1> (tasks). */
  readonly heading?: boolean;
  /** Phones: the panel shows the name instead. */
  readonly hideOnMobile?: boolean;
}) => {
  const Tag = heading ? "h1" : "p";
  return (
    <Tag
      {...(heading ? { "aria-live": "polite" as const } : {})}
      {...stylex.props(
        styles.title,
        hideOnMobile ? styles.titleNoMobile : null
      )}
    >
      {before}
      <bdi {...stylex.props(styles.titleText)}>{children}</bdi>
    </Tag>
  );
};

export interface AppBarProps {
  /** Opens the module's sidebar drawer on phones. */
  readonly onMenu: () => void;
  readonly menuLabel: string;
  /** The module's title or navigation, left. */
  readonly start?: ReactNode;
  /** The module's search box, centred. */
  readonly search?: ReactNode;
  /** The module's own controls, right, before the shared ones. */
  readonly extras?: ReactNode;
  /**
   * Phones: the quick settings button. Off by default: a phone's bar keeps
   * its room for the title and search, and the account menu has "All
   * settings".
   */
  readonly settingsOnMobile?: boolean;
  /**
   * Touch screens: the ⌘K palette button. The calendar's crowded row
   * leaves it out (its view menu has what the row hides).
   */
  readonly palette?: boolean;
}

export const AppBar = ({
  extras,
  menuLabel,
  onMenu,
  palette = true,
  search,
  settingsOnMobile = false,
  start,
}: AppBarProps) => {
  const ui = useMailUi();
  const hasSearch = search !== undefined;
  return (
    <div
      {...stylex.props(
        styles.bar,
        hasSearch ? styles.withSearch : styles.withoutSearch
      )}
    >
      <div
        {...stylex.props(styles.start, hasSearch ? null : styles.startAlone)}
      >
        <span {...stylex.props(styles.mobileOnly)}>
          <IconButton icon={MenuIcon} label={menuLabel} onClick={onMenu} />
        </span>
        {start}
      </div>
      {hasSearch ? <div {...stylex.props(styles.search)}>{search}</div> : null}
      <div {...stylex.props(styles.end)}>
        {extras}
        {extras === undefined ? null : (
          <span aria-hidden {...stylex.props(styles.divider)} />
        )}
        <span {...stylex.props(styles.helpButton)}>
          <IconButton
            icon={Keyboard}
            label={m.help_shortcuts()}
            onClick={() => {
              ui.setHelpOpen(true);
            }}
            shortcut={["?"]}
          />
        </span>
        {/* Touch screens: the palette (⌘K) without a keyboard. A lightning
            bolt ("quick actions"), not ⌘: Android and Windows have no ⌘. */}
        {palette ? (
          <span {...stylex.props(styles.touchOnly)}>
            <IconButton
              aria-haspopup="dialog"
              icon={Zap}
              label={m.palette_open()}
              onClick={() => {
                ui.setPaletteOpen(true);
              }}
            />
          </span>
        ) : null}
        <span {...stylex.props(settingsOnMobile ? null : styles.hideOnMobile)}>
          <SettingsPopover />
        </span>
        <AccountMenu />
      </div>
    </div>
  );
};
