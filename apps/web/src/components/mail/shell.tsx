// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The signed-in mail app (Helix): Proton's sidebar, the top
// bar, the floating panels (children), Gmail's app rail, compose windows,
// the ⌘K palette and the global shortcuts. Mail data is fetched in the
// browser only; the server renders this frame with loading states.

import { ComposeButton } from "@nebulamail/ui";
import { colors, shadow, zIndex } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate, useParams } from "@tanstack/react-router";
import {
  CalendarDays,
  Clock,
  Inbox,
  Keyboard,
  Languages,
  ListTodo,
  Monitor,
  Moon,
  PenLine,
  Rows3,
  Search,
  Sun,
  Users,
} from "lucide-react";
import { useEffect } from "react";
import type { ReactNode } from "react";

import { useShortcut } from "../../lib/shortcuts";
import { useSidebarCollapsed } from "../../lib/use-layout";
import { useModalDialog } from "../../lib/use-modal-dialog";
import { useScrollShrink } from "../../lib/use-scroll-shrink";
import { useMailboxes, usePush } from "../../mail/hooks";
import { useScheduleLimit } from "../../mail/use-schedule";
import { m } from "../../paraglide/messages.js";
import { usePreferencesSync } from "../../prefs-sync/use-prefs-sync";
import { CalendarReminders } from "../calendar/reminders";
import { InShellContext } from "../status-page";
import { AppRail } from "./app-rail";
import { bp } from "./breakpoints.stylex";
import { CommandPalette } from "./command-palette";
import type { Command } from "./commands";
import { CommandsProvider, useCommands } from "./commands";
import { useComposer } from "./compose-context";
import { ComposeProvider } from "./compose-provider";
import { ComposerDock } from "./composer-dock";
import { LinkConfirmProvider } from "./link-dialog";
import { MailUiProvider, isInOverlay, useMailUi } from "./mail-ui";
import {
  DENSITIES,
  LANGUAGES,
  THEMES,
  usePreferences,
} from "./settings-popover";
import { ShortcutHelp } from "./shortcut-help";
import { SPECIAL_ICONS, Sidebar, specialLabel } from "./sidebar";
import { TopBar } from "./top-bar";
import { useServerAlerts } from "./use-server-alerts";

const SIDEBAR_KEY = "nebula.sidebar.collapsed";

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
    // Installed app on a phone with a notch (viewport-fit=cover).
    paddingBottom: "env(safe-area-inset-bottom)",
    paddingInlineStart: "env(safe-area-inset-left)",
    paddingTop: "env(safe-area-inset-top)",
    position: "fixed",
    width: "280px",
  },
  fab: {
    bottom: "calc(20px + env(safe-area-inset-bottom))",
    display: { default: "none", [bp.mobile]: "block" },
    insetInlineEnd: "18px",
    position: "fixed",
    width: "150px",
    zIndex: zIndex.raised,
  },
  fabHidden: { display: "none" },
  fabShrunk: { width: "auto" },
  header: { gridArea: "top", minWidth: 0 },
  // Phones reading an email: the reader's own toolbar (with its back
  // arrow) is the top of the screen; the app bar would stack 60px more
  // chrome above the subject.
  headerReading: { display: { default: null, [bp.mobile]: "none" } },
  rootReading: {
    gridTemplateAreas: {
      default: '"side top rail" "side main rail"',
      [bp.compact]: '"side top" "side main"',
      [bp.narrow]: '"side top" "side main"',
      [bp.mobile]: '"main"',
    },
    gridTemplateRows: {
      default: "60px minmax(0, 1fr)",
      [bp.mobile]: "minmax(0, 1fr)",
    },
  },
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
  // Compact windows: the sidebar's icon rail carries the app switcher
  // (sidebar-rail.tsx), so one rail, not two.
  rail: {
    display: { default: "block", [bp.compact]: "none", [bp.narrow]: "none" },
    gridArea: "rail",
    minHeight: 0,
  },
  root: {
    backgroundColor: colors.canvas,
    color: colors.fg,
    display: "grid",
    gridTemplateAreas: {
      default: '"side top rail" "side main rail"',
      [bp.compact]: '"side top" "side main"',
      [bp.narrow]: '"side top" "side main"',
      [bp.mobile]: '"top" "main"',
    },
    gridTemplateRows: "60px minmax(0, 1fr)",
    height: "100dvh",
    overflow: "hidden",
    // Notch, rounded corners and home bar of phones (viewport-fit=cover in
    // __root.tsx); 0 everywhere else.
    boxSizing: "border-box",
    paddingBottom: "env(safe-area-inset-bottom)",
    paddingInlineEnd: "env(safe-area-inset-right)",
    paddingInlineStart: "env(safe-area-inset-left)",
    paddingTop: "env(safe-area-inset-top)",
  },
  rootFull: {
    gridTemplateColumns: {
      default: "252px minmax(0, 1fr) 52px",
      [bp.compact]: "64px minmax(0, 1fr)",
      [bp.narrow]: "252px minmax(0, 1fr)",
      [bp.mobile]: "minmax(0, 1fr)",
    },
  },
  rootRail: {
    gridTemplateColumns: {
      default: "64px minmax(0, 1fr) 52px",
      [bp.compact]: "64px minmax(0, 1fr)",
      [bp.narrow]: "64px minmax(0, 1fr)",
      [bp.mobile]: "minmax(0, 1fr)",
    },
  },
  side: {
    backgroundColor: colors.side,
    display: { default: "block", [bp.mobile]: "none" },
    gridArea: "side",
    minHeight: 0,
    overflow: "hidden",
    position: "relative",
  },
  // The full sidebar steps aside for its rail on compact windows.
  sideFull: { display: { default: "contents", [bp.compact]: "none" } },
  sideAuto: { display: { default: "none", [bp.compact]: "contents" } },
});

