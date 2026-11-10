// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The calendar app (/calendar): sidebar, toolbar, the view (day, week,
// month, agenda) and the open event next to it, with the same app rail,
// ⌘K palette and composer as mail. Keyboard first: D/W/M/A change the
// view, T goes to today, J/K (or N/P) move, C creates, ? lists them all.
// Phones: one column, the sidebar in a drawer, the event instead of the
// view. Data is only fetched in the browser.

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Button,
  ComposeButton,
  DateLocale,
  Dialog,
  IconButton,
  Menu,
  MenuGroup,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  Panel,
  PortalScope,
  SegmentedControl,
  Shortcut,
  Spinner,
} from "@nebulamail/ui";
import { colors, font, shadow, zIndex } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  Bell,
  CalendarDays,
  CalendarPlus,
  CalendarRange,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Columns3,
  Keyboard,
  List,
  ListTodo,
  Mail,
  Settings2,
  Share2,
  Square,
} from "lucide-react";
import { useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";

import { calendarsAvailable } from "../../calendar/api";
import { canChangeOneOccurrence } from "../../calendar/editing";
import {
  calendarKeys,
  useCalendarPush,
  useCalendars,
  useOccurrences,
  useOwnCalendarAddresses,
  useTimeZone,
} from "../../calendar/hooks";
import type { CalendarView } from "../../calendar/layout";
import { VIEWS, daysOf, viewRange, weeksOf } from "../../calendar/layout";
import type { CalendarSummary, Occurrence } from "../../calendar/model";
import { holdsEvents } from "../../calendar/model";
import { refOf } from "../../calendar/targets";
import { useShortcut } from "../../lib/shortcuts";
import { useModalDialog } from "../../lib/use-modal-dialog";
import { useNow } from "../../lib/use-now";
import { useScrollShrink } from "../../lib/use-scroll-shrink";
import { m } from "../../paraglide/messages.js";
import { AppBar } from "../mail/app-bar";
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
import { usePhone } from "../mail/use-phone";
import { useCalendarTasks } from "../tasks/calendar-tasks";
import { AgendaView } from "./agenda-view";
import { CalendarSidebar } from "./calendar-sidebar";
import { EditEventEditor, NewEventEditor } from "./event-editor";
import { EventPanel } from "./event-panel";
import { rangeTitle, rangeTitleShort } from "./format";
import { MonthView } from "./month-view";
import { CalendarReminders, RemindersControl } from "./reminders";
import { ScopeDialog } from "./scope-dialog";
import { ShareCalendarDialog } from "./share-calendar-dialog";
import type { MoveRequest } from "./time-grid";
import { TimeGrid } from "./time-grid";
import { useCalendarNav } from "./use-calendar-nav";
import type { WeekStartChoice } from "./use-calendar-prefs";
import { WEEK_STARTS, useDateLocale, useWeekStart } from "./use-calendar-prefs";
import { eventFor, useEventActions } from "./use-event-actions";

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
    flexBasis: { default: "400px", [bp.narrow]: "100%" },
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
  side: {
    backgroundColor: colors.side,
    display: { default: "block", [bp.mobile]: "none" },
    gridArea: "side",
    minHeight: 0,
    overflow: "hidden",
  },
  title: {
    fontSize: { default: font.sizeXl, [bp.mobile]: font.sizeLg },
    fontWeight: font.weightBold,
    letterSpacing: font.trackingTight,
    margin: 0,
    // Room before the view switch when the range is cut short (phones).
    marginInlineEnd: "8px",
    minWidth: 0,
    overflow: "hidden",
    paddingInlineStart: "6px",
    textOverflow: "ellipsis",
    textTransform: "capitalize",
    whiteSpace: "nowrap",
  },
  // Phones: the short range ("Nov 8 – 14") takes what the row leaves.
  titleGrow: { flexGrow: { default: null, [bp.mobile]: 1 } },
  titleFull: { display: { default: "inline", [bp.mobile]: "none" } },
  titleShort: { display: { default: "none", [bp.mobile]: "inline" } },
  // Phones: ‹ › and the calendar settings move into the view menu, so the
  // row keeps Today, the range and one menu.
  desktopOnly: { display: { default: "inline-flex", [bp.mobile]: "none" } },
  viewPanel: { flexBasis: 0, flexGrow: 1, minWidth: 0 },
  viewSwitch: { display: { default: "flex", [bp.mobile]: "none" } },
  viewMenu: { display: { default: "none", [bp.mobile]: "inline-flex" } },
});

