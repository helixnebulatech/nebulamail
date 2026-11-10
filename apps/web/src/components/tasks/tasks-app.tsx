// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Tasks app (/tasks): sidebar (views and lists), the
// task list and the open task next to it, with the same app rail, ⌘K
// palette, composer and reminders as mail and the calendar. Keyboard
// first: c/n new task, j/k, x, Enter, e, #, ! 0–3, d, v, / search, ?
// lists them all. Phones: one column, the sidebar in a drawer, the task
// instead of the list, a "Task" button. Only in the browser (dates depend
// on its zone and language).

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  ComposeButton,
  DateLocale,
  Dialog,
  IconButton,
  Menu,
  MenuItem,
  Panel,
  PortalScope,
  SearchField,
  Shortcut,
  Spinner,
} from "@nebulamail/ui";
import { colors, shadow, zIndex } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import {
  CalendarDays,
  CalendarPlus,
  CalendarRange,
  CheckCheck,
  Flag,
  Inbox,
  Keyboard,
  ListTodo,
  Mail,
  Plus,
  Search,
  Settings2,
  Share2,
  Sun,
  Users,
} from "lucide-react";
import { useRef, useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";

import { useCalendars } from "../../calendar/hooks";
import type { CalendarSummary } from "../../calendar/model";
import { useShortcut } from "../../lib/shortcuts";
import { useModalDialog } from "../../lib/use-modal-dialog";
import { useScrollShrink } from "../../lib/use-scroll-shrink";
import { m } from "../../paraglide/messages.js";
import { usePreferencesSync } from "../../prefs-sync/use-prefs-sync";
import { useTasksAvailable, useTasksPush } from "../../tasks/hooks";
import type { TaskList } from "../../tasks/model";
import type { TaskViewName } from "../../tasks/nav";
import { TASK_VIEWS } from "../../tasks/nav";
import { colorVars } from "../calendar/event-chip";
import { CalendarReminders } from "../calendar/reminders";
import { ShareCalendarDialog } from "../calendar/share-calendar-dialog";
import { useDateLocale } from "../calendar/use-calendar-prefs";
import { AppBar, AppBarTitle } from "../mail/app-bar";
import { AppRail } from "../mail/app-rail";
import { bp } from "../mail/breakpoints.stylex";
import { CommandPalette } from "../mail/command-palette";
import type { Command } from "../mail/commands";
import { CommandsProvider, useCommands } from "../mail/commands";
import { ComposeProvider } from "../mail/compose-provider";
import { ComposerDock } from "../mail/composer-dock";
import { LinkConfirmProvider } from "../mail/link-dialog";
import { MailUiProvider, isInOverlay, useMailUi } from "../mail/mail-ui";
import { useKeys } from "../mail/use-apple";
import {
  AddCalendarDialog,
  DeleteListDialog,
  ListDialog,
  useRemoveFromTasks,
} from "./list-dialogs";
import { priorityLabel, viewLabel } from "./task-format";
import { TaskList as TaskListView } from "./task-list";
import { TaskPanel } from "./task-panel";
import { rowStyles } from "./task-row";
import { TasksSidebar } from "./tasks-sidebar";
import { useTaskActions } from "./use-task-actions";
import { useTasksNav } from "./use-tasks-nav";

const styles = stylex.create({
  drawer: {
    "::backdrop": {
      backdropFilter: "blur(3px)",
      backgroundColor: colors.scrim,
    },
    backgroundColor: colors.side,
    borderStyle: "none",
    boxShadow: shadow.sh3,
    color: colors.fg,
    height: "100dvh",
    insetBlock: 0,
    insetInlineEnd: "auto",
    insetInlineStart: 0,
    margin: 0,
    maxHeight: "100dvh",
    maxWidth: "86vw",
    padding: 0,
    paddingBottom: "env(safe-area-inset-bottom)",
    paddingTop: "env(safe-area-inset-top)",
    position: "fixed",
    width: "280px",
  },
  fab: {
    bottom: "calc(20px + env(safe-area-inset-bottom))",
    display: { default: "none", [bp.mobile]: "block" },
    insetInlineEnd: "18px",
    position: "fixed",
    zIndex: zIndex.raised,
  },
  header: { gridArea: "top", minWidth: 0 },
  helpGrid: {
    columnGap: "28px",
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
    rowGap: "4px",
  },
  helpRow: {
    alignItems: "center",
    columnGap: "12px",
    display: "flex",
    justifyContent: "space-between",
    minHeight: "30px",
  },
  helpTerm: { color: colors.fg, margin: 0 },
  hidden: { display: "none" },
  listPanel: { flexBasis: 0, flexGrow: 1, minWidth: 0 },
  main: {
    display: "flex",
    gap: "10px",
    gridArea: "main",
    minHeight: 0,
    minWidth: 0,
    paddingBottom: { default: "10px", [bp.mobile]: 0 },
    paddingInlineEnd: { default: "10px", [bp.mobile]: 0 },
    paddingInlineStart: { default: "2px", [bp.mobile]: 0 },
  },
  narrowHidden: { display: { default: "flex", [bp.narrow]: "none" } },
  notAvailable: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    justifyContent: "center",
    padding: "32px",
    rowGap: "12px",
    textAlign: "center",
  },
  panel: {
    flexBasis: { default: "420px", [bp.narrow]: "100%" },
    flexGrow: { default: 0, [bp.narrow]: 1 },
    flexShrink: 0,
    maxWidth: "100%",
  },
  rail: {
    display: { default: "block", [bp.narrow]: "none" },
    gridArea: "rail",
    minHeight: 0,
  },
  root: {
    backgroundColor: colors.canvas,
    boxSizing: "border-box",
    color: colors.fg,
    display: "grid",
    gridTemplateAreas: {
      default: '"side top rail" "side main rail"',
      [bp.narrow]: '"side top" "side main"',
      [bp.mobile]: '"top" "main"',
    },
    gridTemplateColumns: {
      default: "252px minmax(0, 1fr) 52px",
      // The same columns as the mail shell (shell.tsx).
      [bp.narrow]: "252px minmax(0, 1fr)",
      [bp.mobile]: "minmax(0, 1fr)",
    },
    gridTemplateRows: "60px minmax(0, 1fr)",
    height: "100dvh",
    overflow: "hidden",
    paddingBottom: "env(safe-area-inset-bottom)",
    paddingInlineEnd: "env(safe-area-inset-right)",
    paddingInlineStart: "env(safe-area-inset-left)",
    paddingTop: "env(safe-area-inset-top)",
  },
  // The app bar's search column (app-bar.tsx): same place and width as
  // mail's search.
  search: { display: "block", flexGrow: 1, minWidth: 0, width: "100%" },
  side: {
    backgroundColor: colors.side,
    display: { default: "block", [bp.mobile]: "none" },
    gridArea: "side",
    minHeight: 0,
    overflow: "hidden",
  },
  titleDot: { height: "12px", width: "12px" },
});

