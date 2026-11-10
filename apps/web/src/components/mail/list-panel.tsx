// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The list panel: one header row (the split-inbox tabs, as in Superhuman,
// with the range, the read filter and select-all at its end), bulk actions
// for the selection, and one row per conversation with roving focus (j/k),
// virtualized and loading more near the end.

import type { EmailId } from "@nebulamail/jmap-client";
import type { Icon } from "@nebulamail/ui";
import {
  Button,
  Checkbox,
  EmptyState,
  IconButton,
  IllustrationInboxZero,
  IllustrationNoResults,
  Menu,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuSubmenu,
  Panel,
  Tab,
  TabList,
  TabPanel,
  Tabs,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useRouteContext } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Archive,
  ChevronDown,
  FolderInput,
  Folder,
  ListFilter,
  Mail,
  MailCheck,
  MailOpen,
  OctagonAlert,
  Paperclip,
  SquareCheck,
  Star,
  StarOff,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import type { ReactElement, ReactNode } from "react";
import { useEffect, useLayoutEffect, useMemo, useRef } from "react";

import { useShortcut } from "../../lib/shortcuts";
import { useNow } from "../../lib/use-now";
import { useDeliveryStatusPages, useOwnAddresses } from "../../mail/hooks";
import { useScheduledSendAt } from "../../mail/use-schedule";
import { READ_FILTERS } from "../../mail/views";
import type { ReadFilter } from "../../mail/views";
import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import type { Command } from "./commands";
import { useCommands } from "./commands";
import { EmailRow } from "./email-row";
import type { RowMenuProps } from "./email-row";
import { ListSkeleton } from "./list-skeleton";
import { useList } from "./list-state";
import { isInOverlay } from "./mail-ui";
import { mailboxLabel } from "./mailbox-label";
import { specialLabel } from "./sidebar";
import { shared } from "./styles";
import { useKeys } from "./use-apple";
import { useConversationActions } from "./use-conversation-actions";
import { useReadingPane } from "./use-reading-pane";
import { VacationBanner } from "./vacation-banner";

/** Rows kept rendered above and below the visible ones. */
const OVERSCAN = 8;
/** Start loading the next page this many rows before the end. */
const LOAD_AHEAD = 15;
/** First guess of a row's height; each row is measured once shown. */
const ESTIMATED_ROW = { comfortable: 82, compact: 54 } as const;
const ROW_GAP = 2;

/** The header is narrower than tabs + range + read filter. */
const NARROW_HEADER = "@container (max-width: 559px)";
/**
 * Room under the last row for the floating Compose button on phones
 * (shell.tsx: 20 px from the bottom, up to 52 px tall shrunk or not)
 * plus a 24 px gap.
 */
const FAB_CLEARANCE = "calc(96px + env(safe-area-inset-bottom))";