const VIEW_SHORTCUTS: Readonly<Record<CalendarView, string>> = {
  agenda: "a",
  day: "d",
  month: "m",
  week: "w",
};

const VIEW_ICONS = {
  agenda: List,
  day: Square,
  month: CalendarDays,
  week: Columns3,
} as const;

const viewLabel = (view: CalendarView): string => {
  switch (view) {
    case "day": {
      return m.cal_view_day();
    }
    case "week": {
      return m.cal_view_week();
    }
    case "month": {
      return m.cal_view_month();
    }
    case "agenda": {
      return m.cal_view_agenda();
    }
    default: {
      return view satisfies never;
    }
  }
};

const weekStartLabel = (choice: WeekStartChoice): string => {
  switch (choice) {
    case "auto": {
      return m.cal_week_start_auto();
    }
    case "mon": {
      return m.cal_week_start_mon();
    }
    case "sun": {
      return m.cal_week_start_sun();
    }
    case "sat": {
      return m.cal_week_start_sat();
    }
    default: {
      return choice satisfies never;
    }
  }
};

const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

const HELP: readonly { readonly spec: string; readonly label: () => string }[] =
  [
    { label: m.cal_new_event, spec: "c" },
    { label: m.cal_today, spec: "t" },
    { label: m.cal_next, spec: "j" },
    { label: m.cal_previous, spec: "k" },
    { label: m.cal_view_day, spec: "d" },
    { label: m.cal_view_week, spec: "w" },
    { label: m.cal_view_month, spec: "m" },
    { label: m.cal_view_agenda, spec: "a" },
    { label: m.cal_edit, spec: "e" },
    { label: m.cal_delete, spec: "#" },
    { label: m.cal_save, spec: "mod+enter" },
    { label: m.cal_move_hint, spec: "alt+arrowdown" },
    { label: m.cal_resize_hint, spec: "alt+shift+arrowdown" },
    { label: m.cal_go_mail, spec: "g i" },
    { label: m.tasks_go_tasks, spec: "g k" },
    { label: m.sc_palette, spec: "mod+k" },
    { label: m.sc_help, spec: "?" },
  ];