const VIEW_ICONS = {
  all: Inbox,
  done: CheckCheck,
  today: Sun,
  upcoming: CalendarRange,
} as const;

const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

const HELP: readonly { readonly spec: string; readonly label: () => string }[] =
  [
    { label: m.tasks_new_task, spec: "c" },
    { label: m.tasks_sc_next, spec: "j" },
    { label: m.tasks_sc_previous, spec: "k" },
    { label: m.tasks_sc_toggle, spec: "x" },
    { label: m.tasks_sc_toggle, spec: "space" },
    { label: m.tasks_sc_open, spec: "enter" },
    { label: m.tasks_sc_edit_title, spec: "e" },
    { label: m.tasks_delete, spec: "#" },
    { label: m.tasks_sc_priority, spec: "! 3" },
    { label: m.tasks_sc_due, spec: "d" },
    { label: m.tasks_sc_move, spec: "v" },
    { label: m.tasks_sc_indent, spec: "tab" },
    { label: m.tasks_sc_outdent, spec: "shift+tab" },
    { label: m.tasks_sc_reorder, spec: "alt+arrowdown" },
    { label: m.tasks_sc_search, spec: "/" },
    { label: m.tasks_sc_save, spec: "mod+enter" },
    { label: m.tasks_sc_close, spec: "escape" },
    { label: m.tasks_go_tasks, spec: "g k" },
    { label: m.cal_go_mail, spec: "g i" },
    { label: m.rail_calendar, spec: "g c" },
    { label: m.sc_palette, spec: "mod+k" },
    { label: m.sc_help, spec: "?" },
  ];

