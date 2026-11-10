// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The reader's toolbar: back/close, the actions the folder allows, the
// position in the list and the "More" menu.

import type { EmailId } from "@nebulamail/jmap-client";
import { IconButton, Menu, MenuItem } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  Archive,
  ArrowLeft,
  ChevronDown,
  ChevronUp,
  Download,
  Ellipsis,
  FileCode,
  FileText,
  Folder,
  FolderInput,
  ListTodo,
  Mail,
  MailCheck,
  MailOpen,
  OctagonAlert,
  Printer,
  Star,
  Trash2,
  Type,
  X,
} from "lucide-react";

import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import type { ReaderView } from "./email-body";
import { useListContent } from "./list-state";
import { mailboxLabel } from "./mailbox-label";
import { readerStyles } from "./reader-styles";
import { useKeys } from "./use-apple";
import { useCompactToolbar } from "./use-compact-toolbar";
import type { ReaderActions } from "./use-reader-actions";
import { useReadingPane } from "./use-reading-pane";

const styles = stylex.create({
  desktopOnly: { display: { default: "inline-flex", [bp.narrow]: "none" } },
  narrowOnly: { display: { default: "none", [bp.narrow]: "inline-flex" } },
  always: { display: "inline-flex" },
  position: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    marginInline: "6px",
    whiteSpace: "nowrap",
  },
  // 1×16 px between groups; with the buttons' 2 px gap, 8 px each side.
  sep: {
    backgroundColor: colors.line,
    flexShrink: 0,
    height: "16px",
    marginInline: "6px",
    width: "1px",
  },
  spacer: { flexGrow: 1 },
  /** Hidden on phones and touch tablets; they also live in "More". */
  wide: {
    display: {
      default: "inline-flex",
      [bp.mobile]: "none",
      [bp.narrowTouch]: "none",
    },
  },
});

interface ToolbarProps {
  readonly actions: ReaderActions;
  /** The newest email is loaded (print, download, original act on it). */
  readonly ready: boolean;
  readonly latestId: EmailId | undefined;
  readonly view: ReaderView;
  readonly onToggleView: () => void;
}

/** The divider between two groups of buttons. */
const GroupSep = ({ wide = false }: { readonly wide?: boolean }) => (
  <span aria-hidden {...stylex.props(styles.sep, wide ? styles.wide : null)} />
);

interface GroupProps {
  readonly actions: ReaderActions;
}

/** Archive, delete and spam. */
const DestroyGroup = ({ actions }: GroupProps) => {
  const keys = useKeys();
  const { can } = actions;
  const handleArchive = actions.archive;
  const handleTrash = actions.trash;
  const handleSpam = actions.inJunk ? actions.notSpam : actions.spam;
  const spamIcon = actions.inJunk ? MailCheck : OctagonAlert;
  const spamLabel = actions.inJunk ? m.action_not_spam() : m.action_spam();
  return (
    <>
      {can.archive ? (
        <IconButton
          icon={Archive}
          label={m.action_archive()}
          onClick={handleArchive}
          shortcut={keys("e")}
        />
      ) : null}
      {can.trash ? (
        <IconButton
          icon={Trash2}
          label={m.action_trash()}
          onClick={handleTrash}
          shortcut={keys("#")}
        />
      ) : null}
      {(actions.inJunk ? can.notSpam : can.spam) ? (
        <span {...stylex.props(styles.wide)}>
          <IconButton
            icon={spamIcon}
            label={spamLabel}
            onClick={handleSpam}
            shortcut={keys("!")}
          />
        </span>
      ) : null}
    </>
  );
};

/** Move to a folder: shown whenever it may move (on phones it has no other place). */
const MoveGroup = ({ actions }: GroupProps) => {
  const keys = useKeys();
  const handleMoveMenu = actions.setMoveMenuOpen;
  return (
    <span>
      <Menu
        onOpenChange={handleMoveMenu}
        open={actions.moveMenuOpen}
        trigger={
          <IconButton
            icon={FolderInput}
            label={m.action_move()}
            shortcut={keys("v")}
          />
        }
      >
        {actions.can.moveTargets.map((box) => (
          <MenuItem
            icon={Folder}
            key={box.id}
            onClick={() => {
              actions.move(box);
            }}
          >
            {mailboxLabel(box)}
          </MenuItem>
        ))}
      </Menu>
    </span>
  );
};

/** Read or unread, and star. */
const MarkGroup = ({ actions }: GroupProps) => {
  const keys = useKeys();
  const { can, flagged, unread } = actions;
  const handleFlag = actions.toggleFlag;
  const handleReadToggle = unread ? actions.markRead : actions.markUnread;
  const flagLabel = flagged ? m.action_unflag() : m.action_flag();
  return (
    <>
      {can.setSeen ? (
        <span {...stylex.props(styles.wide)}>
          <IconButton
            icon={unread ? MailOpen : Mail}
            label={unread ? m.action_mark_read() : m.action_mark_unread()}
            onClick={handleReadToggle}
            shortcut={unread ? keys("shift+i") : keys("shift+u")}
          />
        </span>
      ) : null}
      {can.setFlagged ? (
        <span {...stylex.props(styles.wide)}>
          <IconButton
            icon={Star}
            label={flagLabel}
            onClick={handleFlag}
            pressed={flagged}
            shortcut={keys("s")}
          />
        </span>
      ) : null}
    </>
  );
};

