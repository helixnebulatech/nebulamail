// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Proton-style sidebar: brand, "New message", special folders with
// unread counts, "More", Folders and Labels. Collapses into a Zero-style
// icon rail; the choice is remembered on this device (UI only).

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  ComposeButton,
  IconButton,
  MenuItem,
  MenuSeparator,
  NavItem,
} from "@nebulamail/ui";
import type { Icon } from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import {
  Link,
  useMatchRoute,
  useNavigate,
  useParams,
} from "@tanstack/react-router";
import {
  Archive,
  ChevronDown,
  ChevronUp,
  Clock,
  FileText,
  Folder,
  Inbox,
  OctagonAlert,
  PanelLeftClose,
  PenLine,
  Send,
  Star,
  Trash2,
  Users,
} from "lucide-react";
import { Fragment, useState } from "react";

import type { MailboxRole, MailboxSummary } from "../../mail/api";
import { useMailboxes } from "../../mail/hooks";
import { useScheduleLimit } from "../../mail/use-schedule";
import { STARRED, mailboxParam } from "../../mail/views";
import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useComposer } from "./compose-context";
import {
  FolderLabel,
  QuotaMeter,
  SharedWithMe,
  ShareFolderMenu,
  ShareFolderProvider,
} from "./shared-folders";
import { RailItem, RailMenu, SidebarRail } from "./sidebar-rail";
import { SidebarSection, sidebarText } from "./sidebar-section";
import { SidebarTop } from "./sidebar-top";
import { useKeys } from "./use-apple";

/** Shared by the contacts and settings sidebars, so all three line up. */
export const sidebarLayout = stylex.create({
  compose: { marginBottom: "12px" },
  // The same 14px under the primary button in sidebars laid out with a
  // 16px row gap (calendar, tasks) as in mail's (12px + its 2px gap).
  composeInGap16: { marginBottom: "-2px" },
  foot: { display: "flex", justifyContent: "flex-start", paddingTop: "8px" },
  nav: { display: "flex", flexDirection: "column", rowGap: "1px" },
  root: {
    display: "flex",
    flexDirection: "column",
    height: "100%",
    minHeight: 0,
    overflowY: "auto",
    paddingBottom: "14px",
    paddingInline: "12px",
    paddingTop: "12px",
    rowGap: "2px",
  },
  section: {
    alignItems: "center",
    display: "flex",
    marginTop: "12px",
    paddingInlineEnd: "4px",
    paddingInlineStart: "6px",
  },
  spacer: { flexGrow: 1 },
});

const styles = stylex.create({
  // Only where the right-hand rail is hidden (shell.tsx): with the rail on
  // screen it would be a second way to the same place.
  railHidden: {
    display: { default: "none", [bp.narrow]: "flex" },
    flexDirection: "column",
    rowGap: "2px",
  },
});

interface Special {
  readonly key: MailboxRole | typeof STARRED;
  readonly icon: Icon;
  readonly label: () => string;
  /** Count shown: unread, all (drafts), or none. */
  readonly count: "unread" | "total" | "none";
  readonly more: boolean;
  readonly shortcut?: string;
}

const SPECIAL: readonly Special[] = [
  {
    count: "unread",
    icon: Inbox,
    key: "inbox",
    label: m.nav_inbox,
    more: false,
    shortcut: "g i",
  },
  {
    count: "total",
    icon: FileText,
    key: "drafts",
    label: m.nav_drafts,
    more: false,
    shortcut: "g d",
  },
  {
    count: "none",
    icon: Send,
    key: "sent",
    label: m.nav_sent,
    more: false,
    shortcut: "g t",
  },
  {
    count: "none",
    icon: Star,
    key: STARRED,
    label: m.nav_starred,
    more: false,
    shortcut: "g s",
  },
  {
    count: "none",
    icon: Archive,
    key: "archive",
    label: m.nav_archive,
    more: true,
  },
  {
    count: "unread",
    icon: OctagonAlert,
    key: "junk",
    label: m.nav_junk,
    more: true,
  },
  { count: "none", icon: Trash2, key: "trash", label: m.nav_trash, more: true },
];