/** Phones: "New task", shrinking to its icon while the list scrolls down. */
const TasksFab = ({ onClick }: { readonly onClick: () => void }) => {
  const shrunk = useScrollShrink();
  return (
    <ComposeButton
      icon={Plus}
      collapsed={shrunk ? "fab" : false}
      onClick={onClick}
    >
      {m.tasks_new_task()}
    </ComposeButton>
  );
};

const TasksHelp = () => {
  const ui = useMailUi();
  const keys = useKeys();
  return (
    <Dialog.Root
      open={ui.helpOpen}
      onOpenChange={(open) => {
        ui.setHelpOpen(open);
      }}
    >
      <Dialog.Content closeLabel={m.close()} size="lg">
        <Dialog.Title>{m.tasks_shortcuts()}</Dialog.Title>
        <Dialog.Body>
          <dl {...stylex.props(styles.helpGrid)}>
            {HELP.map((entry) => (
              <div key={entry.spec} {...stylex.props(styles.helpRow)}>
                <dt {...stylex.props(styles.helpTerm)}>{entry.label()}</dt>
                <dd {...stylex.props(styles.helpTerm)}>
                  <Shortcut keys={keys(entry.spec)} />
                </dd>
              </div>
            ))}
          </dl>
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};

const Drawer = ({
  children,
  onClose,
  open,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly children: ReactNode;
}) => {
  const dialog = useModalDialog(open);
  return (
    <dialog
      aria-label={m.tasks_sidebar()}
      onClose={onClose}
      ref={dialog}
      {...stylex.props(styles.drawer)}
    >
      {/* Menus and dialogs of the sidebar must open inside the drawer:
          the page behind a modal <dialog> is inert. */}
      {open ? <PortalScope>{children}</PortalScope> : null}
    </dialog>
  );
};

const Toolbar = ({
  heading,
  list,
  onMenu,
  onSearch,
  search,
}: {
  readonly heading: string;
  readonly list: TaskList | undefined;
  readonly search: string;
  readonly onSearch: (text: string) => void;
  readonly onMenu: () => void;
}) => {
  const ui = useMailUi();
  const keys = useKeys();
  const navigate = useNavigate();
  const title = (
    <AppBarTitle
      before={
        list === undefined ? null : (
          <span
            aria-hidden
            {...stylex.props(
              rowStyles.dot,
              styles.titleDot,
              colorVars.color(list.color)
            )}
          />
        )
      }
      heading
    >
      {heading}
    </AppBarTitle>
  );
  const searchBox = (
    <search {...stylex.props(styles.search)}>
      <SearchField
        hint={<Shortcut keys={keys("/")} />}
        label={m.tasks_search()}
        onChange={(event) => {
          onSearch(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            onSearch("");
            event.currentTarget.blur();
          }
        }}
        placeholder={m.tasks_search_placeholder()}
        ref={(element) => {
          // "/" focuses this box while Tasks is open.
          ui.registerSearch(
            element instanceof HTMLInputElement ? element : null
          );
        }}
        value={search}
      />
    </search>
  );
  const extras = (
    <Menu
      align="end"
      trigger={<IconButton icon={Settings2} label={m.tasks_settings()} />}
    >
      <MenuItem
        icon={Keyboard}
        onClick={() => {
          ui.setHelpOpen(true);
        }}
        shortcut={["?"]}
      >
        {m.tasks_shortcuts()}
      </MenuItem>
      <MenuItem
        icon={CalendarDays}
        onClick={() => {
          void navigate({ to: "/calendar" });
        }}
      >
        {m.tasks_open_calendar()}
      </MenuItem>
    </Menu>
  );
  return (
    <AppBar
      extras={extras}
      menuLabel={m.tasks_sidebar()}
      onMenu={onMenu}
      search={searchBox}
      start={title}
    />
  );
};

/** The open list's "Share…" in ⌘K (as in the sidebar's menu). */
const shareListCommands = (
  list: TaskList | undefined,
  calendars: readonly CalendarSummary[] | undefined,
  onShare: (list: TaskList) => void
): Command[] =>
  list !== undefined &&
  calendars?.find((calendar) => calendar.key === list.key)?.mayShare === true
    ? [
        {
          group: "actions",
          icon: Share2,
          id: "tasks-share-list",
          keywords: "share compartir",
          label: m.tasks_share_command({ name: cleanDisplayText(list.name) }),
          run: () => {
            onShare(list);
          },
        },
      ]
    : [];

const Layout = () => {
  const nav = useTasksNav();
  const ui = useMailUi();
  const navigate = useNavigate();
  const available = useTasksAvailable() !== false;
  const actions = useTaskActions();
  const [drawer, setDrawer] = useState(false);
  const [editingList, setEditingList] = useState<{
    readonly list: TaskList | null;
  } | null>(null);
  const [deletingList, setDeletingList] = useState<TaskList | null>(null);
  const [addingCalendar, setAddingCalendar] = useState(false);
  const [sharingList, setSharingList] = useState<TaskList | null>(null);
  const calendars = useCalendars();
  const [search, setSearch] = useState("");
  const quickAdd = useRef<HTMLInputElement>(null);
  useTasksPush();
  usePreferencesSync();
  const removeFromTasks = useRemoveFromTasks((item) => {
    if (nav.scope.kind === "list" && nav.scope.key === item.key) {
      nav.setView("today");
    }
  });

  const lists = actions.snapshot?.lists ?? [];
  const list =
    nav.scope.kind === "list"
      ? lists.find(
          (item) => nav.scope.kind === "list" && item.key === nav.scope.key
        )
      : undefined;
  const heading =
    nav.scope.kind === "view"
      ? viewLabel(nav.scope.view)
      : cleanDisplayText(list?.name ?? "") || m.tasks_list();

  const newTask = (): void => {
    nav.openTask(null);
    // After the panel (on phones, instead of the list) closes.
    setTimeout(() => {
      quickAdd.current?.focus();
    }, 0);
  };
  const goView = (view: TaskViewName): void => {
    nav.setView(view);
  };

  const onNew = (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      newTask();
    }
  };
  useShortcut("c", onNew);
  useShortcut("n", onNew);
  useShortcut("mod+k", () => {
    ui.setPaletteOpen(!ui.paletteOpen);
  });
  useShortcut(
    "/",
    guard(() => {
      ui.focusSearch();
    })
  );
  useShortcut(
    "?",
    guard(() => {
      ui.setHelpOpen(true);
    })
  );
  useShortcut(
    "g i",
    guard(() => {
      void navigate({ params: { mailbox: "inbox" }, to: "/mail/$mailbox" });
    })
  );
  useShortcut(
    "g k",
    guard(() => {
      goView("today");
    })
  );
  useShortcut(
    "g c",
    guard(() => {
      void navigate({ to: "/calendar" });
    })
  );

  const commands: Command[] = [
    {
      group: "actions",
      icon: Plus,
      id: "tasks-new",
      keywords: "task tarea nueva new",
      label: m.tasks_new_task(),
      run: newTask,
      shortcut: "c",
    },
    {
      group: "actions",
      icon: Search,
      id: "tasks-search",
      label: m.tasks_sc_search(),
      run: () => {
        setTimeout(ui.focusSearch, 0);
      },
      shortcut: "/",
    },
    {
      group: "actions",
      icon: Plus,
      id: "tasks-new-list",
      label: m.tasks_new_list(),
      run: () => {
        setEditingList({ list: null });
      },
    },
    {
      group: "actions",
      icon: CalendarPlus,
      id: "tasks-add-calendar",
      keywords: "calendar calendario list lista",
      label: m.tasks_add_calendar(),
      run: () => {
        setAddingCalendar(true);
      },
    },
    ...TASK_VIEWS.map((view) => ({
      group: "goto" as const,
      icon: VIEW_ICONS[view],
      id: `tasks-view-${view}`,
      label: viewLabel(view),
      run: () => {
        goView(view);
      },
      ...(view === "today" ? { shortcut: "g k" } : {}),
    })),
    ...shareListCommands(list, calendars.data, setSharingList),
    ...lists.map((item) => ({
      group: "goto" as const,
      icon: ListTodo,
      id: `tasks-list-${item.key}`,
      keywords: "list lista",
      label: cleanDisplayText(item.name),
      run: () => {
        nav.setList(item.key);
      },
    })),
    {
      group: "goto",
      icon: Mail,
      id: "goto-mail",
      label: m.cal_go_mail(),
      run: () => {
        void navigate({ params: { mailbox: "inbox" }, to: "/mail/$mailbox" });
      },
      shortcut: "g i",
    },
    {
      group: "goto",
      icon: CalendarDays,
      id: "goto-calendar",
      keywords: "calendar calendario",
      label: m.rail_calendar(),
      run: () => {
        void navigate({ to: "/calendar" });
      },
      shortcut: "g c",
    },
    {
      group: "goto",
      icon: Users,
      id: "goto-contacts",
      keywords: "contacts contactos",
      label: m.rail_contacts(),
      run: () => {
        void navigate({ to: "/contacts" });
      },
    },
    {
      group: "actions",
      icon: Keyboard,
      id: "help",
      label: m.tasks_shortcuts(),
      run: () => {
        ui.setHelpOpen(true);
      },
      shortcut: "?",
    },
  ];
  useCommands(commands);
  // The open (or focused) task's actions in the palette.
  const open =
    nav.open === null
      ? undefined
      : actions.snapshot?.tasks.find(
          (task) =>
            task.id === nav.open?.id && task.accountId === nav.open.accountId
        );
  useCommands(
    open === undefined || !actions.canEdit(open)
      ? []
      : [
          {
            group: "actions",
            icon: CheckCheck,
            id: "task-toggle",
            label: open.completed
              ? m.tasks_mark_not_done()
              : m.tasks_mark_done({ title: cleanDisplayText(open.title) }),
            run: () => {
              actions.complete(open, !open.completed);
            },
            shortcut: "x",
          },
          ...(["high", "medium", "low", "none"] as const).map((priority) => ({
            group: "actions" as const,
            icon: Flag,
            id: `task-priority-${priority}`,
            keywords: "priority prioridad",
            label: m.tasks_set_priority({ priority: priorityLabel(priority) }),
            run: () => {
              actions.setPriority(open, priority);
            },
          })),
          {
            group: "actions",
            icon: Settings2,
            id: "task-delete",
            label: m.tasks_delete(),
            run: () => {
              nav.openTask(null);
              actions.remove(open);
            },
            shortcut: "#",
          },
        ]
  );

  const panelOpen = nav.open !== null;
  const sidebar = (onNavigate?: () => void) => (
    <TasksSidebar
      nav={nav}
      onAddCalendar={() => {
        setDrawer(false);
        setAddingCalendar(true);
      }}
      onDeleteList={(item) => {
        // Dialogs open over the page: the drawer (modal) closes first.
        setDrawer(false);
        setDeletingList(item);
      }}
      onEditList={(item) => {
        setDrawer(false);
        setEditingList({ list: item });
      }}
      onShareList={(item) => {
        setDrawer(false);
        setSharingList(item);
      }}
      onNewTask={newTask}
      onRemoveList={(item) => {
        setDrawer(false);
        removeFromTasks(item);
      }}
      snapshot={actions.snapshot}
      {...(onNavigate === undefined ? {} : { onNavigate })}
    />
  );

  return (
    <div {...stylex.props(styles.root)}>
      <div {...stylex.props(styles.side)}>{sidebar()}</div>
      <header {...stylex.props(styles.header)}>
        <Toolbar
          heading={heading}
          list={list}
          onMenu={() => {
            setDrawer(true);
          }}
          onSearch={setSearch}
          search={search}
        />
      </header>
      <main {...stylex.props(styles.main)}>
        {available ? (
          <>
            <Panel
              label={m.tasks_title_app()}
              xstyle={[
                styles.listPanel,
                panelOpen ? styles.narrowHidden : null,
              ]}
            >
              <TaskListView
                actions={actions}
                key={
                  nav.scope.kind === "list"
                    ? `list:${nav.scope.key}`
                    : nav.scope.view
                }
                nav={nav}
                quickAddRef={quickAdd}
                search={search}
              />
            </Panel>
            {nav.open === null ? null : (
              <Panel label={m.tasks_task()} xstyle={styles.panel}>
                <TaskPanel
                  actions={actions}
                  onClose={() => {
                    nav.openTask(null);
                  }}
                  onOpen={(task) => {
                    nav.openTask(task);
                  }}
                  open={nav.open}
                />
              </Panel>
            )}
          </>
        ) : (
          <Panel xstyle={styles.listPanel}>
            <div {...stylex.props(styles.notAvailable)}>
              <p>{m.tasks_not_available()}</p>
            </div>
          </Panel>
        )}
      </main>
      <div {...stylex.props(styles.rail)}>
        <AppRail />
      </div>
      <span {...stylex.props(styles.fab, panelOpen ? styles.hidden : null)}>
        <TasksFab onClick={newTask} />
      </span>
      <ComposerDock />
      <CommandPalette />
      <TasksHelp />
      <CalendarReminders />
      <ListDialog
        list={editingList?.list ?? null}
        onCreated={(key) => {
          nav.setList(key);
        }}
        onOpenChange={(isOpen) => {
          if (!isOpen) {
            setEditingList(null);
          }
        }}
        open={editingList !== null}
      />
      <AddCalendarDialog
        lists={lists}
        onAdded={(key) => {
          nav.setList(key);
        }}
        onOpenChange={setAddingCalendar}
        open={addingCalendar}
      />
      <ShareCalendarDialog
        calendar={sharingList}
        kind="list"
        onOpenChange={(isOpen) => {
          if (!isOpen) {
            setSharingList(null);
          }
        }}
      />
      <DeleteListDialog
        list={deletingList}
        onDeleted={() => {
          if (deletingList !== null && list?.key === deletingList.key) {
            nav.setView("today");
          }
        }}
        onOpenChange={(isOpen) => {
          if (!isOpen) {
            setDeletingList(null);
          }
        }}
      />
      <Drawer
        onClose={() => {
          setDrawer(false);
        }}
        open={drawer}
      >
        {sidebar(() => {
          setDrawer(false);
        })}
      </Drawer>
    </div>
  );
};

const noSubscribe = (): (() => void) => () => {
  // Nothing to stop: the value never changes.
};

/** True in the browser after hydration (see the calendar app). */
const useInBrowser = (): boolean =>
  useSyncExternalStore(
    noSubscribe,
    () => true,
    () => false
  );

export const TasksApp = () => {
  const locale = useDateLocale();
  const inBrowser = useInBrowser();
  if (!inBrowser) {
    return (
      <div {...stylex.props(styles.root)}>
        <div {...stylex.props(styles.side)} />
        <main {...stylex.props(styles.main, styles.notAvailable)}>
          <Spinner label={m.tasks_loading()} />
        </main>
      </div>
    );
  }
  return (
    <DateLocale locale={locale}>
      <MailUiProvider>
        <CommandsProvider>
          <ComposeProvider>
            <LinkConfirmProvider>
              <Layout />
            </LinkConfirmProvider>
          </ComposeProvider>
        </CommandsProvider>
      </MailUiProvider>
    </DateLocale>
  );
};
