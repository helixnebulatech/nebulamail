// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The avatar menu at the end of every module's top bar (app-bar.tsx):
// who is signed in, all settings, the shortcuts, sign out.

import {
  AlertDialog,
  Avatar,
  Menu,
  MenuGroup,
  MenuItem,
  MenuSeparator,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import { ChevronDown, Keyboard, LogOut, Settings } from "lucide-react";
import { useState } from "react";

import { logout } from "../../lib/auth";
import { getClient, resetClient } from "../../mail/client";
import { forgetDeviceData } from "../../mail/device";
import { useAccount } from "../../mail/hooks";
import { listScheduled } from "../../mail/schedule";
import { canSendSignedOut, waitingForSignIn } from "../../mail/send-signed-out";
import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useMailUi } from "./mail-ui";
import { shared } from "./styles";

/** The avatar's place while the account loads (Avatar's size and radius). */
const avatarSkeleton = stylex.create({
  root: {
    backgroundColor: colors.hover,
    borderRadius: "10px",
    flexShrink: 0,
    height: "30px",
    width: "30px",
  },
});

const styles = stylex.create({
  account: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    columnGap: "9px",
    display: "flex",
    justifyContent: "center",
    minHeight: { default: null, [touch.coarse]: touch.target },
    minWidth: { default: null, [touch.coarse]: touch.target },
    paddingBlock: "4px",
    paddingInlineEnd: { default: "8px", [bp.narrow]: "4px" },
    paddingInlineStart: "4px",
    whiteSpace: "nowrap",
  },
  // Lighter than the page title at the bar's start: the short name in
  // fg2, regular weight (the full address is in the menu and the label).
  accountText: {
    color: colors.fg2,
    display: { default: "block", [bp.compact]: "none", [bp.narrow]: "none" },
    fontSize: font.sizeMd,
    fontWeight: font.weightNormal,
    lineHeight: 1.2,
    maxWidth: "120px",
  },
  chevron: {
    color: colors.fg3,
    display: { default: "block", [bp.narrow]: "none" },
  },
  menuHead: {
    display: "flex",
    flexDirection: "column",
    rowGap: "2px",
  },
  menuHeadName: {
    color: colors.fg,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    maxWidth: "240px",
  },
});

const signOut = async (): Promise<void> => {
  resetClient();
  await forgetDeviceData().catch(() => null);
  try {
    await logout();
  } finally {
    // Leave even if that failed: /login shows whether a session is left.
    // Under the base path: the static build lives at /nebulamail/
    // where "/login" would be Stalwart's own page.
    globalThis.location.assign(`${import.meta.env.BASE_URL}login`);
  }
};

/**
 * How many scheduled messages would wait for the next sign-in (on a server
 * that can send them after sign-out, mail/send-signed-out.ts); 0 if none
 * or if it can't be known (signing out must never get stuck).
 */
const waitingCount = async (): Promise<number> => {
  try {
    if (!canSendSignedOut(await getClient())) {
      return 0;
    }
    return waitingForSignIn(await listScheduled()).length;
  } catch {
    return 0;
  }
};

/** The avatar menu, on every module's top bar. */
export const AccountMenu = () => {
  const { data } = useAccount();
  const ui = useMailUi();
  const navigate = useNavigate();
  const username = data?.username ?? "";
  const display = username.split("@")[0] ?? username;
  // Scheduled messages that would wait for the next sign-in: asked first.
  const [waiting, setWaiting] = useState(0);
  const leave = async (): Promise<void> => {
    const count = await waitingCount();
    if (count > 0) {
      setWaiting(count);
      return;
    }
    await signOut();
  };
  return (
    <>
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.logout_button()}
        description={m.logout_scheduled_waiting({ count: waiting })}
        onConfirm={() => {
          setWaiting(0);
          void signOut();
        }}
        onOpenChange={(open) => {
          if (!open) {
            setWaiting(0);
          }
        }}
        open={waiting > 0}
        title={m.logout_scheduled_title()}
      />
      <Menu
        align="end"
        trigger={
          <button
            aria-label={m.account_label({ username })}
            type="button"
            {...stylex.props(
              shared.resetButton,
              styles.account,
              shared.focusRing
            )}
          >
            {/* While the session loads: a quiet placeholder, not «?». */}
            {display === "" ? (
              <span aria-hidden {...stylex.props(avatarSkeleton.root)} />
            ) : (
              <Avatar name={display} size={30} />
            )}
            <span {...stylex.props(styles.accountText, shared.ellipsis)}>
              {display}
            </span>
            <ChevronDown
              aria-hidden
              size={14}
              {...stylex.props(styles.chevron)}
            />
          </button>
        }
      >
        <MenuGroup
          label={
            <span {...stylex.props(styles.menuHead)}>
              {m.account_signed_in_as()}
              <span {...stylex.props(styles.menuHeadName, shared.ellipsis)}>
                {username}
              </span>
            </span>
          }
        >
          <MenuItem
            icon={Settings}
            onClick={() => {
              void navigate({ to: "/settings/general" });
            }}
          >
            {m.settings_all()}
          </MenuItem>
          <MenuItem
            icon={Keyboard}
            onClick={() => {
              ui.setHelpOpen(true);
            }}
            shortcut={["?"]}
          >
            {m.help_shortcuts()}
          </MenuItem>
        </MenuGroup>
        <MenuSeparator />
        <MenuItem
          icon={LogOut}
          onClick={() => {
            void leave();
          }}
        >
          {m.logout_button()}
        </MenuItem>
      </Menu>
    </>
  );
};
