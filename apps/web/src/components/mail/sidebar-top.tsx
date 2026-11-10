// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The top of every module's sidebar (mail, calendar, tasks, contacts,
// settings): the brand,
// the app switcher where the right-hand rail is hidden, and the module's
// primary button. One component, so the brand and the button sit at the
// same pixels in every module and nothing jumps when switching.

import { IconButton, Mark, Menu, MenuItem, Wordmark } from "@nebulamail/ui";
import { colors, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link, useMatchRoute, useNavigate } from "@tanstack/react-router";
import { CalendarDays, LayoutGrid, ListTodo, Mail, Users } from "lucide-react";
import type { ReactNode } from "react";

import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { shared } from "./styles";

const styles = stylex.create({
  root: {
    display: "flex",
    flexDirection: "column",
    flexShrink: 0,
    rowGap: "10px",
  },
  head: {
    alignItems: "center",
    display: "flex",
    height: "36px",
    paddingInlineEnd: "4px",
  },
  brand: {
    alignItems: "center",
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "8px",
    display: "flex",
    height: "36px",
    paddingInline: "6px",
    textDecoration: "none",
  },
  spacer: { flexGrow: 1 },
  // The primary button's place, 44px in every module (settings puts its
  // "Back to mail" here), so the first item below starts at the same y.
  slot: {
    display: "flex",
    flexDirection: "column",
    height: "44px",
    justifyContent: "center",
  },
  // Only where the right-hand rail is hidden (narrow windows, phones):
  // with the rail on screen it would be a second way to the same places.
  apps: { display: { default: "none", [bp.narrow]: "inline-flex" } },
});

export type AppModule = "mail" | "calendar" | "tasks" | "contacts" | "settings";

const APPS = [
  { icon: Mail, key: "mail", label: m.rail_mail, to: "/mail" },
  {
    icon: CalendarDays,
    key: "calendar",
    label: m.rail_calendar,
    to: "/calendar",
  },
  { icon: ListTodo, key: "tasks", label: m.rail_tasks, to: "/tasks" },
  { icon: Users, key: "contacts", label: m.rail_contacts, to: "/contacts" },
] as const;

const useCurrentModule = (): AppModule => {
  const matchRoute = useMatchRoute();
  for (const app of APPS) {
    if (matchRoute({ fuzzy: true, to: app.to }) !== false) {
      return app.key;
    }
  }
  return "settings";
};

/**
 * The other modules, in a menu: where the right-hand rail is hidden
 * (narrow windows, the icon rail of compact ones).
 */
export const AppsMenu = ({
  current: given,
  onNavigate,
  side = "bottom",
}: {
  /** The module on screen (left out of the menu); from the URL if omitted. */
  readonly current?: AppModule;
  readonly onNavigate?: (() => void) | undefined;
  /** Where the menu opens (the icon rail opens it to the right). */
  readonly side?: "bottom" | "right";
}) => {
  const navigate = useNavigate();
  const fromUrl = useCurrentModule();
  const current = given ?? fromUrl;
  return (
    <Menu
      align="end"
      side={side}
      trigger={
        <IconButton
          icon={LayoutGrid}
          label={m.apps_label()}
          size="sm"
          {...(side === "right" ? { tooltipSide: "right" as const } : {})}
        />
      }
    >
      {APPS.filter((app) => app.key !== current).map((app) => (
        <MenuItem
          icon={app.icon}
          key={app.key}
          onClick={() => {
            onNavigate?.();
            void navigate({ to: app.to });
          }}
        >
          {app.label()}
        </MenuItem>
      ))}
    </Menu>
  );
};

export const SidebarTop = ({
  children,
  current,
  onNavigate,
  xstyle,
}: {
  /** Room below, as each sidebar needs. */
  readonly xstyle?: stylex.StyleXStyles;
  /** The module's primary button ("New message", "New event"…). */
  readonly children?: ReactNode;
  readonly current: AppModule;
  /** Closes the phone drawer after a link. */
  readonly onNavigate?: (() => void) | undefined;
}) => (
  <div {...stylex.props(styles.root, xstyle)}>
    <div {...stylex.props(styles.head)}>
      <Link
        aria-label={m.app_name()}
        onClick={onNavigate}
        params={{ mailbox: "inbox" }}
        to="/mail/$mailbox"
        {...stylex.props(styles.brand, shared.focusRing)}
      >
        <Mark label={null} size={22} />
        <Wordmark />
      </Link>
      <span {...stylex.props(styles.spacer)} />
      <span {...stylex.props(styles.apps)}>
        <AppsMenu current={current} onNavigate={onNavigate} />
      </span>
    </div>
    {children === undefined ? null : (
      <div {...stylex.props(styles.slot)}>{children}</div>
    )}
  </div>
);