const countFor = (
  special: Special,
  mailboxes: readonly MailboxSummary[] | undefined
): number | undefined => {
  const mailbox = mailboxes?.find((box) => box.role === special.key);
  switch (special.count) {
    case "unread": {
      return mailbox?.unread;
    }
    case "total": {
      return mailbox?.total;
    }
    case "none": {
      return undefined;
    }
    default: {
      return special.count satisfies never;
    }
  }
};

/** Name of a special folder, translated (server names may be English). */
export const specialLabel = (key: string): string | null =>
  SPECIAL.find((special) => special.key === key)?.label() ?? null;

/** Special folders translated, others cleaned (also for shared accounts). */
const folderName = (mailbox: MailboxSummary): string =>
  (mailbox.role === null ? null : specialLabel(mailbox.role)) ??
  cleanDisplayText(mailbox.name);

export const SPECIAL_ICONS: Readonly<Record<string, Icon>> = Object.fromEntries(
  SPECIAL.map((special) => [special.key, special.icon])
);

export interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed?: () => void;
  /** Called after navigating (closes the mobile drawer). */
  readonly onNavigate?: () => void;
}

export const Sidebar = ({
  collapsed,
  onNavigate,
  onToggleCollapsed,
}: SidebarProps) => {
  const { data: mailboxes } = useMailboxes();
  const composer = useComposer();
  const keys = useKeys();
  const params = useParams({ strict: false });
  const matchRoute = useMatchRoute();
  const navigate = useNavigate();
  const inContacts = matchRoute({ fuzzy: true, to: "/contacts" }) !== false;
  const inScheduled = matchRoute({ to: "/mail/scheduled" }) !== false;
  const inMail = matchRoute({ fuzzy: true, to: "/mail" }) !== false;
  // «Programados» only when the server can hold messages (FUTURERELEASE).
  const canSchedule = useScheduleLimit() !== null;
  // Outside a mail folder (contacts, settings, scheduled), none is current.
  const current = params.mailbox ?? (inMail && !inScheduled ? "inbox" : "");
  const [showMore, setShowMore] = useState(false);
  const [foldersOpen, setFoldersOpen] = useState(true);
  const moreActive = SPECIAL.some(
    (special) => special.more && special.key === current
  );
  const folders = (mailboxes ?? []).filter((box) => box.role === null);
  const visible = SPECIAL.filter(
    (special) => !special.more || showMore || moreActive
  );

  const compose = () => {
    onNavigate?.();
    composer.open();
  };

  if (collapsed) {
    return (
      <ShareFolderProvider
        current={current}
        folderName={folderName}
        mailboxes={mailboxes}
      >
        <SidebarRail
          label={m.nav_label()}
          onExpand={onToggleCollapsed}
          primary={
            <ComposeButton
              icon={PenLine}
              collapsed
              onClick={compose}
              shortcut={keys("c")}
            >
              {m.compose_new()}
            </ComposeButton>
          }
        >
          {/* The same places in the same order as the full sidebar:
              the main folders, Scheduled right after Sent, then one
              entry for the rest ("More" and the user's folders). */}
          {SPECIAL.filter((special) => !special.more).map((special) => {
            const count = countFor(special, mailboxes);
            return (
              <Fragment key={special.key}>
                <RailItem
                  active={current === special.key}
                  badge={special.count === "unread" ? count : undefined}
                  icon={special.icon}
                  label={special.label()}
                  render={(props) => (
                    <Link
                      {...props}
                      onClick={onNavigate}
                      params={{ mailbox: special.key }}
                      to="/mail/$mailbox"
                    />
                  )}
                  shortcut={
                    special.shortcut === undefined
                      ? undefined
                      : keys(special.shortcut)
                  }
                />
                {special.key === "sent" && canSchedule ? (
                  <RailItem
                    active={inScheduled}
                    icon={Clock}
                    label={m.nav_scheduled()}
                    render={(props) => (
                      <Link
                        {...props}
                        onClick={onNavigate}
                        to="/mail/scheduled"
                      />
                    )}
                  />
                ) : null}
              </Fragment>
            );
          })}
          <RailMenu
            active={
              moreActive ||
              folders.some((folder) => current === mailboxParam(folder))
            }
            icon={Folder}
            label={m.nav_folders()}
          >
            {SPECIAL.filter((special) => special.more).map((special) => (
              <MenuItem
                icon={special.icon}
                key={special.key}
                onClick={() => {
                  onNavigate?.();
                  void navigate({
                    params: { mailbox: special.key },
                    to: "/mail/$mailbox",
                  });
                }}
              >
                {special.label()}
              </MenuItem>
            ))}
            {folders.length > 0 ? <MenuSeparator /> : null}
            {folders.map((folder) => (
              <MenuItem
                icon={Folder}
                key={folder.id}
                onClick={() => {
                  onNavigate?.();
                  void navigate({
                    params: { mailbox: mailboxParam(folder) },
                    to: "/mail/$mailbox",
                  });
                }}
              >
                {folderName(folder)}
              </MenuItem>
            ))}
          </RailMenu>
        </SidebarRail>
      </ShareFolderProvider>
    );
  }

  return (
    <ShareFolderProvider
      current={current}
      folderName={folderName}
      mailboxes={mailboxes}
    >
      <nav aria-label={m.nav_label()} {...stylex.props(sidebarLayout.root)}>
        <SidebarTop
          current="mail"
          onNavigate={onNavigate}
          xstyle={sidebarLayout.compose}
        >
          <ComposeButton icon={PenLine} onClick={compose} shortcut={keys("c")}>
            {m.compose_new()}
          </ComposeButton>
        </SidebarTop>
        <div {...stylex.props(sidebarLayout.nav)}>
          {visible.map((special) => {
            const count = countFor(special, mailboxes);
            return (
              <Fragment key={special.key}>
                <NavItem
                  active={current === special.key}
                  icon={special.icon}
                  mutedCount={special.count === "total"}
                  render={
                    <Link
                      onClick={onNavigate}
                      params={{ mailbox: special.key }}
                      to="/mail/$mailbox"
                    />
                  }
                  {...(count === undefined ? {} : { count })}
                >
                  {special.label()}
                </NavItem>
                {/* Right after Sent: what is about to be sent. */}
                {special.key === "sent" && canSchedule ? (
                  <NavItem
                    active={inScheduled}
                    icon={Clock}
                    render={<Link onClick={onNavigate} to="/mail/scheduled" />}
                  >
                    {m.nav_scheduled()}
                  </NavItem>
                ) : null}
              </Fragment>
            );
          })}
          {moreActive ? null : (
            <NavItem
              icon={showMore ? ChevronUp : ChevronDown}
              onClick={() => {
                setShowMore((value) => !value);
              }}
            >
              {showMore ? m.nav_less() : m.nav_more()}
            </NavItem>
          )}
        </div>
        <SidebarSection
          onToggle={() => {
            setFoldersOpen((value) => !value);
          }}
          open={foldersOpen}
        >
          {m.nav_folders()}
        </SidebarSection>
        {foldersOpen ? (
          <div {...stylex.props(sidebarLayout.nav)}>
            {/* While loading, nothing: "No folders yet" would be wrong. */}
            {mailboxes !== undefined && folders.length === 0 ? (
              <p {...stylex.props(sidebarText.text)}>{m.nav_folders_empty()}</p>
            ) : (
              folders.map((folder) => (
                <ShareFolderMenu key={folder.id} mailbox={folder}>
                  <NavItem
                    active={current === mailboxParam(folder)}
                    icon={Folder}
                    render={
                      <Link
                        onClick={onNavigate}
                        params={{ mailbox: mailboxParam(folder) }}
                        to="/mail/$mailbox"
                      />
                    }
                    {...(folder.unread > 0 ? { count: folder.unread } : {})}
                  >
                    <FolderLabel mailbox={folder} />
                  </NavItem>
                </ShareFolderMenu>
              ))
            )}
          </div>
        ) : null}
        {/* No «Labels» section: the app can't create or show labels yet,
            and a permanent «No labels yet» read as an unfinished feature. */}
        <SharedWithMe
          current={current}
          folderName={folderName}
          onNavigate={onNavigate}
        />
        {/* Contacts is in the right-hand rail; only where the rail hides
            (narrow windows, phones) is it listed here. */}
        <div {...stylex.props(styles.railHidden)}>
          <div {...stylex.props(sidebarLayout.section)} />
          <NavItem
            active={inContacts}
            icon={Users}
            render={<Link onClick={onNavigate} to="/contacts" />}
          >
            {m.rail_contacts()}
          </NavItem>
        </div>
        <span {...stylex.props(sidebarLayout.spacer)} />
        <QuotaMeter />
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
    </ShareFolderProvider>
  );
};
