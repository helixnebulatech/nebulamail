// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Left side of Tasks: the brand, "New task", the views
// (Today, Upcoming, All, Completed) and the lists with their colours and
// open tasks: the user's own ("My lists": create, add an existing
// calendar, rename, recolour, delete) and those others share, by owner (only what `myRights` allow).

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  ComposeButton,
  IconButton,
  Menu,
  MenuItem,
  NavItem,
  Spinner,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  CalendarPlus,
  CalendarRange,
  CheckCheck,
  Inbox,
  ListMinus,
  ListPlus,
  MoreHorizontal,
  Palette,
  PenLine,
  Plus,
  Share2,
  Sun,
  Trash2,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { useCalendars, useTimeZone } from "../../calendar/hooks";
import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import type { TaskSnapshot } from "../../tasks/adapter";
import type { TaskList } from "../../tasks/model";
import type { TaskViewName } from "../../tasks/nav";
import { TASK_VIEWS } from "../../tasks/nav";
import { todayCount } from "../../tasks/views";
import { SafeText } from "../calendar/event-text";
import { sidebarLayout } from "../mail/sidebar";
import { SidebarSection } from "../mail/sidebar-section";
import { SidebarTop } from "../mail/sidebar-top";
import { shared } from "../mail/styles";
import { useDragOverList } from "./task-drag";
import { viewLabel } from "./task-format";
import type { TasksNav } from "./use-tasks-nav";

const styles = stylex.create({
  hint: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    margin: 0,
    // On the label column, like the mail sidebar's secondary text.
    paddingInlineStart: "36px",
  },
  item: {
    alignItems: "center",
    borderRadius: radius.md,
    display: "flex",
    minWidth: 0,
    position: "relative",
  },
  itemDrop: { boxShadow: `0 0 0 2px ${colors.acc}` },
  itemMain: { flexGrow: 1, minWidth: 0 },
  // Like the calendar list: the "⋯" lies over the count (no width kept for
  // it, so counts line up with the views' and names aren't cut short) and
  // shows on the row's hover or focus (always on touch screens, which have
  // no hover); the count steps aside meanwhile (NavItem menuOverCount).
  itemMenu: {
    insetInlineEnd: "4px",
    opacity: {
      default: 0,
      [stylex.when.ancestor(":hover")]: 1,
      [stylex.when.ancestor(":focus-within")]: 1,
      [touch.noHover]: 1,
    },
    position: "absolute",
    top: "50%",
    transform: "translateY(-50%)",
  },
  list: { display: "flex", flexDirection: "column", rowGap: "1px" },
  root: {
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    height: "100%",
    overflowY: "auto",
    paddingBottom: "14px",
    paddingInline: "12px",
    paddingTop: "12px",
    rowGap: "16px",
  },
});

const VIEW_ICONS: Readonly<Record<TaskViewName, LucideIcon>> = {
  all: Inbox,
  done: CheckCheck,
  today: Sun,
  upcoming: CalendarRange,
};

const ListItems = ({
  active,
  lists,
  mayDelete,
  mayShare,
  onDelete,
  onEdit,
  onOpen,
  onRemove,
  onShare,
}: {
  readonly lists: readonly TaskList[];
  readonly active: string | null;
  readonly mayDelete: (list: TaskList) => boolean;
  readonly mayShare: (list: TaskList) => boolean;
  readonly onOpen: (list: TaskList) => void;
  readonly onEdit: (list: TaskList) => void;
  readonly onShare: (list: TaskList) => void;
  readonly onRemove: (list: TaskList) => void;
  readonly onDelete: (list: TaskList) => void;
}) => {
  // A task dragged over a list (task-drag.tsx) lights it up.
  const over = useDragOverList();
  return (
    <ul {...stylex.props(shared.resetList, styles.list)}>
      {lists.map((list) => {
        const name = cleanDisplayText(list.name);
        return (
          <li
            data-task-drop-list={list.key}
            key={list.key}
            {...stylex.props(
              styles.item,
              over === list.key ? styles.itemDrop : null,
              stylex.defaultMarker()
            )}
          >
            <span {...stylex.props(styles.itemMain)}>
              <NavItem
                active={active === list.key}
                color={list.color}
                count={list.open}
                menuOverCount
                mutedCount
                onClick={() => {
                  onOpen(list);
                }}
                title={name}
              >
                <SafeText text={list.name} />
              </NavItem>
            </span>
            <span {...stylex.props(styles.itemMenu)}>
              <Menu
                align="end"
                trigger={
                  <IconButton
                    icon={MoreHorizontal}
                    label={m.tasks_list_options({ name })}
                    size="xs"
                    tooltip={false}
                  />
                }
              >
                <MenuItem
                  icon={list.owner === null ? PenLine : Palette}
                  onClick={() => {
                    onEdit(list);
                  }}
                >
                  {list.owner === null
                    ? m.tasks_edit_list_menu()
                    : m.cal_change_color()}
                </MenuItem>
                {mayShare(list) ? (
                  <MenuItem
                    icon={Share2}
                    onClick={() => {
                      onShare(list);
                    }}
                  >
                    {m.cal_share_menu()}
                  </MenuItem>
                ) : null}
                <MenuItem
                  icon={ListMinus}
                  onClick={() => {
                    onRemove(list);
                  }}
                >
                  {m.tasks_remove_list()}
                </MenuItem>
                {mayDelete(list) ? (
                  <MenuItem
                    danger
                    icon={Trash2}
                    onClick={() => {
                      onDelete(list);
                    }}
                  >
                    {m.tasks_delete_list()}
                  </MenuItem>
                ) : null}
              </Menu>
            </span>
          </li>
        );
      })}
    </ul>
  );
};