/** What a module puts in the shell's sidebar (and the phone drawer). */
export type ShellSidebar = (props: {
  readonly collapsed: boolean;
  /** Omitted where the window forces the rail (compact widths). */
  readonly onToggleCollapsed?: (() => void) | undefined;
  /** Closes the phone drawer after navigating. */
  readonly onNavigate?: (() => void) | undefined;
}) => ReactNode;

const mailSidebar: ShellSidebar = ({
  collapsed,
  onNavigate,
  onToggleCollapsed,
}) => (
  <Sidebar
    collapsed={collapsed}
    {...(onNavigate === undefined ? {} : { onNavigate })}
    {...(onToggleCollapsed === undefined ? {} : { onToggleCollapsed })}
  />
);

export interface MailShellProps {
  readonly children: ReactNode;
  /** The module's sidebar; mail's folders by default. */
  readonly sidebar?: ShellSidebar;
  /** The module's top bar; mail's (search + protection) by default. */
  readonly bar?: ReactNode;
  /**
   * The phones' floating button; "New message" by default, `null` for
   * none.
   */
  readonly fab?: ReactNode;
  /** The phone drawer's name ("Mail folders" by default). */
  readonly drawerLabel?: string;
}

/** Runs `run` unless the key press happened in a dialog or composer. */
const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

