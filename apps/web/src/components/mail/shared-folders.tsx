// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sidebar additions: "Shared with me" (folders of other
// accounts in the session, mail:share), the storage bar (RFC 9425 quota)
// and the way to share one of our folders (context menu and ⌘K).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { ContextMenu, MenuItem, NavItem, ProgressBar } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { FolderOpen, Share2, Users } from "lucide-react";
import { createContext, use, useState } from "react";
import type { ReactElement, ReactNode } from "react";

import { formatBytes } from "../../lib/format";
import { parseMailboxParam } from "../../mail/account-scope";
import type { MailboxSummary } from "../../mail/api";
import { mailKeys } from "../../mail/queries";
import type { QuotaLevel } from "../../mail/quota";
import { getStorageQuota } from "../../mail/quota";
import { listSharedAccounts, sharingSupported } from "../../mail/sharing";
import { mailboxParam } from "../../mail/views";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { useCommands } from "./commands";
import { ShareDialog } from "./share-dialog";
import { SidebarSection, sidebarText } from "./sidebar-section";
import { shared } from "./styles";

const styles = stylex.create({
  // Lines up with the section heading's text, like "No labels yet".
  // A sharer's address over their folders (sidebar-section.tsx text).
  account: { paddingBlock: "6px 2px" },
  quota: { paddingBlock: "10px 4px", paddingInline: "6px" },
  quotaBlock: { display: "block", marginTop: "4px" },
  quotaFull: { color: colors.danger },
  quotaText: { color: colors.fg3, fontSize: font.sizeSm },
  quotaWarn: { color: colors.warn },
  nav: { display: "flex", flexDirection: "column", rowGap: "1px" },
  sharedIcon: { color: colors.fg3, flexShrink: 0, marginInlineStart: "6px" },
});

const sharingKey = [...mailKeys.all, "sharing-supported"] as const;
const sharedKey = [...mailKeys.mailboxes(), "shared-accounts"] as const;

const useSharing = () =>
  useQuery({
    queryFn: sharingSupported,
    queryKey: sharingKey,
    staleTime: Infinity,
  });

/** A folder's name for the UI; the sidebar translates special folders. */
export type FolderName = (mailbox: MailboxSummary) => string;

const plainName: FolderName = (mailbox) => cleanDisplayText(mailbox.name);

/** "Shared with me": each account's folders, by owner. */
export const SharedWithMe = ({
  current,
  folderName = plainName,
  onNavigate,
}: {
  readonly current: string;
  readonly folderName?: FolderName;
  readonly onNavigate?: (() => void) | undefined;
}) => {
  const [open, setOpen] = useState(true);
  const accounts = useQuery({
    queryFn: listSharedAccounts,
    queryKey: sharedKey,
    staleTime: 60_000,
  });
  const list = accounts.data ?? [];
  if (list.length === 0) {
    return null;
  }
  return (
    <>
      <SidebarSection
        onToggle={() => {
          setOpen((value) => !value);
        }}
        open={open}
      >
        {m.nav_shared()}
      </SidebarSection>
      {open
        ? list.map((account) => (
            <div key={account.accountId} {...stylex.props(styles.nav)}>
              <p
                {...stylex.props(
                  sidebarText.text,
                  styles.account,
                  shared.ellipsis
                )}
              >
                <bdi>{cleanDisplayText(account.name)}</bdi>
              </p>
              {account.mailboxes.map((mailbox) => {
                const param = mailboxParam(mailbox);
                return (
                  <NavItem
                    active={current === param}
                    icon={FolderOpen}
                    key={mailbox.id}
                    render={
                      <Link
                        onClick={onNavigate}
                        params={{ mailbox: param }}
                        to="/mail/$mailbox"
                      />
                    }
                    {...(mailbox.unread > 0 ? { count: mailbox.unread } : {})}
                  >
                    <bdi>{folderName(mailbox)}</bdi>
                  </NavItem>
                );
              })}
            </div>
          ))
        : null}
    </>
  );
};

/** The folder's name plus a "shared" mark when others can see it. */
export const FolderLabel = ({
  mailbox,
}: {
  readonly mailbox: MailboxSummary;
}) => (
  <>
    <bdi>{plainName(mailbox)}</bdi>
    {mailbox.sharedWith > 0 ? (
      <>
        <Share2 aria-hidden size={12} {...stylex.props(styles.sharedIcon)} />
        <span {...stylex.props(shared.srOnly)}>{m.share_shared_mark()}</span>
      </>
    ) : null}
  </>
);