export const TasksSidebar = ({
  nav,
  onDeleteList,
  onAddCalendar,
  onEditList,
  onNavigate,
  onNewTask,
  onRemoveList,
  onShareList,
  snapshot,
}: {
  readonly nav: TasksNav;
  readonly snapshot: TaskSnapshot | undefined;
  readonly onNewTask: () => void;
  /** New list (null) or its name and colour. */
  readonly onEditList: (list: TaskList | null) => void;
  /** "Add a calendar…": show an existing calendar as a list. */
  readonly onAddCalendar: () => void;
  readonly onDeleteList: (list: TaskList) => void;
  /** "Remove from Tasks": the calendar stays. */
  readonly onRemoveList: (list: TaskList) => void;
  readonly onShareList: (list: TaskList) => void;
  /** Called after choosing something (closes the phone drawer). */
  readonly onNavigate?: () => void;
}) => {
  const zone = useTimeZone();
  const now = useNow();
  const calendars = useCalendars();
  const lists = snapshot?.lists ?? [];
  const own = lists.filter((list) => list.owner === null);
  const owners = Object.entries(
    Object.groupBy(
      lists.filter((list) => list.owner !== null),
      (list) => list.owner ?? ""
    )
  ).flatMap(([owner, group]) =>
    group === undefined ? [] : [[owner, group] as const]
  );
  const counts: Partial<Record<TaskViewName, number>> = {
    today: snapshot === undefined ? 0 : todayCount(snapshot.tasks, now, zone),
  };
  const activeList = nav.scope.kind === "list" ? nav.scope.key : null;
  const mayDelete = (list: TaskList): boolean =>
    calendars.data?.find((calendar) => calendar.key === list.key)?.mayDelete ===
    true;
  const mayShare = (list: TaskList): boolean =>
    calendars.data?.find((calendar) => calendar.key === list.key)?.mayShare ===
    true;
  const openList = (list: TaskList): void => {
    nav.setList(list.key);
    onNavigate?.();
  };

  return (
    <nav aria-label={m.tasks_sidebar()} {...stylex.props(styles.root)}>
      <SidebarTop
        current="tasks"
        onNavigate={onNavigate}
        xstyle={sidebarLayout.composeInGap16}
      >
        <ComposeButton
          icon={Plus}
          onClick={() => {
            onNewTask();
            onNavigate?.();
          }}
          shortcut={["C"]}
        >
          {m.tasks_new_task()}
        </ComposeButton>
      </SidebarTop>
      <ul
        aria-label={m.tasks_views()}
        {...stylex.props(shared.resetList, styles.list)}
      >
        {TASK_VIEWS.map((view) => (
          <li key={view}>
            <NavItem
              active={nav.scope.kind === "view" && nav.scope.view === view}
              count={counts[view] ?? 0}
              icon={VIEW_ICONS[view]}
              onClick={() => {
                nav.setView(view);
                onNavigate?.();
              }}
            >
              {viewLabel(view)}
            </NavItem>
          </li>
        ))}
      </ul>
      <section aria-label={m.tasks_my_lists()}>
        <SidebarSection
          action={
            <Menu
              align="end"
              trigger={
                <IconButton icon={Plus} label={m.tasks_add_list()} size="xs" />
              }
            >
              <MenuItem
                icon={ListPlus}
                onClick={() => {
                  onEditList(null);
                }}
              >
                {m.tasks_new_list()}
              </MenuItem>
              <MenuItem icon={CalendarPlus} onClick={onAddCalendar}>
                {m.tasks_add_calendar()}
              </MenuItem>
            </Menu>
          }
          flush
          onToggle={null}
          open
        >
          {m.tasks_my_lists()}
        </SidebarSection>
        {snapshot === undefined ? <Spinner label={m.tasks_loading()} /> : null}
        {snapshot !== undefined && own.length === 0 ? (
          <p {...stylex.props(styles.hint)}>{m.tasks_no_lists()}</p>
        ) : null}
        <ListItems
          active={activeList}
          lists={own}
          mayDelete={mayDelete}
          mayShare={mayShare}
          onDelete={onDeleteList}
          onRemove={onRemoveList}
          onEdit={onEditList}
          onOpen={openList}
          onShare={onShareList}
        />
      </section>
      {owners.map(([owner, group]) => (
        <section aria-label={m.cal_shared_by({ owner })} key={owner}>
          <SidebarSection flush onToggle={null} open>
            <SafeText text={m.cal_shared_by({ owner })} />
          </SidebarSection>
          <ListItems
            active={activeList}
            lists={group}
            mayDelete={mayDelete}
            mayShare={mayShare}
            onDelete={onDeleteList}
            onRemove={onRemoveList}
            onEdit={onEditList}
            onOpen={openList}
            onShare={onShareList}
          />
        </section>
      ))}
    </nav>
  );
};