/** Shortcuts and palette commands that work everywhere in the app. */
const useGlobalCommands = (): void => {
  const ui = useMailUi();
  const composer = useComposer();
  const navigate = useNavigate();
  const { preferences, setLanguage, setPreferences } = usePreferences();

  const canSchedule = useScheduleLimit() !== null;
  const goTo = (mailbox: string): void => {
    void navigate({ params: { mailbox }, to: "/mail/$mailbox" });
  };

  useShortcut(
    "c",
    guard(() => {
      composer.open();
    })
  );
  useShortcut(
    "/",
    guard(() => {
      ui.focusSearch();
    })
  );
  useShortcut("mod+k", () => {
    ui.setPaletteOpen(!ui.paletteOpen);
  });
  useShortcut(
    "?",
    guard(() => {
      ui.setHelpOpen(true);
    })
  );
  const goToShortcuts = [
    ["g i", "inbox"],
    ["g s", "starred"],
    ["g t", "sent"],
    ["g d", "drafts"],
  ] as const;
  useShortcut(
    "g i",
    guard(() => {
      goTo("inbox");
    })
  );
  useShortcut(
    "g s",
    guard(() => {
      goTo("starred");
    })
  );
  useShortcut(
    "g t",
    guard(() => {
      goTo("sent");
    })
  );
  useShortcut(
    "g d",
    guard(() => {
      goTo("drafts");
    })
  );
  // Calendar: "g c" (sequences win over the lone "c" that composes,
  // lib/shortcuts.ts). Contacts are "people" ("g p").
  useShortcut(
    "g c",
    guard(() => {
      void navigate({ to: "/calendar" });
    })
  );
  useShortcut(
    "g p",
    guard(() => {
      void navigate({ to: "/contacts" });
    })
  );
  // "g t" is Sent: tasks are "g k" (as in "to-do Kanban"; free in all apps).
  useShortcut(
    "g k",
    guard(() => {
      void navigate({ to: "/tasks" });
    })
  );

  const goToCommands: Command[] = [
    ...goToShortcuts.map(([shortcut, mailbox]) => ({
      group: "goto" as const,
      icon: SPECIAL_ICONS[mailbox] ?? Inbox,
      id: `goto-${mailbox}`,
      label: specialLabel(mailbox) ?? mailbox,
      run: () => {
        goTo(mailbox);
      },
      shortcut,
    })),
    ...(["archive", "junk", "trash"] as const).map((mailbox) => ({
      group: "goto" as const,
      icon: SPECIAL_ICONS[mailbox] ?? Inbox,
      id: `goto-${mailbox}`,
      label: specialLabel(mailbox) ?? mailbox,
      run: () => {
        goTo(mailbox);
      },
    })),
    ...(canSchedule
      ? [
          {
            group: "goto" as const,
            icon: Clock,
            id: "goto-scheduled",
            keywords: "scheduled programados later después",
            label: m.nav_scheduled(),
            run: () => {
              void navigate({ to: "/mail/scheduled" });
            },
          },
        ]
      : []),
  ];

  useCommands([
    {
      group: "actions",
      icon: PenLine,
      id: "compose",
      label: m.compose_new(),
      run: () => {
        composer.open();
      },
      shortcut: "c",
    },
    {
      group: "actions",
      icon: Search,
      id: "search",
      label: m.sc_search(),
      run: () => {
        // After the dialog closes and gives focus back.
        setTimeout(ui.focusSearch, 0);
      },
      shortcut: "/",
    },
    {
      group: "actions",
      icon: Keyboard,
      id: "help",
      label: m.help_shortcuts(),
      run: () => {
        ui.setHelpOpen(true);
      },
      shortcut: "?",
    },
    ...goToCommands,
    {
      group: "goto",
      icon: CalendarDays,
      id: "goto-calendar",
      keywords: "calendar calendario events eventos",
      label: m.rail_calendar(),
      run: () => {
        void navigate({ to: "/calendar" });
      },
      shortcut: "g c",
    },
    {
      group: "goto",
      icon: ListTodo,
      id: "goto-tasks",
      keywords: "tasks tareas to-do pendientes",
      label: m.rail_tasks(),
      run: () => {
        void navigate({ to: "/tasks" });
      },
      shortcut: "g k",
    },
    {
      group: "goto",
      icon: Users,
      id: "goto-contacts",
      keywords: "contacts contactos people personas",
      label: m.rail_contacts(),
      run: () => {
        void navigate({ to: "/contacts" });
      },
      shortcut: "g p",
    },
    ...THEMES.map((theme) => ({
      group: "settings" as const,
      icon: { dark: Moon, light: Sun, system: Monitor }[theme.value],
      id: `theme-${theme.value}`,
      keywords: `theme tema ${theme.value}`,
      label: m.palette_setting({
        setting: m.settings_theme(),
        value: theme.label(),
      }),
      run: () => {
        setPreferences({ theme: theme.value });
      },
    })),
    ...DENSITIES.filter((density) => density.value !== preferences.density).map(
      (density) => ({
        group: "settings" as const,
        icon: Rows3,
        id: `density-${density.value}`,
        keywords: `density densidad ${density.value}`,
        label: m.palette_setting({
          setting: m.settings_density(),
          value: density.label(),
        }),
        run: () => {
          setPreferences({ density: density.value });
        },
      })
    ),
    ...LANGUAGES.map((language) => ({
      group: "settings" as const,
      icon: Languages,
      id: `language-${language.value}`,
      keywords: "language idioma",
      label: m.palette_setting({
        setting: m.settings_language(),
        value: language.label(),
      }),
      run: () => {
        setLanguage(language.value);
      },
    })),
  ]);
};