/** What the folders of the sidebar need to offer "Share". */
interface ShareFolder {
  /** The server lets folders be shared (mail:share). */
  readonly supported: boolean;
  readonly share: (mailbox: MailboxSummary) => void;
}

const ShareFolderContext = createContext<ShareFolder | null>(null);

/**
 * Sharing one of our folders, from its context menu or ⌘K: the state and
 * its dialog live here (composition rule "state-lift-state"), once for the
 * whole sidebar, collapsed or not. Inside the sidebar on purpose: on phones
 * it is a modal drawer and its dialogs must open inside it (PortalScope).
 */
export const ShareFolderProvider = ({
  children,
  current,
  folderName = plainName,
  mailboxes,
}: {
  readonly children: ReactNode;
  readonly mailboxes: readonly MailboxSummary[] | undefined;
  readonly current: string;
  readonly folderName?: FolderName;
}) => {
  const sharing = useSharing();
  const [sharingWith, setSharingWith] = useState<MailboxSummary | null>(null);
  const supported = sharing.data === true;
  const ref = parseMailboxParam(current);
  const open =
    ref.accountId === null
      ? mailboxes?.find((box) => mailboxParam(box) === ref.mailbox)
      : undefined;
  useCommands(
    supported && open !== undefined && open.mayShare
      ? [
          {
            group: "actions",
            icon: Users,
            id: "share-folder",
            keywords: "share compartir",
            label: m.share_command({ name: folderName(open) }),
            rank: 100,
            run: () => {
              setSharingWith(open);
            },
          },
        ]
      : []
  );
  return (
    <ShareFolderContext
      // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object.
      value={{ share: setSharingWith, supported }}
    >
      {children}
      <ShareDialog
        mailbox={
          sharingWith === null
            ? null
            : { id: sharingWith.id, name: folderName(sharingWith) }
        }
        onOpenChange={(next) => {
          if (!next) {
            setSharingWith(null);
          }
        }}
      />
    </ShareFolderContext>
  );
};

/** A folder of the sidebar, with "Share" in its context menu when allowed. */
export const ShareFolderMenu = ({
  children,
  mailbox,
}: {
  readonly mailbox: MailboxSummary;
  /** The folder's NavItem. */
  readonly children: ReactElement;
}) => {
  const sharing = use(ShareFolderContext);
  if (sharing === null) {
    throw new Error("ShareFolderMenu must be inside ShareFolderProvider");
  }
  if (!sharing.supported || !mailbox.mayShare) {
    return children;
  }
  return (
    <ContextMenu
      menu={
        <MenuItem
          icon={Share2}
          onClick={() => {
            sharing.share(mailbox);
          }}
        >
          {m.share_menu()}
        </MenuItem>
      }
    >
      {/* NavItem doesn't pass the trigger's handlers on: a wrapper does. */}
      <div>{children}</div>
    </ContextMenu>
  );
};

const QUOTA_WARNINGS: Partial<Record<QuotaLevel, () => string>> = {
  full: m.quota_full,
  warn: m.quota_warn,
};

/** Storage used, with a warning near the limit; nothing without a quota. */
export const QuotaMeter = () => {
  const quota = useQuery({
    queryFn: getStorageQuota,
    queryKey: [...mailKeys.all, "quota"],
    staleTime: 5 * 60_000,
  });
  if (quota.data === undefined || quota.data === null) {
    return null;
  }
  const { level, limit, used } = quota.data;
  const locale = getLocale();
  const text = m.quota_used({
    limit: formatBytes(limit, locale),
    used: formatBytes(used, locale),
  });
  const warning = QUOTA_WARNINGS[level]?.() ?? null;
  return (
    <div {...stylex.props(styles.quota)}>
      <ProgressBar
        end={<span {...stylex.props(styles.quotaText)}>{text}</span>}
        label={m.quota_label()}
        max={limit}
        value={Math.min(used, limit)}
      />
      {warning === null ? null : (
        <output
          {...stylex.props(
            styles.quotaText,
            styles.quotaBlock,
            level === "full" ? styles.quotaFull : styles.quotaWarn
          )}
        >
          {warning}
        </output>
      )}
    </div>
  );
};