const styles = stylex.create({
  bulk: { backgroundColor: colors.sel },
  center: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
    rowGap: "12px",
  },
  countShort: { display: { default: "none", [bp.mobile]: "inline" } },
  countWide: { display: { default: "inline", [bp.mobile]: "none" } },
  count: {
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    marginInline: "4px 6px",
    whiteSpace: "nowrap",
  },
  /**
   * Range, read filter and select-all, at the end of the header row (after
   * the tabs, or alone in folders without them). As tall as a tab, so they
   * sit on the tabs' line.
   */
  tools: {
    alignItems: "center",
    alignSelf: "flex-end",
    // Room between the range and the filter's focus ring.
    columnGap: "8px",
    display: "flex",
    flexShrink: 0,
    height: "38px",
    marginInlineStart: "auto",
    minWidth: 0,
  },
  toolsHeader: { display: { default: "flex", [bp.mobile]: "none" } },
  toolsTitle: {
    alignSelf: "center",
    // The title has 14 px above and 2 px below: centre on its text.
    paddingTop: "12px",
    display: { default: "none", [bp.mobile]: "flex" },
  },
  /** Phones: the folder's title and the tools on one row. */
  titleRow: {
    alignItems: "center",
    display: { default: "contents", [bp.mobile]: "flex" },
    paddingInlineEnd: { default: null, [bp.mobile]: "16px" },
  },
  /** Folders without tabs: on phones the tools sit by the title instead. */
  headerNoTabs: { display: { default: "flex", [bp.mobile]: "none" } },
  /**
   * Select-all shows where it's needed: hovering or tabbing into the
   * header, and always on touch screens (no hover). Phones select by
   * tapping an avatar instead, and the bulk bar has its own select-all.
   */
  selectAll: {
    display: { default: "flex", [bp.mobile]: "none" },
    opacity: {
      default: 0,
      [stylex.when.ancestor(":hover")]: 1,
      [stylex.when.ancestor(":focus-within")]: 1,
      [touch.noHover]: 1,
    },
    transitionDuration: "120ms",
    transitionProperty: "opacity",
  },
  /** The header row: tabs (or nothing) and the tools, one line under them. */
  header: {
    alignItems: "stretch",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    containerType: "inline-size",
    display: "flex",
    minHeight: "49px",
    paddingInlineEnd: { default: "10px", [bp.mobile]: "16px" },
  },
  heading: {
    fontSize: { default: font.sizeLg, [bp.mobile]: font.sizeXxl },
    fontWeight: { default: font.weightBold, [bp.mobile]: 750 },
    letterSpacing: "-0.02em",
    margin: 0,
  },
  // The folder's name: the top bar shows it on wider screens (top-bar.tsx),
  // so here it is for screen readers only, except on phones.
  headingBar: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: { default: 0, [bp.mobile]: "1px" },
    clip: { default: "rect(0 0 0 0)", [bp.mobile]: "auto" },
    display: "flex",
    height: { default: "1px", [bp.mobile]: "auto" },
    minHeight: { default: 0, [bp.mobile]: "48px" },
    overflow: "hidden",
    paddingInline: { default: 0, [bp.mobile]: "16px" },
    position: { default: "absolute", [bp.mobile]: "static" },
    width: { default: "1px", [bp.mobile]: "auto" },
  },
  loadMore: { display: "flex", justifyContent: "center", padding: "10px" },
  panel: {
    flexBasis: { default: "clamp(340px, 36%, 470px)", [bp.narrow]: "100%" },
    flexGrow: { default: 0, [bp.narrow]: 1 },
    flexShrink: 0,
    minWidth: 0,
  },
  panelHidden: { display: { default: "flex", [bp.narrow]: "none" } },
  // "Full page" reading (Settings → Open emails): the list takes the whole
  // width, and the open email takes its place.
  panelFull: { flexBasis: "100%", flexGrow: 1 },
  panelGone: { display: "none" },
  panelMobile: {
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderWidth: { default: "1px", [bp.mobile]: 0 },
  },
  range: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    marginInlineStart: "auto",
    whiteSpace: "nowrap",
  },
  /** Inbox title: shown on phones only (the tabs name it on wider screens). */
  mobileTitle: {
    borderWidth: 0,
    // In its row with the tools on phones (titleRow): it takes the room
    // and shrinks with an ellipsis; elsewhere it's out of the flow.
    flexGrow: { default: 0, [bp.mobile]: 1 },
    flexShrink: { default: 0, [bp.mobile]: 1 },
    minWidth: 0,
    clip: { default: "rect(0 0 0 0)", [bp.mobile]: "auto" },
    height: { default: "1px", [bp.mobile]: "auto" },
    margin: { default: "-1px", [bp.mobile]: 0 },
    overflow: "hidden",
    padding: { default: 0, [bp.mobile]: "14px 16px 2px" },
    position: { default: "absolute", [bp.mobile]: "static" },
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    width: { default: "1px", [bp.mobile]: "auto" },
  },
  /**
   * No room for "1–50 of 230" next to the tabs and the filter whenever the
   * header itself is narrow (it depends on the panes, not the window).
   */
  rangeWide: {
    display: { default: "inline", [NARROW_HEADER]: "none" },
  },
  rows: {
    containerName: "mail-rows",
    containerType: "inline-size",
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
    paddingBlock: { default: "4px", [bp.mobile]: `4px ${FAB_CLEARANCE}` },
    paddingInline: "6px",
  },
  /** Room for the rows that aren't rendered (virtual list). */
  spacers: (before: number, after: number) => ({
    paddingBottom: `${after}px`,
    paddingTop: `${before}px`,
  }),
  virtual: {
    display: "flex",
    flexDirection: "column",
    rowGap: `${ROW_GAP}px`,
  },
  toolbar: {
    alignItems: "center",
    containerType: "inline-size",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "6px",
    display: "flex",
    minHeight: "48px",
    paddingBlock: "8px",
    paddingInlineEnd: "10px",
    // The select-all box over the rows' avatar column (rows: 6 px + 20 px
    // gutter, a 28 px compact avatar holding a 16 px box).
    paddingInlineStart: { default: "32px", [bp.mobile]: "14px" },
  },
  tabsWrap: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
  },
});

const SPLIT_LABEL: Readonly<Record<string, () => string>> = {
  attachments: m.split_attachments,
  important: m.split_important,
  unread: m.split_unread,
};
const SPLIT_ICON: Readonly<Record<string, Icon>> = {
  attachments: Paperclip,
  important: Zap,
  unread: Mail,
};

