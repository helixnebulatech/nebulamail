// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The full settings page (/settings/…): one panel with the open section.
// The sections are in the settings sidebar (settings-shell.tsx); phones,
// whose sidebar is a drawer, get them as a row of tabs on top.

import { NavItem, Panel } from "@nebulamail/ui";
import { colors } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link, useLocation } from "@tanstack/react-router";
import type { ReactNode } from "react";

import { m } from "../../paraglide/messages.js";
import { bp } from "../mail/breakpoints.stylex";
import { shared } from "../mail/styles";
import { settingsMeasure } from "./measure.stylex";
import { SETTINGS_SECTIONS } from "./settings-shell";

const styles = stylex.create({
  panel: {
    display: "grid",
    flexGrow: 1,
    gridTemplateRows: {
      default: "minmax(0, 1fr)",
      [bp.mobile]: "auto minmax(0, 1fr)",
    },
    minWidth: 0,
  },
  nav: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "2px",
    display: { default: "none", [bp.mobile]: "flex" },
    overflowX: "auto",
    padding: "8px",
  },
  navItem: { flexShrink: 0 },
  content: {
    minHeight: 0,
    minWidth: 0,
    overflowY: "auto",
    padding: { default: "24px 28px 40px", [bp.mobile]: "16px 14px 96px" },
  },
  // The reading column, centred in the panel: no wide empty band on the
  // end side of a 1440px window.
  column: {
    marginInline: "auto",
    maxWidth: settingsMeasure.width,
    width: "100%",
  },
});

export const SettingsPage = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const { pathname } = useLocation();
  return (
    <Panel label={m.settings_page_title()} xstyle={styles.panel}>
      {/* The top bar shows the name; this keeps the page's outline. */}
      <h1 {...stylex.props(shared.srOnly)}>{m.settings_page_title()}</h1>
      <nav aria-label={m.settings_sections()} {...stylex.props(styles.nav)}>
        {SETTINGS_SECTIONS.map((section) => (
          <span key={section.to} {...stylex.props(styles.navItem)}>
            <NavItem
              active={pathname.startsWith(section.to)}
              icon={section.icon}
              render={<Link to={section.to} />}
              surface="panel"
            >
              {section.label()}
            </NavItem>
          </span>
        ))}
      </nav>
      <div {...stylex.props(styles.content)}>
        <div {...stylex.props(styles.column)}>{children}</div>
      </div>
    </Panel>
  );
};