/** Phones: the sidebar as a modal drawer (native <dialog>: focus trap, Esc). */
const Drawer = ({
  label,
  sidebar,
}: {
  readonly label: string;
  readonly sidebar: ShellSidebar;
}) => {
  const ui = useMailUi();
  const { drawerOpen, setDrawerOpen } = ui;
  const dialog = useModalDialog(drawerOpen);
  const handleClose = (): void => {
    setDrawerOpen(false);
  };
  return (
    <dialog
      aria-label={label}
      onClose={handleClose}
      ref={dialog}
      {...stylex.props(styles.drawer)}
    >
      {drawerOpen
        ? sidebar({ collapsed: false, onNavigate: handleClose })
        : null}
    </dialog>
  );
};

const Layout = ({
  bar,
  children,
  drawerLabel = m.nav_label(),
  fab,
  sidebar = mailSidebar,
}: MailShellProps) => {
  const navigate = useNavigate();
  usePush((emailId) => {
    void navigate({
      params: { emailId, mailbox: "inbox" },
      to: "/mail/$mailbox/$emailId",
    });
  });
  useGlobalCommands();
  usePreferencesSync();
  // RFC 3501 §7.1 / RFC 9051 §7.1: the mail server's [ALERT]s.
  useServerAlerts();
  const composer = useComposer();
  const [collapsed, setCollapsed] = useSidebarCollapsed(SIDEBAR_KEY);
  const { data: mailboxes } = useMailboxes();
  // The reader has its own reply bar where the floating button would sit.
  const reading = useParams({ strict: false }).emailId !== undefined;
  const fabShrunk = useScrollShrink();

  // Tab title with the inbox unread count, like every nebulamail.
  const inboxUnread =
    mailboxes?.find((box) => box.role === "inbox")?.unread ?? 0;
  useEffect(() => {
    document.title =
      inboxUnread > 0
        ? `(${inboxUnread}) ${m.nav_inbox()} · ${m.app_name()}`
        : m.app_name();
  }, [inboxUnread]);

  return (
    <div
      {...stylex.props(
        styles.root,
        collapsed ? styles.rootRail : styles.rootFull,
        reading ? styles.rootReading : null
      )}
    >
      <div {...stylex.props(styles.side)}>
        {collapsed ? (
          sidebar({
            collapsed: true,
            onToggleCollapsed: () => {
              setCollapsed(false);
            },
          })
        ) : (
          <>
            <div {...stylex.props(styles.sideFull)}>
              {sidebar({
                collapsed: false,
                onToggleCollapsed: () => {
                  setCollapsed(true);
                },
              })}
            </div>
            <div {...stylex.props(styles.sideAuto)}>
              {sidebar({ collapsed: true })}
            </div>
          </>
        )}
      </div>
      <header
        {...stylex.props(styles.header, reading ? styles.headerReading : null)}
      >
        {bar ?? <TopBar />}
      </header>
      <main {...stylex.props(styles.main)}>
        <InShellContext value>{children}</InShellContext>
      </main>
      <div {...stylex.props(styles.rail)}>
        <AppRail />
      </div>
      {fab === null ? null : (
        <span
          {...stylex.props(
            styles.fab,
            fabShrunk ? styles.fabShrunk : null,
            reading ? styles.fabHidden : null
          )}
        >
          {fab ?? (
            <ComposeButton
              icon={PenLine}
              collapsed={fabShrunk ? "fab" : false}
              onClick={() => {
                composer.open();
              }}
            >
              {m.compose_new()}
            </ComposeButton>
          )}
        </span>
      )}
      <ComposerDock />
      <CommandPalette />
      <ShortcutHelp />
      <CalendarReminders />
      <Drawer label={drawerLabel} sidebar={sidebar} />
    </div>
  );
};

export const MailShell = (props: MailShellProps) => (
  <MailUiProvider>
    <CommandsProvider>
      <ComposeProvider>
        <LinkConfirmProvider>
          <Layout {...props} />
        </LinkConfirmProvider>
      </ComposeProvider>
    </CommandsProvider>
  </MailUiProvider>
);