const FILTER_LABEL: Readonly<Record<ReadFilter, () => string>> = {
  all: m.filter_all,
  attachment: m.filter_attachment,
  read: m.filter_read,
  unread: m.filter_unread,
};

const MoveMenu = ({
  ids,
  onDone,
  trigger,
}: {
  readonly ids: readonly EmailId[];
  readonly onDone?: () => void;
  readonly trigger: ReactElement;
}) => {
  const actions = useConversationActions();
  return (
    <Menu trigger={trigger}>
      {actions.can.moveTargets.map((box) => (
        <MenuItem
          icon={Folder}
          key={box.id}
          onClick={() => {
            actions.move(ids, box, mailboxLabel(box));
            onDone?.();
          }}
        >
          {mailboxLabel(box)}
        </MenuItem>
      ))}
    </Menu>
  );
};

export { MoveMenu };

/** The open list is the Junk folder ("Not spam" instead of "Report spam"). */
const isJunkList = (list: ReturnType<typeof useList>): boolean =>
  list.state.view?.kind === "mailbox" &&
  list.state.view.mailbox.role === "junk";

/**
 * A row's right-click menu (Gmail/Outlook style): the row's actions with
 * their keys. On a selected row it acts on the whole selection, like the
 * bulk bar; otherwise on that row alone. Only what the folder allows.
 */
