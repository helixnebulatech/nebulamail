// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The right app rail (Gmail): mail, calendar, tasks (they live in the
// calendars) and contacts (when the server has them), and the shortcut help.

import { CAPABILITIES, hasCapability } from "@nebulamail/jmap-client";
import { IconButton, Tooltip } from "@nebulamail/ui";
import { colors, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { Link, useMatchRoute } from "@tanstack/react-router";
import { Calendar, Keyboard, ListTodo, Mail, Users } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { getClient } from "../../mail/client";
import { m } from "../../paraglide/messages.js";
import { useMailUi } from "./mail-ui";
import { shared } from "./styles";

const styles = stylex.create({
  app: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    color: { ":hover": colors.fg, default: colors.fg2 },
    display: "grid",
    height: "36px",
    justifyContent: "center",
    minHeight: { default: null, [touch.coarse]: touch.target },
    minWidth: { default: null, [touch.coarse]: touch.target },
    width: "36px",
  },
  appActive: {
    // Same as the sidebar's current item (NavItem): the selected fill.
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    color: { ":hover": colors.accText, default: colors.accText },
  },
  rail: {
    alignItems: "center",
    borderInlineStartColor: colors.line,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: "1px",
    display: "flex",
    flexDirection: "column",
    height: "100%",
    paddingBlock: "14px",
    rowGap: "6px",
  },
  sep: {
    borderStyle: "none",
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    marginBlock: "4px",
    width: "22px",
  },
});

const RailLink = ({
  active,
  icon: Icon,
  label,
  to,
}: {
  readonly active: boolean;
  readonly icon: LucideIcon;
  readonly label: string;
  readonly to: "/mail" | "/calendar" | "/tasks" | "/contacts";
}) => (
  <Tooltip content={label} side="left">
    <Link
      aria-label={label}
      to={to}
      {...(active ? { "aria-current": "page" as const } : {})}
      {...stylex.props(
        styles.app,
        active ? styles.appActive : null,
        shared.focusRing
      )}
    >
      <Icon aria-hidden size={18} />
    </Link>
  </Tooltip>
);

/** Which apps the server has (the IMAP bridge has neither). */
const availableApps = async () => {
  const { session } = await getClient();
  return {
    calendar: hasCapability(session, CAPABILITIES.calendars),
    contacts: hasCapability(session, CAPABILITIES.contacts),
  };
};

export const AppRail = () => {
  const ui = useMailUi();
  const apps = useQuery({
    queryFn: availableApps,
    queryKey: ["session", "apps"],
    staleTime: Infinity,
  });
  const matchRoute = useMatchRoute();
  const inMail = matchRoute({ fuzzy: true, to: "/mail" }) !== false;
  const inCalendar = matchRoute({ fuzzy: true, to: "/calendar" }) !== false;
  const inTasks = matchRoute({ fuzzy: true, to: "/tasks" }) !== false;
  const inContacts = matchRoute({ fuzzy: true, to: "/contacts" }) !== false;
  // Until the session says otherwise, every app is drawn: the rail keeps
  // its shape while the page loads instead of growing icons afterwards.
  // Only a server without calendars or contacts (the IMAP bridge) loses
  // them once it answers.
  const calendar = apps.data?.calendar ?? true;
  const contacts = apps.data?.contacts ?? true;
  return (
    <aside aria-label={m.rail_label()} {...stylex.props(styles.rail)}>
      <RailLink active={inMail} icon={Mail} label={m.rail_mail()} to="/mail" />
      {calendar && (
        <RailLink
          active={inCalendar}
          icon={Calendar}
          label={m.rail_calendar()}
          to="/calendar"
        />
      )}
      {calendar && (
        <RailLink
          active={inTasks}
          icon={ListTodo}
          label={m.rail_tasks()}
          to="/tasks"
        />
      )}
      {contacts && (
        <RailLink
          active={inContacts}
          icon={Users}
          label={m.rail_contacts()}
          to="/contacts"
        />
      )}
      <hr {...stylex.props(styles.sep)} />
      <IconButton
        icon={Keyboard}
        label={m.help_shortcuts()}
        onClick={() => {
          ui.setHelpOpen(true);
        }}
        shortcut={["?"]}
      />
    </aside>
  );
};