const CalendarHelp = () => {
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
        <Dialog.Title>{m.cal_shortcuts()}</Dialog.Title>
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

const Toolbar = ({ onMenu }: { readonly onMenu: () => void }) => {
  const nav = useCalendarNav();
  const locale = useDateLocale();
  const [weekStart, firstDay, setWeekStart] = useWeekStart();
  const [remindersOpen, setRemindersOpen] = useState(false);
  const ui = useMailUi();
  const range = viewRange(nav.view, nav.anchor, locale, firstDay);
  // Phones: a quiet Today, so the date title leads the row.
  const phone = usePhone();
  const options = VIEWS.map((view) => ({
    label: viewLabel(view),
    value: view,
  }));
  const start = (
    <>
      <Button
        onClick={() => {
          nav.goToday();
        }}
        shortcut={["T"]}
        size="sm"
        variant={phone ? "ghost" : "secondary"}
      >
        {m.cal_today()}
      </Button>
      <span {...stylex.props(styles.desktopOnly)}>
        <IconButton
          icon={ChevronLeft}
          label={m.cal_previous()}
          onClick={() => {
            nav.shift(-1);
          }}
          shortcut={["K"]}
        />
        <IconButton
          icon={ChevronRight}
          label={m.cal_next()}
          onClick={() => {
            nav.shift(1);
          }}
          shortcut={["J"]}
        />
      </span>
      <h1 aria-live="polite" {...stylex.props(styles.title, styles.titleGrow)}>
        <span {...stylex.props(styles.titleFull)}>
          {rangeTitle(nav.view, nav.anchor, range, locale)}
        </span>
        <span {...stylex.props(styles.titleShort)}>
          {rangeTitleShort(nav.view, nav.anchor, range, locale)}
        </span>
      </h1>
    </>
  );
  const settingsItems = (
    <>
      <MenuGroup label={m.cal_week_start()}>
        <MenuRadioGroup
          onValueChange={(next) => {
            const choice = WEEK_STARTS.find((candidate) => candidate === next);
            if (choice !== undefined) {
              setWeekStart(choice);
            }
          }}
          value={weekStart}
        >
          {WEEK_STARTS.map((choice) => (
            <MenuRadioItem closeOnClick key={choice} value={choice}>
              {weekStartLabel(choice)}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
      </MenuGroup>
      <MenuItem
        icon={Bell}
        onClick={() => {
          setRemindersOpen(true);
        }}
      >
        {m.cal_reminders_menu()}
      </MenuItem>
      <MenuItem
        icon={Keyboard}
        onClick={() => {
          ui.setHelpOpen(true);
        }}
        shortcut={["?"]}
      >
        {m.cal_shortcuts()}
      </MenuItem>
    </>
  );
  const extras = (
    <>
      <span {...stylex.props(styles.viewSwitch)}>
        <SegmentedControl
          label={m.cal_view()}
          onValueChange={(view) => {
            nav.setView(view);
          }}
          options={options}
          // As tall as the Today button (both 28px).
          size="sm"
          value={nav.view}
        />
      </span>
      <span {...stylex.props(styles.viewMenu)}>
        <Menu
          align="end"
          trigger={
            // The view's name, not just its icon: a lone square or columns
            // glyph says little on a phone.
            <Button
              aria-label={`${m.cal_view()}: ${viewLabel(nav.view)}`}
              iconEnd={ChevronDown}
              size="sm"
              variant="ghost"
            >
              {viewLabel(nav.view)}
            </Button>
          }
        >
          <MenuRadioGroup
            onValueChange={(next) => {
              const view = VIEWS.find((candidate) => candidate === next);
              if (view !== undefined) {
                nav.setView(view);
              }
            }}
            value={nav.view}
          >
            {VIEWS.map((view) => (
              <MenuRadioItem
                closeOnClick
                icon={VIEW_ICONS[view]}
                key={view}
                value={view}
              >
                {viewLabel(view)}
              </MenuRadioItem>
            ))}
          </MenuRadioGroup>
          <MenuSeparator />
          <MenuItem
            icon={ChevronLeft}
            onClick={() => {
              nav.shift(-1);
            }}
          >
            {m.cal_previous()}
          </MenuItem>
          <MenuItem
            icon={ChevronRight}
            onClick={() => {
              nav.shift(1);
            }}
          >
            {m.cal_next()}
          </MenuItem>
          <MenuSeparator />
          {settingsItems}
        </Menu>
      </span>
      <span {...stylex.props(styles.desktopOnly)}>
        <Menu
          align="end"
          trigger={<IconButton icon={Settings2} label={m.cal_settings()} />}
        >
          {settingsItems}
        </Menu>
      </span>
    </>
  );
  return (
    <>
      <AppBar
        extras={extras}
        menuLabel={m.cal_sidebar()}
        onMenu={onMenu}
        palette={false}
        start={start}
      />

      <Dialog.Root open={remindersOpen} onOpenChange={setRemindersOpen}>
        <Dialog.Content closeLabel={m.close()} size="sm">
          <Dialog.Title>{m.cal_reminders()}</Dialog.Title>
          <Dialog.Body>
            <RemindersControl />
          </Dialog.Body>
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
};

const Drawer = ({
  open,
  onClose,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
}) => {
  const dialog = useModalDialog(open);
  return (
    <dialog
      aria-label={m.cal_sidebar()}
      onClose={onClose}
      ref={dialog}
      {...stylex.props(styles.drawer)}
    >
      {open ? (
        // Menus and dialogs of the sidebar must open inside the drawer: the
        // page behind a modal <dialog> is inert.
        <PortalScope>
          <CalendarSidebar onNavigate={onClose} />
        </PortalScope>
      ) : null}
    </dialog>
  );
};

interface PendingMove {
  readonly request: MoveRequest;
}

const ViewArea = () => {
  const nav = useCalendarNav();
  const zone = useTimeZone();
  const locale = useDateLocale();
  const [, firstDay] = useWeekStart();
  const now = useNow();
  const calendars = useCalendars();
  const own = useOwnCalendarAddresses();
  const actions = useEventActions();
  const [pendingMove, setPendingMove] = useState<PendingMove | null>(null);
  const range = viewRange(nav.view, nav.anchor, locale, firstDay);
  const query = useOccurrences(range.start, range.end);
  const tasks = useCalendarTasks(daysOf(range));
  const visible = new Set(
    (calendars.data ?? [])
      .filter((calendar) => calendar.visible)
      .map((calendar) => calendar.key)
  );
  const occurrences = (query.data ?? []).filter(
    (occurrence) =>
      occurrence.calendarKey === null || visible.has(occurrence.calendarKey)
  );

  const open = (occurrence: Occurrence): void => {
    nav.openEvent(refOf(occurrence, calendars.data ?? []));
  };

  const applyMove = async (
    request: MoveRequest,
    scope: "one" | "all"
  ): Promise<void> => {
    const event = await eventFor(
      request.occurrence,
      zone,
      own.data ?? [],
      calendars.data ?? []
    );
    if (event !== null) {
      actions.move.mutate({ event, request, scope });
    }
  };

  const move = (request: MoveRequest): void => {
    const calendar = calendars.data?.find(
      (item) => item.key === request.occurrence.calendarKey
    );
    if (
      calendar?.mayWrite === false ||
      (!request.occurrence.isOrganizer && request.occurrence.hasParticipants)
    ) {
      return;
    }
    if (request.occurrence.recurring) {
      setPendingMove({ request });
    } else {
      void applyMove(request, "all");
    }
  };

  let content: ReactNode = null;
  if (query.data === undefined) {
    content = (
      <div {...stylex.props(styles.notAvailable)}>
        {query.isError ? (
          <p>{m.cal_error_load()}</p>
        ) : (
          <Spinner label={m.cal_loading()} />
        )}
      </div>
    );
  } else if (nav.view === "month") {
    content = (
      <MonthView
        dayExtras={(day) => tasks.forDay(day, false)}
        locale={locale}
        month={nav.anchor.month}
        occurrences={occurrences}
        onDay={(day) => {
          nav.goTo(day, "day");
        }}
        onOpen={open}
        today={nav.todayDate}
        weeks={weeksOf(range)}
        zone={zone}
      />
    );
  } else if (nav.view === "agenda") {
    content = (
      <AgendaView
        dayExtras={(day) => tasks.forDay(day, true)}
        extraDays={tasks.days}
        locale={locale}
        occurrences={occurrences}
        onOpen={open}
        range={range}
        today={nav.todayDate}
        zone={zone}
      />
    );
  } else {
    content = (
      <TimeGrid
        allDayExtras={tasks.allDay}
        days={daysOf(range)}
        key={range.start.toString()}
        locale={locale}
        now={now}
        occurrences={occurrences}
        onDay={(day) => {
          nav.goTo(day, "day");
        }}
        onMove={move}
        onNew={(start, end) => {
          nav.newEvent(start, end ?? null);
        }}
        onOpen={open}
        timedExtras={tasks.timed}
        today={nav.todayDate}
        zone={zone}
      />
    );
  }

  return (
    <>
      {content}
      <ScopeDialog
        onCancel={() => {
          setPendingMove(null);
        }}
        onChoose={(scope) => {
          const request = pendingMove?.request;
          setPendingMove(null);
          if (request !== undefined) {
            void applyMove(request, scope);
          }
        }}
        oneAllowed={
          pendingMove === null ||
          canChangeOneOccurrence(
            {
              allDay: pendingMove.request.occurrence.allDay,
              isOccurrence: true,
            },
            pendingMove.request.occurrence.allDay
          )
        }
        open={pendingMove !== null}
        title={m.cal_move_repeating()}
      />
    </>
  );
};

const useCalendarCommands = (
  onShare: (calendar: CalendarSummary) => void
): void => {
  const nav = useCalendarNav();
  const calendars = useCalendars();
  const ui = useMailUi();
  const navigate = useNavigate();
  const newEvent = (): void => {
    nav.newEventHere();
  };
  useShortcut("c", guard(newEvent));
  useShortcut("t", guard(nav.goToday));
  useShortcut(
    "d",
    guard(() => {
      nav.setView("day");
    })
  );
  useShortcut(
    "w",
    guard(() => {
      nav.setView("week");
    })
  );
  useShortcut(
    "m",
    guard(() => {
      nav.setView("month");
    })
  );
  useShortcut(
    "a",
    guard(() => {
      nav.setView("agenda");
    })
  );
  const next = guard(() => {
    nav.shift(1);
  });
  const previous = guard(() => {
    nav.shift(-1);
  });
  useShortcut("j", next);
  useShortcut("n", next);
  useShortcut("k", previous);
  useShortcut("p", previous);
  useShortcut("mod+k", () => {
    ui.setPaletteOpen(!ui.paletteOpen);
  });
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
      void navigate({ to: "/tasks" });
    })
  );
  const commands: Command[] = [
    {
      group: "actions",
      icon: CalendarPlus,
      id: "cal-new",
      label: m.cal_new_event(),
      run: newEvent,
      shortcut: "c",
    },
    {
      group: "actions",
      icon: CalendarRange,
      id: "cal-today",
      label: m.cal_today(),
      run: nav.goToday,
      shortcut: "t",
    },
    ...VIEWS.map((view) => ({
      group: "goto" as const,
      icon: VIEW_ICONS[view],
      id: `cal-view-${view}`,
      label: viewLabel(view),
      run: () => {
        nav.setView(view);
      },
      shortcut: VIEW_SHORTCUTS[view],
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
      icon: ListTodo,
      id: "goto-tasks",
      keywords: "tasks tareas",
      label: m.rail_tasks(),
      run: () => {
        void navigate({ to: "/tasks" });
      },
      shortcut: "g k",
    },
    // "Share…" of each calendar the user may share (as in the sidebar).
    ...(calendars.data ?? [])
      .filter((calendar) => calendar.mayShare === true && holdsEvents(calendar))
      .map((calendar) => ({
        group: "actions" as const,
        icon: Share2,
        id: `cal-share-${calendar.key}`,
        keywords: "share compartir",
        label: m.cal_share_command({ name: cleanDisplayText(calendar.name) }),
        run: () => {
          onShare(calendar);
        },
      })),
    {
      group: "actions",
      icon: Keyboard,
      id: "help",
      label: m.cal_shortcuts(),
      run: () => {
        ui.setHelpOpen(true);
      },
      shortcut: "?",
    },
  ];
  useCommands(commands);
};

const Layout = () => {
  const nav = useCalendarNav();
  const [drawer, setDrawer] = useState(false);
  // Sharing from ⌘K (the sidebar's menu has its own dialog: on phones it
  // must open inside the drawer).
  const [sharing, setSharing] = useState<CalendarSummary | null>(null);
  const available = useQuery({
    queryFn: calendarsAvailable,
    queryKey: [...calendarKeys.all, "available"],
    staleTime: Infinity,
  }).data;
  useCalendarPush();
  useCalendarCommands(setSharing);

  const panelOpen = nav.event !== null || nav.newAt !== null;
  const fabShrunk = useScrollShrink();
  let panel: ReactNode = null;
  if (nav.newAt !== null) {
    panel = (
      <NewEventEditor
        end={nav.newEnd}
        key={`new-${nav.newAt}-${nav.newEnd ?? ""}`}
        onCancel={() => {
          nav.closePanel();
        }}
        onSaved={(target) => {
          nav.openEvent(target);
        }}
        start={nav.newAt}
      />
    );
  } else if (nav.event !== null && nav.editing) {
    panel = (
      <EditEventEditor
        key={`edit-${nav.event.id}`}
        onCancel={() => {
          if (nav.event !== null) {
            nav.openEvent(nav.event);
          }
        }}
        onSaved={(target) => {
          nav.openEvent(target);
        }}
        target={nav.event}
      />
    );
  } else if (nav.event !== null) {
    const target = nav.event;
    panel = (
      <EventPanel
        key={`view-${target.id}`}
        onClose={() => {
          nav.closePanel();
        }}
        onEdit={() => {
          nav.editEvent(target);
        }}
        target={target}
      />
    );
  }

  return (
    <div {...stylex.props(styles.root)}>
      <div {...stylex.props(styles.side)}>
        <CalendarSidebar />
      </div>
      <header {...stylex.props(styles.header)}>
        <Toolbar
          onMenu={() => {
            setDrawer(true);
          }}
        />
      </header>
      <main {...stylex.props(styles.main)}>
        {available === false ? (
          <Panel xstyle={styles.viewPanel}>
            <div {...stylex.props(styles.notAvailable)}>
              <p>{m.cal_not_available()}</p>
            </div>
          </Panel>
        ) : (
          <>
            <Panel
              label={m.cal_title_app()}
              xstyle={[
                styles.viewPanel,
                panelOpen ? styles.narrowHidden : null,
              ]}
            >
              <ViewArea />
            </Panel>
            {panelOpen ? (
              <Panel label={m.cal_event()} xstyle={styles.panel}>
                {panel}
              </Panel>
            ) : null}
          </>
        )}
      </main>
      <div {...stylex.props(styles.rail)}>
        <AppRail />
      </div>
      <span {...stylex.props(styles.fab, panelOpen ? styles.hidden : null)}>
        <ComposeButton
          icon={CalendarPlus}
          collapsed={fabShrunk ? "fab" : false}
          onClick={() => {
            nav.newEventHere();
          }}
        >
          {m.cal_new_event()}
        </ComposeButton>
      </span>
      <ComposerDock />
      <CommandPalette />
      <CalendarHelp />
      <CalendarReminders />
      <ShareCalendarDialog
        calendar={sharing}
        onOpenChange={(open) => {
          if (!open) {
            setSharing(null);
          }
        }}
      />
      <Drawer
        onClose={() => {
          setDrawer(false);
        }}
        open={drawer}
      />
    </div>
  );
};

const noSubscribe = (): (() => void) => () => {
  // Nothing to stop: the value never changes.
};

/**
 * True in the browser after hydration. Dates depend on the browser's time
 * zone and language, which the server doesn't know: it renders an empty
 * frame and the browser fills it in.
 */
const useInBrowser = (): boolean =>
  useSyncExternalStore(
    noSubscribe,
    () => true,
    () => false
  );

export const CalendarApp = () => {
  const locale = useDateLocale();
  const inBrowser = useInBrowser();
  if (!inBrowser) {
    return (
      <div {...stylex.props(styles.root)}>
        <div {...stylex.props(styles.side)} />
        <main {...stylex.props(styles.main, styles.notAvailable)}>
          <Spinner label={m.cal_loading()} />
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