const RowMenu = ({ id, unread, flagged }: RowMenuProps) => {
  const list = useList();
  const actions = useConversationActions();
  const keys = useKeys();
  const { selected } = list.state;
  const ids: EmailId[] = selected.has(id) ? [...selected] : [id];
  const done = (): void => {
    if (selected.has(id)) {
      list.actions.setSelected(new Set());
    }
  };
  const inJunk = isJunkList(list);
  const { can } = actions;
  return (
    <>
      {can.archive ? (
        <MenuItem
          icon={Archive}
          onClick={() => {
            actions.archive(ids);
            done();
          }}
          shortcut={keys("e")}
        >
          {m.action_archive()}
        </MenuItem>
      ) : null}
      {can.trash ? (
        <MenuItem
          icon={Trash2}
          onClick={() => {
            actions.trash(ids);
            done();
          }}
          shortcut={keys("#")}
        >
          {m.action_trash()}
        </MenuItem>
      ) : null}
      {(inJunk ? can.notSpam : can.spam) ? (
        <MenuItem
          icon={inJunk ? MailCheck : OctagonAlert}
          onClick={() => {
            if (inJunk) {
              actions.notSpam(ids);
            } else {
              actions.spam(ids);
            }
            done();
          }}
          shortcut={keys("!")}
        >
          {inJunk ? m.action_not_spam() : m.action_spam()}
        </MenuItem>
      ) : null}
      {can.moveTargets.length > 0 ? (
        <MenuSubmenu icon={FolderInput} label={m.action_move()}>
          {can.moveTargets.map((box) => (
            <MenuItem
              icon={Folder}
              key={box.id}
              onClick={() => {
                actions.move(ids, box, mailboxLabel(box));
                done();
              }}
            >
              {mailboxLabel(box)}
            </MenuItem>
          ))}
        </MenuSubmenu>
      ) : null}
      {can.setSeen || can.setFlagged ? <MenuSeparator /> : null}
      {can.setSeen ? (
        <MenuItem
          icon={unread ? MailOpen : Mail}
          onClick={() => {
            actions.setSeen(ids, unread);
          }}
          shortcut={unread ? keys("shift+i") : keys("shift+u")}
        >
          {unread ? m.action_mark_read() : m.action_mark_unread()}
        </MenuItem>
      ) : null}
      {can.setFlagged ? (
        <MenuItem
          icon={flagged ? StarOff : Star}
          onClick={() => {
            actions.setFlagged(ids, !flagged);
          }}
          shortcut={keys("s")}
        >
          {flagged ? m.action_unflag() : m.action_flag()}
        </MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuItem
        icon={SquareCheck}
        onClick={() => {
          const next = new Set(selected);
          if (selected.has(id)) {
            next.delete(id);
          } else {
            next.add(id);
          }
          list.actions.setSelected(next);
        }}
        shortcut={keys("x")}
      >
        {selected.has(id) ? m.deselect_row() : m.select_row()}
      </MenuItem>
    </>
  );
};

/**
 * The read filter (All / Unread / Read / Attachments) as a small menu on
 * the toolbar: it names the current choice, and stays highlighted while
 * it hides something.
 */
const ReadFilterMenu = () => {
  const list = useList();
  const current = list.state.readFilter;
  const filtered = current !== "all";
  const label = `${m.filter_label()}: ${FILTER_LABEL[current]()}`;
  return (
    <Menu
      align="end"
      trigger={
        // Just the icon while it shows everything; the choice by name
        // (highlighted) while it hides something.
        filtered ? (
          <Button
            aria-label={label}
            icon={ListFilter}
            iconEnd={ChevronDown}
            pressed
            size="sm"
            variant="ghost"
          >
            {FILTER_LABEL[current]()}
          </Button>
        ) : (
          <IconButton icon={ListFilter} label={label} size="sm" />
        )
      }
    >
      <MenuRadioGroup
        onValueChange={(value) => {
          const filter = READ_FILTERS.find((option) => option === value);
          if (filter !== undefined) {
            list.actions.setSearch({ filter });
          }
        }}
        value={current}
      >
        {READ_FILTERS.map((value) => (
          <MenuRadioItem closeOnClick key={value} value={value}>
            {FILTER_LABEL[value]()}
          </MenuRadioItem>
        ))}
      </MenuRadioGroup>
    </Menu>
  );
};

/** Select-all, for the header row and the bulk bar. */
const SelectAll = () => {
  const list = useList();
  const ids = list.state.selected;
  const all =
    list.state.emails.length > 0 && ids.size === list.state.emails.length;
  const some = ids.size > 0 && !all;
  return (
    <Checkbox
      aria-label={m.select_all()}
      checked={all}
      disabled={list.state.emails.length === 0}
      indeterminate={some}
      onCheckedChange={(checked) => {
        list.actions.setSelected(
          checked
            ? new Set(list.state.emails.map((email) => email.id))
            : new Set()
        );
      }}
    />
  );
};

/**
 * The end of the header row: the range (only when the folder has more
 * than what's loaded: "1–33 of 33" says nothing), the read filter and
 * select-all.
 */
const ListTools = ({
  place,
}: {
  /**
   * "header": the header row (wider screens). "title": next to the folder's
   * title, on phones, where the tabs need the header's whole width.
   */
  readonly place: "header" | "title";
}) => {
  const list = useList();
  const shown = list.state.emails.length;
  return (
    <span
      {...stylex.props(
        styles.tools,
        place === "header" ? styles.toolsHeader : styles.toolsTitle
      )}
    >
      {list.state.total > shown ? (
        <span {...stylex.props(styles.range, styles.rangeWide)}>
          {m.list_range({ from: 1, to: shown, total: list.state.total })}
        </span>
      ) : null}
      <ReadFilterMenu />
      <span {...stylex.props(styles.selectAll)}>
        <SelectAll />
      </span>
    </span>
  );
};

/** Bulk actions while something is selected (a row under the header). */
const BulkBar = () => {
  const list = useList();
  const actions = useConversationActions();
  const inJunk = isJunkList(list);
  const ids = [...list.state.selected];
  const clear = () => {
    list.actions.setSelected(new Set());
  };

  if (ids.length > 0) {
    const anyUnread = list.state.emails.some(
      (row) => list.state.selected.has(row.id) && actions.view(row).unread
    );
    return (
      <div
        aria-label={m.list_bulk_label()}
        role="toolbar"
        {...stylex.props(styles.toolbar, styles.bulk)}
      >
        <SelectAll />
        {/* Phones show just the number, so every action fits in 360 px. */}
        <span {...stylex.props(styles.count)}>
          <span {...stylex.props(styles.countWide)}>
            {m.selected_count({ count: ids.length })}
          </span>
          <span aria-hidden {...stylex.props(styles.countShort)}>
            {ids.length}
          </span>
          <span {...stylex.props(shared.srOnly, styles.countShort)}>
            {m.selected_count({ count: ids.length })}
          </span>
        </span>
        {actions.can.archive ? (
          <IconButton
            icon={Archive}
            label={m.action_archive()}
            onClick={() => {
              actions.archive(ids);
              clear();
            }}
            shortcut={["E"]}
            size="sm"
          />
        ) : null}
        {actions.can.trash ? (
          <IconButton
            icon={Trash2}
            label={m.action_trash()}
            onClick={() => {
              actions.trash(ids);
              clear();
            }}
            shortcut={["#"]}
            size="sm"
          />
        ) : null}
        {actions.can.setSeen ? (
          <IconButton
            icon={anyUnread ? MailOpen : Mail}
            label={anyUnread ? m.action_mark_read() : m.action_mark_unread()}
            onClick={() => {
              actions.setSeen(ids, anyUnread);
            }}
            size="sm"
          />
        ) : null}
        {(inJunk ? actions.can.notSpam : actions.can.spam) ? (
          <IconButton
            icon={inJunk ? MailCheck : OctagonAlert}
            label={inJunk ? m.action_not_spam() : m.action_spam()}
            onClick={() => {
              if (inJunk) {
                actions.notSpam(ids);
              } else {
                actions.spam(ids);
              }
              clear();
            }}
            shortcut={["!"]}
            size="sm"
          />
        ) : null}
        {actions.can.move ? (
          <MoveMenu
            ids={ids}
            onDone={clear}
            trigger={
              <IconButton
                icon={FolderInput}
                label={m.action_move()}
                size="sm"
              />
            }
          />
        ) : null}
        <span {...stylex.props(styles.range)}>
          <IconButton
            icon={X}
            label={m.list_clear_selection()}
            onClick={clear}
            size="sm"
          />
        </span>
      </div>
    );
  }

  return null;
};

const Empty = () => {
  const list = useList();
  if (list.state.query !== "") {
    return (
      <EmptyState
        illustration={<IllustrationNoResults />}
        title={m.empty_search_title()}
      >
        {m.empty_search_body({ query: list.state.query })}
      </EmptyState>
    );
  }
  const inbox =
    list.state.view?.kind === "mailbox" &&
    list.state.view.mailbox.role === "inbox";
  if (
    inbox &&
    list.state.readFilter === "all" &&
    list.state.split.condition === null
  ) {
    return (
      <EmptyState
        illustration={<IllustrationInboxZero />}
        title={m.empty_inbox_title()}
      >
        {m.empty_inbox_body()}
      </EmptyState>
    );
  }
  return (
    <EmptyState title={m.empty_folder_title()}>
      {list.state.readFilter === "all"
        ? m.empty_folder_body()
        : m.empty_filter_body()}
    </EmptyState>
  );
};

const NO_PAGES: readonly (readonly { readonly id: EmailId }[])[] = [];

/**
 * The rows, virtualized (TanStack Virtual): only the visible
 * ones (plus a margin) are in the DOM, with spacers for the rest, so a
 * mailbox with thousands of conversations scrolls smoothly. Keyboard focus
 * (j/k), selection and hover actions work on any row: moving the focus to
 * a row that isn't rendered scrolls it in first and focuses it on mount.
 */
const Rows = () => {
  // TanStack Virtual returns the same object on every render and changes
  // it inside, which the React Compiler can't see: its memoized output
  // would show stale rows. React's documented opt-out for such libraries.
  "use no memo";
  const list = useList();
  const conversations = useConversationActions();
  const own = useOwnAddresses();
  const now = useNow();
  const compact =
    useRouteContext({ from: "__root__" }).preferences.density === "compact";
  const scroller = useRef<HTMLDivElement>(null);
  const links = useRef(new Map<EmailId, HTMLAnchorElement>());
  const pendingFocus = useRef<EmailId | null>(null);
  const reading = list.state.openId !== undefined;
  const { emails, hasMore } = list.state;
  const { loadMore } = list.actions;
  // Sent shows how each message did (delivered, bounced, read).
  const sent = list.state.folderKind === "sent";
  const delivery = useDeliveryStatusPages(sent ? list.state.pages : NO_PAGES);
  // …and which haven't gone out yet («Programado»).
  const scheduled = useScheduledSendAt(sent);

  // oxlint-disable-next-line react/incompatible-library -- known and handled: this component opts out of the compiler ("use no memo" above) and only passes plain values on.
  const virtualizer = useVirtualizer({
    count: emails.length,
    estimateSize: () => ESTIMATED_ROW[compact ? "compact" : "comfortable"],
    gap: ROW_GAP,
    getItemKey: (index) => emails[index]?.id ?? index,
    getScrollElement: () => scroller.current,
    overscan: OVERSCAN,
  });
  const items = virtualizer.getVirtualItems();
  const lastIndex = items.at(-1)?.index ?? -1;

  // Infinite loading: the next page when the end comes near.
  useEffect(() => {
    if (hasMore && lastIndex >= emails.length - LOAD_AHEAD) {
      loadMore();
    }
  }, [hasMore, lastIndex, emails.length, loadMore]);

  // The open conversation stays in view while stepping with j/k.
  const openIndex =
    list.state.openRowId === undefined
      ? -1
      : emails.findIndex((row) => row.id === list.state.openRowId);
  useEffect(() => {
    if (openIndex !== -1) {
      virtualizer.scrollToIndex(openIndex, { align: "auto" });
    }
  }, [openIndex, virtualizer]);

  const focusRow = (id: EmailId | undefined): void => {
    if (id === undefined) {
      return;
    }
    list.actions.setFocusedId(id);
    const index = emails.findIndex((row) => row.id === id);
    if (index !== -1) {
      virtualizer.scrollToIndex(index, { align: "auto" });
    }
    const link = links.current.get(id);
    if (link === undefined) {
      // Not rendered yet: focus it when it mounts (linkRef below).
      pendingFocus.current = id;
    } else {
      link.focus({ preventScroll: true });
    }
  };

  const step = (direction: 1 | -1) => (event: KeyboardEvent) => {
    if (isInOverlay(event.target)) {
      return;
    }
    const from = list.state.openId ?? list.state.focusedId;
    // The first J/K only shows the cursor where it already is (the first
    // row), instead of skipping it.
    let next: EmailId | undefined = from;
    if (from === undefined) {
      next = emails[0]?.id;
    } else if (list.state.cursorShown) {
      next = list.meta.neighbor(from, direction);
    }
    if (next === undefined) {
      if (direction === 1 && hasMore) {
        loadMore();
      }
      return;
    }
    if (reading) {
      list.actions.open(next);
    } else {
      focusRow(next);
    }
  };
  useShortcut("j", step(1));
  useShortcut("k", step(-1));

  // Actions on the selection, or on the focused row when nothing is
  // selected and no conversation is open (the reader handles those).
  const targets = (): EmailId[] => {
    if (list.state.selected.size > 0) {
      return [...list.state.selected];
    }
    return !reading && list.state.focusedId !== undefined
      ? [list.state.focusedId]
      : [];
  };
  const listAction =
    (run: (ids: EmailId[]) => void) =>
    (event: KeyboardEvent): void => {
      const ids = targets();
      if (isInOverlay(event.target) || ids.length === 0) {
        return;
      }
      if (
        !reading &&
        list.state.focusedId !== undefined &&
        ids.includes(list.state.focusedId)
      ) {
        // Keep the focus in the list after the row leaves.
        const next =
          list.meta.neighbor(list.state.focusedId, 1) ??
          list.meta.neighbor(list.state.focusedId, -1);
        setTimeout(() => {
          focusRow(next);
        }, 0);
      }
      run(ids);
      list.actions.setSelected(new Set());
    };
  const enabled = { enabled: !reading || list.state.selected.size > 0 };
  useShortcut("e", listAction(conversations.archive), enabled);
  useShortcut("#", listAction(conversations.trash), enabled);
  const inJunk = isJunkList(list);
  useShortcut(
    "!",
    listAction(inJunk ? conversations.notSpam : conversations.spam),
    enabled
  );
  useShortcut(
    "shift+u",
    (event) => {
      const ids = targets();
      if (!isInOverlay(event.target) && ids.length > 0) {
        conversations.setSeen(ids, false);
      }
    },
    enabled
  );
  useShortcut(
    "shift+i",
    (event) => {
      const ids = targets();
      if (!isInOverlay(event.target) && ids.length > 0) {
        conversations.setSeen(ids, true);
      }
    },
    enabled
  );
  useShortcut(
    "s",
    (event) => {
      const ids = targets();
      const first = emails.find((row) => row.id === ids[0]);
      if (!isInOverlay(event.target) && first !== undefined) {
        conversations.setFlagged(ids, !conversations.view(first).flagged);
      }
    },
    enabled
  );
  useShortcut("x", (event) => {
    const id = list.state.focusedId;
    if (isInOverlay(event.target) || id === undefined || reading) {
      return;
    }
    const next = new Set(list.state.selected);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    list.actions.setSelected(next);
  });
  useShortcut(
    "escape",
    (event) => {
      if (!isInOverlay(event.target)) {
        list.actions.setSelected(new Set());
      }
    },
    { enabled: list.state.selected.size > 0 }
  );

  const { can } = conversations;
  const allowed: Partial<Record<string, boolean>> = {
    "bulk-archive": can.archive,
    "bulk-not-spam": can.notSpam,
    "bulk-read": can.setSeen,
    "bulk-spam": can.spam,
    "bulk-trash": can.trash,
    "bulk-unread": can.setSeen,
  };
  const commands: Command[] =
    list.state.selected.size === 0
      ? []
      : (
          [
            {
              group: "actions",
              icon: Archive,
              id: "bulk-archive",
              label: `${m.action_archive()} (${list.state.selected.size})`,
              run: () => {
                conversations.archive([...list.state.selected]);
                list.actions.setSelected(new Set());
              },
              shortcut: "e",
            },
            {
              group: "actions",
              icon: Trash2,
              id: "bulk-trash",
              label: `${m.action_trash()} (${list.state.selected.size})`,
              run: () => {
                conversations.trash([...list.state.selected]);
                list.actions.setSelected(new Set());
              },
              shortcut: "#",
            },
            {
              group: "actions",
              icon: MailOpen,
              id: "bulk-read",
              label: `${m.action_mark_read()} (${list.state.selected.size})`,
              run: () => {
                conversations.setSeen([...list.state.selected], true);
              },
              shortcut: "shift+i",
            },
            {
              group: "actions",
              icon: Mail,
              id: "bulk-unread",
              label: `${m.action_mark_unread()} (${list.state.selected.size})`,
              run: () => {
                conversations.setSeen([...list.state.selected], false);
              },
              shortcut: "shift+u",
            },
            inJunk
              ? {
                  group: "actions",
                  icon: MailCheck,
                  id: "bulk-not-spam",
                  label: `${m.action_not_spam()} (${list.state.selected.size})`,
                  run: () => {
                    conversations.notSpam([...list.state.selected]);
                    list.actions.setSelected(new Set());
                  },
                  shortcut: "!",
                }
              : {
                  group: "actions",
                  icon: OctagonAlert,
                  id: "bulk-spam",
                  label: `${m.action_spam()} (${list.state.selected.size})`,
                  run: () => {
                    conversations.spam([...list.state.selected]);
                    list.actions.setSelected(new Set());
                  },
                  shortcut: "!",
                },
          ] satisfies Command[]
        ).filter((command) => allowed[command.id] !== false);
  useCommands(commands);

  // The rows get the same handlers on every render (EmailRow is memo, so
  // a cursor move only redraws the two rows it touches); they read the
  // latest list and actions through this ref, set after each render.
  const latest = useRef({ conversations, list });
  useLayoutEffect(() => {
    latest.current = { conversations, list };
  });
  // Only refs inside: made once.
  const {
    handleArchive,
    handleCheck,
    handleFlag,
    handleFocus,
    handleLink,
    handleOpen,
    handleSeen,
    handleTrash,
  } = useMemo(
    () => ({
      handleArchive: (id: EmailId): void => {
        latest.current.conversations.archive([id]);
      },
      handleCheck: (id: EmailId, checked: boolean): void => {
        const { list: current } = latest.current;
        const next = new Set(current.state.selected);
        if (checked) {
          next.add(id);
        } else {
          next.delete(id);
        }
        current.actions.setSelected(next);
      },
      handleFlag: (id: EmailId, flagged: boolean): void => {
        latest.current.conversations.setFlagged([id], flagged);
      },
      handleFocus: (id: EmailId): void => {
        latest.current.list.actions.setFocusedId(id);
      },
      handleLink: (id: EmailId, element: HTMLAnchorElement | null): void => {
        if (element === null) {
          links.current.delete(id);
          return;
        }
        links.current.set(id, element);
        if (pendingFocus.current === id) {
          pendingFocus.current = null;
          element.focus({ preventScroll: true });
        }
      },
      handleOpen: (id: EmailId): void => {
        latest.current.list.actions.open(id);
      },
      handleSeen: (id: EmailId, seen: boolean): void => {
        latest.current.conversations.setSeen([id], seen);
      },
      handleTrash: (id: EmailId): void => {
        latest.current.conversations.trash([id]);
      },
    }),
    []
  );

  const before = items[0]?.start ?? 0;
  const after = Math.max(
    0,
    virtualizer.getTotalSize() - (items.at(-1)?.end ?? 0)
  );

  return (
    <div ref={scroller} {...stylex.props(styles.rows)}>
      <ul
        aria-label={m.list_label()}
        {...stylex.props(
          shared.resetList,
          styles.virtual,
          styles.spacers(before, after)
        )}
      >
        {items.map((item) => {
          const email = emails[item.index];
          if (email === undefined) {
            return null;
          }
          return (
            <EmailRow
              active={email.id === list.state.openRowId}
              checked={list.state.selected.has(email.id)}
              compact={compact}
              context={list.state.context}
              delivery={delivery.get(email.id)}
              scheduledAt={scheduled.get(email.id)}
              email={email}
              cursor={
                list.state.cursorShown && email.id === list.state.focusedId
              }
              focused={email.id === list.state.focusedId}
              index={item.index}
              itemRef={virtualizer.measureElement}
              key={item.key}
              mailbox={list.state.mailbox}
              now={now}
              onArchive={conversations.can.archive ? handleArchive : undefined}
              onCheck={handleCheck}
              onFlag={conversations.can.setFlagged ? handleFlag : undefined}
              onFocus={handleFocus}
              onLink={handleLink}
              onOpen={handleOpen}
              onSeen={conversations.can.setSeen ? handleSeen : undefined}
              onTrash={conversations.can.trash ? handleTrash : undefined}
              menu={RowMenu}
              own={own}
              selecting={list.state.selected.size > 0}
              setSize={hasMore ? -1 : emails.length}
              showRecipients={list.state.folderKind !== "other"}
            />
          );
        })}
      </ul>
      {hasMore ? (
        <div {...stylex.props(styles.loadMore)}>
          <Button
            loading={list.state.loadingMore}
            onClick={() => {
              loadMore();
            }}
            size="sm"
            variant="ghost"
          >
            {m.list_load_more()}
          </Button>
        </div>
      ) : null}
    </div>
  );
};

const Body = () => {
  const list = useList();
  const compact =
    useRouteContext({ from: "__root__" }).preferences.density === "compact";
  if (list.state.view?.kind === "missing" && list.state.query === "") {
    return (
      <div {...stylex.props(styles.center)}>
        <EmptyState title={specialLabel(list.state.view.role) ?? ""}>
          {m.archive_missing()}
        </EmptyState>
      </div>
    );
  }
  if (list.state.status === "unsupported") {
    // No "Retry": the server will refuse this search again.
    return (
      <div {...stylex.props(styles.center)}>
        <EmptyState title={m.search_unsupported()} />
      </div>
    );
  }
  if (list.state.status === "error") {
    return (
      <div {...stylex.props(styles.center)}>
        <EmptyState
          action={
            <Button
              onClick={() => {
                list.actions.refetch();
              }}
              size="sm"
            >
              {m.retry()}
            </Button>
          }
          title={
            list.state.view === null && list.state.mailboxesLoaded
              ? m.mailbox_missing()
              : m.list_error()
          }
        />
      </div>
    );
  }
  if (list.state.status === "pending") {
    return <ListSkeleton compact={compact} />;
  }
  if (list.state.emails.length === 0) {
    return (
      <div {...stylex.props(styles.center)}>
        <Empty />
      </div>
    );
  }
  return <Rows />;
};

const Heading = ({ children }: { readonly children: ReactNode }) => (
  <div {...stylex.props(styles.headingBar)}>
    <h1 {...stylex.props(styles.heading, shared.ellipsis)}>{children}</h1>
    <ListTools place="title" />
  </div>
);

export const ListPanel = () => {
  const list = useList();
  const pane = useReadingPane();
  const { view } = list.state;
  // An open email hides the list on narrow screens, and always in "full".
  let openStyle = null;
  if (list.state.openId !== undefined) {
    openStyle = pane === "full" ? styles.panelGone : styles.panelHidden;
  }
  // The inbox's tabs, also while the folders load (the address says
  // "inbox"), so the header doesn't grow when they arrive.
  const inbox =
    view?.kind === "mailbox"
      ? view.mailbox.role === "inbox"
      : view === null &&
        !list.state.mailboxesLoaded &&
        list.state.mailbox === "inbox";
  const title = (() => {
    if (list.state.query !== "") {
      return m.search_results({ query: list.state.query });
    }
    if (view?.kind === "mailbox") {
      return mailboxLabel(view.mailbox);
    }
    return specialLabel(list.state.mailbox) ?? "";
  })();

  return (
    <Panel
      label={m.list_label()}
      xstyle={[
        styles.panel,
        styles.panelMobile,
        pane === "full" ? styles.panelFull : null,
        openStyle,
      ]}
    >
      <VacationBanner />
      {inbox && list.state.query === "" ? (
        <Tabs
          onValueChange={(split: string) => {
            list.actions.setSearch({ split });
          }}
          value={list.state.split.id}
        >
          <div {...stylex.props(styles.titleRow)}>
            <h1 {...stylex.props(styles.heading, styles.mobileTitle)}>
              {title}
            </h1>
            <ListTools place="title" />
          </div>
          <div {...stylex.props(stylex.defaultMarker(), styles.header)}>
            <TabList flush label={m.splits_label()}>
              {list.state.splits.map((split) => {
                const icon = SPLIT_ICON[split.id] ?? Zap;
                const count =
                  split.countUnread && view?.kind === "mailbox"
                    ? view.mailbox.unread
                    : undefined;
                return (
                  <Tab
                    icon={icon}
                    key={split.id}
                    value={split.id}
                    {...(count === undefined || count === 0 ? {} : { count })}
                  >
                    {SPLIT_LABEL[split.id]?.() ?? split.id}
                  </Tab>
                );
              })}
            </TabList>
            <ListTools place="header" />
          </div>
          <BulkBar />
          <TabPanel value={list.state.split.id}>
            <div {...stylex.props(styles.tabsWrap)}>
              <Body />
            </div>
          </TabPanel>
        </Tabs>
      ) : (
        <>
          <Heading>{title}</Heading>
          <div
            {...stylex.props(
              stylex.defaultMarker(),
              styles.header,
              styles.headerNoTabs
            )}
          >
            <ListTools place="header" />
          </div>
          <BulkBar />
          <Body />
        </>
      )}
    </Panel>
  );
};