/**
 * Archive, delete, spam, move, read and star: only what the folder's
 * rights allow (rights.ts), in three groups with a divider between those
 * shown: archive, delete and spam | move | read and star.
 */
const ChangeActions = ({ actions }: GroupProps) => {
  const { can } = actions;
  const destroy = can.archive || can.trash;
  const mark = can.setSeen || can.setFlagged;
  return (
    <>
      <DestroyGroup actions={actions} />
      {destroy && can.move ? <GroupSep /> : null}
      {can.move ? <MoveGroup actions={actions} /> : null}
      {(destroy || can.move) && mark ? <GroupSep wide /> : null}
      <MarkGroup actions={actions} />
    </>
  );
};

/**
 * The "More" menu's copies of what the compact toolbar hides (its `wide`
 * buttons): only then, so no action shows twice on screen.
 */
const CompactOnlyItems = ({ actions }: Pick<ToolbarProps, "actions">) => {
  const keys = useKeys();
  const compact = useCompactToolbar();
  if (!compact) {
    return null;
  }
  const { can, flagged, unread } = actions;
  const handleSpam = actions.inJunk ? actions.notSpam : actions.spam;
  const spamIcon = actions.inJunk ? MailCheck : OctagonAlert;
  const spamLabel = actions.inJunk ? m.action_not_spam() : m.action_spam();
  const handleFlag = actions.toggleFlag;
  const flagLabel = flagged ? m.action_unflag() : m.action_flag();
  const handleReadToggle = unread ? actions.markRead : actions.markUnread;
  return (
    <>
      {can.setFlagged ? (
        <MenuItem icon={Star} onClick={handleFlag} shortcut={keys("s")}>
          {flagLabel}
        </MenuItem>
      ) : null}
      {can.setSeen ? (
        <MenuItem
          icon={unread ? MailOpen : Mail}
          onClick={handleReadToggle}
          shortcut={unread ? keys("shift+i") : keys("shift+u")}
        >
          {unread ? m.action_mark_read() : m.action_mark_unread()}
        </MenuItem>
      ) : null}
      {(actions.inJunk ? can.notSpam : can.spam) ? (
        <MenuItem icon={spamIcon} onClick={handleSpam} shortcut={keys("!")}>
          {spamLabel}
        </MenuItem>
      ) : null}
    </>
  );
};

export const ReaderToolbar = ({
  actions,
  latestId,
  onToggleView,
  ready,
  view,
}: ToolbarProps) => {
  const list = useListContent();
  const keys = useKeys();
  const fullPage = useReadingPane() === "full";
  const noEmail = !ready;
  const handleClose = list.actions.close;
  const { next, position, previous } = actions;

  return (
    <div
      aria-label={m.reader_toolbar()}
      role="toolbar"
      {...stylex.props(readerStyles.toolbar)}
    >
      {/* Back to the list when the email takes its place (narrow screens,
          or "full page"); a close button next to the list. */}
      <span {...stylex.props(fullPage ? styles.always : styles.narrowOnly)}>
        <IconButton
          icon={ArrowLeft}
          label={m.reader_back()}
          onClick={handleClose}
          shortcut={keys("escape")}
        />
      </span>
      {fullPage ? null : (
        <span {...stylex.props(styles.desktopOnly)}>
          <IconButton
            icon={X}
            label={m.close()}
            onClick={handleClose}
            shortcut={keys("escape")}
          />
        </span>
      )}
      <span aria-hidden {...stylex.props(styles.sep, styles.wide)} />
      <ChangeActions actions={actions} />
      <span {...stylex.props(styles.spacer)} />
      {position === null ? null : (
        <span {...stylex.props(styles.position, styles.wide)}>
          {m.reader_position({ index: position, total: list.state.total })}
        </span>
      )}
      <IconButton
        disabled={previous === undefined}
        icon={ChevronUp}
        label={m.reader_prev()}
        onClick={() => {
          if (previous !== undefined) {
            list.actions.open(previous);
          }
        }}
        shortcut={keys("k")}
      />
      <IconButton
        disabled={next === undefined}
        icon={ChevronDown}
        label={m.reader_next()}
        onClick={() => {
          if (next !== undefined) {
            list.actions.open(next);
          }
        }}
        shortcut={keys("j")}
      />
      <Menu
        align="end"
        trigger={<IconButton icon={Ellipsis} label={m.more_actions()} />}
      >
        <MenuItem
          icon={view === "html" ? Type : FileText}
          onClick={onToggleView}
        >
          {view === "html" ? m.view_as_text() : m.view_as_html()}
        </MenuItem>
        <CompactOnlyItems actions={actions} />
        {actions.canCreateTask ? (
          <MenuItem
            icon={ListTodo}
            onClick={() => {
              actions.createTask();
            }}
            shortcut={keys("shift+t")}
          >
            {m.tasks_create_from_email()}
          </MenuItem>
        ) : null}
        <MenuItem
          disabled={noEmail}
          icon={Printer}
          onClick={() => {
            actions.print();
          }}
        >
          {m.action_print()}
        </MenuItem>
        <MenuItem
          disabled={noEmail}
          icon={Download}
          onClick={() => {
            actions.downloadEml();
          }}
        >
          {m.action_download_eml()}
        </MenuItem>
        <MenuItem
          disabled={latestId === undefined}
          icon={FileCode}
          onClick={() => {
            actions.showOriginal(latestId ?? null);
          }}
        >
          {m.action_view_original()}
        </MenuItem>
      </Menu>
    </div>
  );
};
