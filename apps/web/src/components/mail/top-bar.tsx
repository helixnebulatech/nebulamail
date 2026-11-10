// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Mail's top bar: the shared app bar with the folder's name, the mail
// search in the centre and the privacy shield (count of blocked
// trackers; a click opens what protection does) among the module's
// controls.

import { Button, Popover, PrivacyShield, ShieldGlyph } from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  useMatchRoute,
  useNavigate,
  useParams,
  useSearch,
} from "@tanstack/react-router";

import { useAccountMailboxes } from "../../mail/hooks";
import type { ImagePolicy } from "../../mail/image-policy";
import { useImageSettings } from "../../mail/use-image-policy";
import { resolveView } from "../../mail/views";
import { m } from "../../paraglide/messages.js";
import { AppBar, AppBarTitle } from "./app-bar";
import { bp } from "./breakpoints.stylex";
import { useMailUi } from "./mail-ui";
import { mailboxLabel } from "./mailbox-label";
import { SearchBox } from "./search-box";
import { specialLabel } from "./sidebar";
import { shared } from "./styles";

const styles = stylex.create({
  // Always one icon of the bar's size (the bar never grows over the
  // search); a click opens what protection did. Something blocked: the
  // shield in the accent colour with a count.
  shield: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    color: { ":hover": colors.fg2, default: colors.fg3 },
    cursor: "pointer",
    // Phones need the room for search; each open email has its own
    // privacy bar.
    display: { default: "flex", [bp.mobile]: "none" },
    flexShrink: 0,
    height: { default: "34px", [touch.coarse]: touch.target },
    justifyContent: "center",
    position: "relative",
    width: { default: "34px", [touch.coarse]: touch.target },
  },
  shieldActive: { color: colors.accText },
  count: {
    backgroundColor: colors.acc,
    borderColor: colors.canvas,
    borderRadius: radius.pill,
    borderStyle: "solid",
    borderWidth: "2px",
    boxSizing: "content-box",
    color: colors.accFg,
    fontSize: "10px",
    fontVariantNumeric: "tabular-nums",
    fontWeight: font.weightSemibold,
    height: "14px",
    insetInlineEnd: "-1px",
    lineHeight: "14px",
    minWidth: "14px",
    paddingInline: "3px",
    position: "absolute",
    textAlign: "center",
    top: "-1px",
  },
  item: { display: "flex", flexDirection: "column", rowGap: "2px" },
  itemTitle: {
    color: colors.fg,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  itemBody: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    lineHeight: 1.45,
    margin: 0,
  },
  items: { display: "flex", flexDirection: "column", rowGap: "12px" },
});

const LOAD_LABEL: Record<ImagePolicy["load"], () => string> = {
  always: m.settings_images_always,
  ask: m.settings_images_ask,
  never: m.settings_images_never,
  "trusted-senders": m.settings_images_trusted,
};

/** What happens to remote images with the user's settings, in one line. */
const imagesLine = (policy: ImagePolicy | null): string | null => {
  if (policy === null) {
    return null;
  }
  if (policy.load === "never" || policy.route === null) {
    return m.privacy_report_images_never();
  }
  const route =
    policy.route === "proxy"
      ? m.shield_panel_images_proxy()
      : m.shield_panel_images_direct();
  return `${LOAD_LABEL[policy.load]()} · ${route}`;
};

const Item = ({
  body,
  title,
}: {
  readonly body: string;
  readonly title: string;
}) => (
  <div {...stylex.props(styles.item)}>
    <p {...stylex.props(styles.itemTitle)}>{title}</p>
    <p {...stylex.props(styles.itemBody)}>{body}</p>
  </div>
);

const Protection = () => {
  const ui = useMailUi();
  const navigate = useNavigate();
  const { policy } = useImageSettings();
  const quiet = ui.blocked === 0;
  const images = imagesLine(policy);
  return (
    <Popover.Root>
      <Popover.Trigger
        render={
          <button
            aria-label={m.shield_label({ count: ui.blocked })}
            type="button"
            {...stylex.props(
              shared.resetButton,
              shared.focusRing,
              styles.shield,
              quiet ? null : styles.shieldActive
            )}
          >
            <PrivacyShield pulse={ui.blocked} quiet={quiet} size={18} />
            {quiet ? null : (
              <span aria-hidden {...stylex.props(styles.count)}>
                {ui.blocked > 99 ? "99+" : ui.blocked}
              </span>
            )}
          </button>
        }
      />
      <Popover.Content variant="accent">
        <Popover.Header icon={<ShieldGlyph size={22} />}>
          <Popover.Title>{m.shield_active()}</Popover.Title>
          <Popover.Description>
            {quiet
              ? m.shield_panel_none()
              : m.shield_panel_blocked({ count: ui.blocked })}
          </Popover.Description>
        </Popover.Header>
        <Popover.Section>
          <div {...stylex.props(styles.items)}>
            <Item
              body={m.shield_panel_trackers_body()}
              title={m.shield_panel_trackers_title()}
            />
            <Item
              body={m.shield_panel_links_body()}
              title={m.shield_panel_links_title()}
            />
            {images === null ? null : (
              <Item body={images} title={m.shield_panel_images_title()} />
            )}
          </div>
        </Popover.Section>
        <Popover.Section>
          <Button
            onClick={() => {
              void navigate({ to: "/settings/general" });
            }}
            size="sm"
            variant="secondary"
          >
            {m.shield_panel_settings()}
          </Button>
        </Popover.Section>
      </Popover.Content>
    </Popover.Root>
  );
};

/** The folder (or search, or Scheduled) the list shows. */
const useMailTitle = (): string => {
  const params = useParams({ strict: false });
  const search: { readonly q?: string } = useSearch({ strict: false });
  const matchRoute = useMatchRoute();
  const { data: mailboxes } = useAccountMailboxes();
  if (matchRoute({ to: "/mail/scheduled" }) !== false) {
    return m.nav_scheduled();
  }
  const query = search.q ?? "";
  if (query !== "") {
    return m.search_title();
  }
  const segment = params.mailbox ?? "inbox";
  const view = resolveView(segment, mailboxes);
  if (view?.kind === "mailbox") {
    return mailboxLabel(view.mailbox);
  }
  if (view?.kind === "missing") {
    return specialLabel(view.role) ?? "";
  }
  return specialLabel(segment) ?? "";
};

export const TopBar = () => {
  const ui = useMailUi();
  const title = useMailTitle();
  return (
    <AppBar
      extras={<Protection />}
      start={<AppBarTitle hideOnMobile>{title}</AppBarTitle>}
      menuLabel={m.menu_open()}
      onMenu={() => {
        ui.setDrawerOpen(true);
      }}
      search={<SearchBox />}
    />
  );
};
