// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings' own frame inside the shared shell (shell.tsx), Linear-style:
// the sidebar lists the sections (with a way back to mail), the top bar
// names the page; no mail folders, no "New message", no mail search.

import { IconButton, NavItem } from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import { Link, useLocation } from "@tanstack/react-router";
import {
  ArrowLeft,
  Filter,
  PanelLeftClose,
  Plane,
  SlidersHorizontal,
} from "lucide-react";

import { m } from "../../paraglide/messages.js";
import { AppBar, AppBarTitle } from "../mail/app-bar";
import { useMailUi } from "../mail/mail-ui";
import type { ShellSidebar } from "../mail/shell";
import { sidebarLayout } from "../mail/sidebar";
import { RailItem, SidebarRail } from "../mail/sidebar-rail";
import { SidebarTop } from "../mail/sidebar-top";

export const SETTINGS_SECTIONS = [
  {
    icon: SlidersHorizontal,
    label: m.settings_section_general,
    to: "/settings/general",
  },
  { icon: Filter, label: m.settings_section_filters, to: "/settings/filters" },
  { icon: Plane, label: m.settings_section_vacation, to: "/settings/vacation" },
] as const;

const SettingsSidebarView = ({
  collapsed,
  onNavigate,
  onToggleCollapsed,
}: Parameters<ShellSidebar>[0]) => {
  const { pathname } = useLocation();
  if (collapsed) {
    return (
      <SidebarRail label={m.settings_sections()} onExpand={onToggleCollapsed}>
        <RailItem
          icon={ArrowLeft}
          label={m.cal_back_to_mail()}
          render={(props) => (
            <Link
              {...props}
              onClick={onNavigate}
              params={{ mailbox: "inbox" }}
              to="/mail/$mailbox"
            />
          )}
        />
        {SETTINGS_SECTIONS.map((section) => (
          <RailItem
            active={pathname.startsWith(section.to)}
            icon={section.icon}
            key={section.to}
            label={section.label()}
            render={(props) => (
              <Link {...props} onClick={onNavigate} to={section.to} />
            )}
          />
        ))}
      </SidebarRail>
    );
  }
  return (
    <nav
      aria-label={m.settings_sections()}
      {...stylex.props(sidebarLayout.root)}
    >
      {/* "Back to mail" takes the primary button's place, so the sections
          start where every other module's first item does; the top bar
          already says "Settings". */}
      <SidebarTop
        current="settings"
        onNavigate={onNavigate}
        xstyle={sidebarLayout.compose}
      >
        <NavItem
          icon={ArrowLeft}
          render={
            <Link
              onClick={onNavigate}
              params={{ mailbox: "inbox" }}
              to="/mail/$mailbox"
            />
          }
        >
          {m.cal_back_to_mail()}
        </NavItem>
      </SidebarTop>
      <div {...stylex.props(sidebarLayout.nav)}>
        {SETTINGS_SECTIONS.map((section) => (
          <NavItem
            active={pathname.startsWith(section.to)}
            icon={section.icon}
            key={section.to}
            render={<Link onClick={onNavigate} to={section.to} />}
          >
            {section.label()}
          </NavItem>
        ))}
      </div>
      <span {...stylex.props(sidebarLayout.spacer)} />
      {onToggleCollapsed === undefined ? null : (
        <div {...stylex.props(sidebarLayout.foot)}>
          <IconButton
            icon={PanelLeftClose}
            label={m.sidebar_collapse()}
            onClick={onToggleCollapsed}
            tooltipSide="right"
          />
        </div>
      )}
    </nav>
  );
};

export const settingsSidebar: ShellSidebar = (props) => (
  <SettingsSidebarView {...props} />
);

export const SettingsBar = () => {
  const ui = useMailUi();
  const { pathname } = useLocation();
  // Like mail's bar naming the folder: the section you are in (the
  // sidebar already says these are the settings).
  const section = SETTINGS_SECTIONS.find((item) =>
    pathname.startsWith(item.to)
  );
  return (
    <AppBar
      menuLabel={m.menu_open()}
      onMenu={() => {
        ui.setDrawerOpen(true);
      }}
      start={
        <AppBarTitle>
          {section === undefined ? m.settings_page_title() : section.label()}
        </AppBarTitle>
      }
    />
  );
};
